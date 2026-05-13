use sha2::{Digest, Sha256};

pub fn hash_api_key(raw_key: &str, secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hasher.update(b":");
    hasher.update(raw_key.as_bytes());
    hex_encode(&hasher.finalize())
}

pub fn hash_payload(payload_json: &str, secret: &str) -> String {
    let mut data = Vec::with_capacity(9 + payload_json.len());
    data.extend_from_slice(b"payload:");
    data.extend_from_slice(payload_json.as_bytes());
    hmac_sha256_hex(secret.as_bytes(), &data)
}

fn hmac_sha256_hex(key: &[u8], data: &[u8]) -> String {
    use std::io::Write;

    const BLOCK_SIZE: usize = 64;
    let mut key_ipad = [0x36u8; BLOCK_SIZE];
    let mut key_opad = [0x5cu8; BLOCK_SIZE];

    let key = if key.len() > BLOCK_SIZE {
        let mut h = Sha256::new();
        h.update(key);
        h.finalize().to_vec()
    } else {
        key.to_vec()
    };

    for (i, &b) in key.iter().enumerate() {
        key_ipad[i] ^= b;
        key_opad[i] ^= b;
    }

    let mut inner = Sha256::new();
    inner.write_all(&key_ipad).unwrap();
    inner.write_all(data).unwrap();
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.write_all(&key_opad).unwrap();
    outer.write_all(&inner_hash).unwrap();
    let final_hash = outer.finalize();

    hex_encode(&final_hash)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_api_key_deterministic() {
        assert_eq!(
            hash_api_key("mykey", "secret"),
            hash_api_key("mykey", "secret")
        );
    }

    #[test]
    fn hash_api_key_hex_64_chars() {
        let h = hash_api_key("key", "secret");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hash_api_key_different_keys() {
        assert_ne!(
            hash_api_key("key1", "secret"),
            hash_api_key("key2", "secret")
        );
    }

    #[test]
    fn hash_api_key_different_secrets() {
        assert_ne!(hash_api_key("key", "s1"), hash_api_key("key", "s2"));
    }

    #[test]
    fn hash_payload_deterministic() {
        let p = r#"{"model":"llama-3"}"#;
        assert_eq!(hash_payload(p, "secret"), hash_payload(p, "secret"));
    }

    #[test]
    fn hash_payload_differs_from_api_key_hash() {
        // Different domain prefixes must produce different digests for identical input.
        assert_ne!(
            hash_api_key("data", "secret"),
            hash_payload("data", "secret")
        );
    }

    #[test]
    fn hash_payload_sensitive_to_content() {
        assert_ne!(
            hash_payload(r#"{"model":"a"}"#, "s"),
            hash_payload(r#"{"model":"b"}"#, "s")
        );
    }
}
