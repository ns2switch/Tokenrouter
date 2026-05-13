use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static FAILED_ATTEMPTS: std::sync::LazyLock<Mutex<HashMap<String, (u32, Instant)>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

fn session_secret() -> String {
    std::env::var("API_KEY_HASH_SECRET").unwrap_or_default()
}

fn rate_limit_config() -> (u32, Duration) {
    let max_failures = std::env::var("ADMIN_RATE_LIMIT_MAX_FAILURES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5u32);
    let window = std::env::var("ADMIN_RATE_LIMIT_WINDOW_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60u64);
    (max_failures, Duration::from_secs(window))
}

fn check_rate_limit(ip: &str) -> bool {
    let (max_failures, window) = rate_limit_config();
    let now = Instant::now();
    let mut map = FAILED_ATTEMPTS.lock().expect("FAILED_ATTEMPTS poisoned");

    map.retain(|_, (_, first)| now.duration_since(*first) <= window);

    if let Some((failures, first)) = map.get(ip) {
        if now.duration_since(*first) > window {
            map.remove(ip);
            return true;
        }
        if *failures >= max_failures {
            return false;
        }
    }
    true
}

fn record_failed_attempt(ip: &str) {
    let now = Instant::now();
    let mut map = FAILED_ATTEMPTS.lock().expect("FAILED_ATTEMPTS poisoned");
    let entry = map.entry(ip.to_string()).or_insert((0, now));
    entry.0 += 1;
    entry.1 = now;
}

fn client_ip(req: &Request<Body>) -> Option<String> {
    req.headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next().map(|s| s.trim().to_string()))
}

pub async fn require_admin_bearer(req: Request<Body>, next: Next) -> Result<Response, StatusCode> {
    let ip = client_ip(&req).unwrap_or_else(|| "unknown".to_string());

    if !check_rate_limit(&ip) {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }

    let ip_allowlist = load_ip_allowlist();
    if !ip_allowlist.is_empty() {
        let client_ip: Option<String> = req
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next().map(|s| s.trim().to_string()));
        match client_ip {
            Some(ref ip) if !ip_allowlist.iter().any(|cidr| ip_matches_cidr(ip, cidr)) => {
                return Err(StatusCode::FORBIDDEN);
            }
            Some(_) => {}
            None => return Err(StatusCode::FORBIDDEN),
        }
    }

    let tokens = load_admin_tokens();
    if tokens.is_empty() {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    let auth = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.to_string())
        .or_else(|| {
            extract_cookie(&req, "admin_token").and_then(|sid| lookup_session(&sid).or(Some(sid)))
        });

    match auth {
        Some(ref token)
            if tokens
                .iter()
                .any(|t| constant_time_eq(t.as_bytes(), token.as_bytes())) =>
        {
            Ok(next.run(req).await)
        }
        _ => {
            record_failed_attempt(&ip);
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

pub(crate) fn load_admin_tokens() -> Vec<String> {
    if let Ok(csv) = std::env::var("ADMIN_BEARER_TOKENS") {
        let tokens: Vec<String> = csv
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        if !tokens.is_empty() {
            return tokens;
        }
    }
    if let Ok(single) = std::env::var("ADMIN_BEARER_TOKEN") {
        if !single.trim().is_empty() {
            return vec![single.trim().to_string()];
        }
    }
    vec![]
}

pub(crate) fn store_session(token: &str) -> String {
    crate::security::hash_api_key(&format!("__admin_session__{token}"), &session_secret())
}

pub(crate) fn lookup_session(session_id: &str) -> Option<String> {
    let secret = session_secret();
    if secret.is_empty() {
        return None;
    }
    let tokens = load_admin_tokens();
    tokens.into_iter().find(|t| {
        let expected = crate::security::hash_api_key(&format!("__admin_session__{t}"), &secret);
        constant_time_eq(session_id.as_bytes(), expected.as_bytes())
    })
}

fn load_ip_allowlist() -> Vec<String> {
    std::env::var("ADMIN_IP_ALLOWLIST")
        .ok()
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Simple CIDR match — supports exact IP and /24, /16, /8 prefixes.
fn ip_matches_cidr(ip: &str, cidr: &str) -> bool {
    let (prefix_str, mask_bits) = match cidr.split_once('/') {
        Some((p, m)) => (p, m.parse::<u8>().unwrap_or(32)),
        None => (cidr, 32),
    };

    let ip_u32 = ip_to_u32(ip);
    let prefix_u32 = ip_to_u32(prefix_str);

    match (ip_u32, prefix_u32) {
        (Some(ip), Some(prefix)) => {
            if mask_bits == 0 {
                return true;
            }
            let mask = if mask_bits >= 32 {
                0xFFFF_FFFFu32
            } else {
                !0u32 << (32 - mask_bits)
            };
            (ip & mask) == (prefix & mask)
        }
        _ => false,
    }
}

fn ip_to_u32(ip: &str) -> Option<u32> {
    let parts: Vec<&str> = ip.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut result = 0u32;
    for part in parts {
        let octet = part.parse::<u32>().ok()?;
        if octet > 255 {
            return None;
        }
        result = (result << 8) | octet;
    }
    Some(result)
}

pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn extract_cookie(req: &Request<Body>, name: &str) -> Option<String> {
    let header = req.headers().get("cookie")?.to_str().ok()?;
    let prefix = format!("{name}=");
    for part in header.split("; ") {
        if let Some(v) = part.strip_prefix(&prefix) {
            return Some(v.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_matching() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(b"!@#$", b"!@#$"));
    }

    #[test]
    fn constant_time_eq_non_matching() {
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"", b"a"));
    }

    #[test]
    fn ip_matches_cidr_exact() {
        assert!(ip_matches_cidr("192.168.1.5", "192.168.1.5"));
        assert!(ip_matches_cidr("10.0.0.1", "10.0.0.1"));
        assert!(!ip_matches_cidr("192.168.1.5", "192.168.1.6"));
    }

    #[test]
    fn ip_matches_cidr_subnet_24() {
        assert!(ip_matches_cidr("10.0.0.1", "10.0.0.0/24"));
        assert!(ip_matches_cidr("10.0.0.255", "10.0.0.0/24"));
        assert!(!ip_matches_cidr("10.0.1.1", "10.0.0.0/24"));
    }

    #[test]
    fn ip_matches_cidr_subnet_16() {
        assert!(ip_matches_cidr("172.16.0.1", "172.16.0.0/16"));
        assert!(ip_matches_cidr("172.16.255.255", "172.16.0.0/16"));
        assert!(!ip_matches_cidr("172.17.0.1", "172.16.0.0/16"));
    }

    #[test]
    fn ip_matches_cidr_subnet_8() {
        assert!(ip_matches_cidr("10.255.255.255", "10.0.0.0/8"));
        assert!(!ip_matches_cidr("11.0.0.1", "10.0.0.0/8"));
    }

    #[test]
    fn ip_matches_cidr_default_32() {
        assert!(ip_matches_cidr("1.2.3.4", "1.2.3.4"));
        assert!(!ip_matches_cidr("1.2.3.5", "1.2.3.4"));
    }

    #[test]
    fn ip_matches_cidr_zero_prefix() {
        assert!(ip_matches_cidr("255.255.255.255", "0.0.0.0/0"));
        assert!(ip_matches_cidr("0.0.0.0", "0.0.0.0/0"));
    }

    #[test]
    fn ip_matches_cidr_invalid_ip() {
        assert!(!ip_matches_cidr("not.an.ip", "0.0.0.0/0"));
        assert!(!ip_matches_cidr("1.2.3.4", "not.cidr"));
        assert!(!ip_matches_cidr("1.2.3", "1.2.3.4"));
    }

    #[test]
    fn ip_to_u32_conversion() {
        assert_eq!(ip_to_u32("0.0.0.0"), Some(0));
        assert_eq!(ip_to_u32("255.255.255.255"), Some(0xFFFFFFFF));
        assert_eq!(ip_to_u32("192.168.1.1"), Some(0xC0A80101));
        assert_eq!(ip_to_u32("10.0.0.1"), Some(0x0A000001));
    }

    #[test]
    fn ip_to_u32_invalid() {
        assert_eq!(ip_to_u32("not.an.ip"), None);
        assert_eq!(ip_to_u32("1.2.3"), None);
        assert_eq!(ip_to_u32("256.0.0.0"), None);
    }
}
