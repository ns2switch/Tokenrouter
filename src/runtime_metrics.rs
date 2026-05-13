use crate::domain::ports::RuntimeMetricsSnapshot;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Mutex;

#[derive(Default)]
pub struct RuntimeMetrics {
    inflight: AtomicI64,
    requests_total: AtomicU64,
    status_counts: Mutex<BTreeMap<u16, u64>>,
    model_counts: Mutex<BTreeMap<String, u64>>,
    ttft_buckets: Mutex<BTreeMap<u64, u64>>,
    ttft_sum_ms: AtomicU64,
    duration_buckets: Mutex<BTreeMap<u64, u64>>,
    duration_sum_ms: AtomicU64,
    throughput_tps_x100: AtomicU64,
}

impl RuntimeMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn inc_inflight(&self) {
        self.inflight.fetch_add(1, Ordering::Relaxed);
    }

    pub fn dec_inflight(&self) {
        self.inflight.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn inflight(&self) -> i64 {
        self.inflight.load(Ordering::Relaxed)
    }

    pub fn record_request(
        &self,
        model: &str,
        status_code: u16,
        total_duration_ms: u64,
        ttft_ms: u64,
        throughput_tps: f64,
    ) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        {
            let mut m = self.status_counts.lock().expect("status_counts poisoned");
            *m.entry(status_code).or_insert(0) += 1;
        }
        {
            let mut m = self.model_counts.lock().expect("model_counts poisoned");
            *m.entry(model.to_string()).or_insert(0) += 1;
        }
        {
            let mut b = self.ttft_buckets.lock().expect("ttft_buckets poisoned");
            let bucket = bucket_ms(ttft_ms);
            *b.entry(bucket).or_insert(0) += 1;
        }
        self.ttft_sum_ms.fetch_add(ttft_ms, Ordering::Relaxed);
        {
            let mut b = self
                .duration_buckets
                .lock()
                .expect("duration_buckets poisoned");
            let bucket = bucket_ms(total_duration_ms);
            *b.entry(bucket).or_insert(0) += 1;
        }
        self.duration_sum_ms
            .fetch_add(total_duration_ms, Ordering::Relaxed);
        let scaled = (throughput_tps.max(0.0) * 100.0) as u64;
        self.throughput_tps_x100
            .fetch_add(scaled, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> RuntimeMetricsSnapshot {
        let status = self.status_counts.lock().expect("status_counts poisoned");
        let models = self.model_counts.lock().expect("model_counts poisoned");
        let ttft = self.ttft_buckets.lock().expect("ttft_buckets poisoned");
        let duration = self
            .duration_buckets
            .lock()
            .expect("duration_buckets poisoned");
        RuntimeMetricsSnapshot {
            requests_total: self.requests_total.load(Ordering::Relaxed),
            status_counts: status.iter().map(|(k, v)| (*k, *v)).collect(),
            model_counts: models.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            ttft_buckets: ttft.iter().map(|(k, v)| (*k, *v)).collect(),
            ttft_sum_ms: self.ttft_sum_ms.load(Ordering::Relaxed),
            duration_buckets: duration.iter().map(|(k, v)| (*k, *v)).collect(),
            duration_sum_ms: self.duration_sum_ms.load(Ordering::Relaxed),
            throughput_tps_x100: self.throughput_tps_x100.load(Ordering::Relaxed),
        }
    }

    pub fn merge_snapshot(&self, snap: &RuntimeMetricsSnapshot) {
        self.requests_total
            .fetch_add(snap.requests_total, Ordering::Relaxed);
        {
            let mut m = self.status_counts.lock().expect("status_counts poisoned");
            for (k, v) in &snap.status_counts {
                *m.entry(*k).or_insert(0) += v;
            }
        }
        {
            let mut m = self.model_counts.lock().expect("model_counts poisoned");
            for (k, v) in &snap.model_counts {
                *m.entry(k.clone()).or_insert(0) += v;
            }
        }
        {
            let mut m = self.ttft_buckets.lock().expect("ttft_buckets poisoned");
            for (k, v) in &snap.ttft_buckets {
                *m.entry(*k).or_insert(0) += v;
            }
        }
        self.ttft_sum_ms
            .fetch_add(snap.ttft_sum_ms, Ordering::Relaxed);
        {
            let mut m = self
                .duration_buckets
                .lock()
                .expect("duration_buckets poisoned");
            for (k, v) in &snap.duration_buckets {
                *m.entry(*k).or_insert(0) += v;
            }
        }
        self.duration_sum_ms
            .fetch_add(snap.duration_sum_ms, Ordering::Relaxed);
        self.throughput_tps_x100
            .fetch_add(snap.throughput_tps_x100, Ordering::Relaxed);
    }

    pub fn render_prometheus(&self) -> String {
        let mut out = String::new();

        out.push_str("# HELP tokenrouter_inflight_requests Current in-flight requests\n");
        out.push_str("# TYPE tokenrouter_inflight_requests gauge\n");
        out.push_str(&format!(
            "tokenrouter_inflight_requests {}\n",
            self.inflight.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP tokenrouter_requests_total Total completed requests\n");
        out.push_str("# TYPE tokenrouter_requests_total counter\n");
        out.push_str(&format!(
            "tokenrouter_requests_total {}\n",
            self.requests_total.load(Ordering::Relaxed)
        ));

        out.push_str(
            "# HELP tokenrouter_requests_by_status_total Requests grouped by status code\n",
        );
        out.push_str("# TYPE tokenrouter_requests_by_status_total counter\n");
        let status = self.status_counts.lock().expect("status_counts poisoned");
        for (code, count) in status.iter() {
            out.push_str(&format!(
                "tokenrouter_requests_by_status_total{{status=\"{}\"}} {}\n",
                code, count
            ));
        }
        drop(status);

        out.push_str("# HELP tokenrouter_requests_by_model_total Requests grouped by model\n");
        out.push_str("# TYPE tokenrouter_requests_by_model_total counter\n");
        let models = self.model_counts.lock().expect("model_counts poisoned");
        for (model, count) in models.iter() {
            out.push_str(&format!(
                "tokenrouter_requests_by_model_total{{model=\"{}\"}} {}\n",
                escape_label(model),
                count
            ));
        }
        drop(models);

        render_histogram(
            &mut out,
            "tokenrouter_ttft_ms",
            "Time-to-first-token histogram (ms)",
            &BUCKET_BOUNDARIES,
            &self.ttft_buckets.lock().expect("ttft_buckets poisoned"),
            self.ttft_sum_ms.load(Ordering::Relaxed),
        );

        render_histogram(
            &mut out,
            "tokenrouter_duration_ms",
            "Total request duration histogram (ms)",
            &BUCKET_BOUNDARIES,
            &self
                .duration_buckets
                .lock()
                .expect("duration_buckets poisoned"),
            self.duration_sum_ms.load(Ordering::Relaxed),
        );

        out.push_str("# HELP tokenrouter_throughput_tps_x100_sum Sum of output throughput in tokens/s scaled by 100\n");
        out.push_str("# TYPE tokenrouter_throughput_tps_x100_sum counter\n");
        out.push_str(&format!(
            "tokenrouter_throughput_tps_x100_sum {}\n",
            self.throughput_tps_x100.load(Ordering::Relaxed)
        ));

        out
    }
}

const BUCKET_BOUNDARIES: [u64; 10] = [10, 25, 50, 100, 250, 500, 1000, 2000, 5000, 10000];

fn bucket_ms(v: u64) -> u64 {
    for b in BUCKET_BOUNDARIES {
        if v <= b {
            return b;
        }
    }
    20000
}

fn render_histogram(
    out: &mut String,
    name: &str,
    help: &str,
    boundaries: &[u64],
    buckets: &BTreeMap<u64, u64>,
    sum: u64,
) {
    out.push_str(&format!("# HELP {name} {help}\n"));
    out.push_str(&format!("# TYPE {name} histogram\n"));

    let total: u64 = buckets.values().sum();
    let mut cumulative: u64 = 0;

    for &b in boundaries {
        cumulative += buckets.get(&b).copied().unwrap_or(0);
        out.push_str(&format!("{name}_bucket{{le=\"{b}\"}} {cumulative}\n"));
    }
    out.push_str(&format!("{name}_bucket{{le=\"+Inf\"}} {total}\n"));
    out.push_str(&format!("{name}_sum {sum}\n"));
    out.push_str(&format!("{name}_count {total}\n"));
}

fn escape_label(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_ms_boundaries() {
        assert_eq!(bucket_ms(0), 10);
        assert_eq!(bucket_ms(10), 10);
        assert_eq!(bucket_ms(11), 25);
        assert_eq!(bucket_ms(25), 25);
        assert_eq!(bucket_ms(26), 50);
        assert_eq!(bucket_ms(500), 500);
        assert_eq!(bucket_ms(501), 1000);
        assert_eq!(bucket_ms(10000), 10000);
    }

    #[test]
    fn bucket_ms_overflow_uses_max_bucket() {
        assert_eq!(bucket_ms(10001), 20000);
        assert_eq!(bucket_ms(u64::MAX), 20000);
    }

    #[test]
    fn escape_label_plain() {
        assert_eq!(escape_label("gpt-4"), "gpt-4");
    }

    #[test]
    fn escape_label_quotes_and_backslash() {
        assert_eq!(escape_label(r#"a"b"#), r#"a\"b"#);
        assert_eq!(escape_label(r#"a\b"#), r#"a\\b"#);
    }

    #[test]
    fn inflight_inc_dec() {
        let m = RuntimeMetrics::new();
        assert_eq!(m.inflight.load(Ordering::Relaxed), 0);
        m.inc_inflight();
        m.inc_inflight();
        assert_eq!(m.inflight.load(Ordering::Relaxed), 2);
        m.dec_inflight();
        assert_eq!(m.inflight.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn record_request_counters() {
        let m = RuntimeMetrics::new();
        m.record_request("gpt-4", 200, 500, 100, 10.0);
        m.record_request("gpt-4", 200, 800, 200, 20.0);
        m.record_request("llama-3", 429, 50, 0, 0.0);

        assert_eq!(m.requests_total.load(Ordering::Relaxed), 3);

        let status = m.status_counts.lock().unwrap();
        assert_eq!(status[&200], 2);
        assert_eq!(status[&429], 1);
        drop(status);

        let models = m.model_counts.lock().unwrap();
        assert_eq!(models["gpt-4"], 2);
        assert_eq!(models["llama-3"], 1);
    }

    #[test]
    fn render_prometheus_contains_required_lines() {
        let m = RuntimeMetrics::new();
        m.inc_inflight();
        m.record_request("mymodel", 200, 100, 50, 5.0);

        let out = m.render_prometheus();
        assert!(out.contains("tokenrouter_inflight_requests 1"));
        assert!(out.contains("tokenrouter_requests_total 1"));
        assert!(out.contains(r#"tokenrouter_requests_by_status_total{status="200"} 1"#));
        assert!(out.contains(r#"tokenrouter_requests_by_model_total{model="mymodel"} 1"#));
    }

    #[test]
    fn histogram_renders_cumulative_buckets() {
        let m = RuntimeMetrics::new();
        m.record_request("m", 200, 30, 8, 1.0);
        m.record_request("m", 200, 200, 50, 1.0);
        m.record_request("m", 200, 600, 300, 1.0);

        let out = m.render_prometheus();
        assert!(out.contains("tokenrouter_ttft_ms_bucket{le=\"10\"} 1"));
        assert!(out.contains("tokenrouter_ttft_ms_bucket{le=\"50\"} 2"));
        assert!(out.contains("tokenrouter_ttft_ms_bucket{le=\"500\"} 3"));
        assert!(out.contains("tokenrouter_ttft_ms_bucket{le=\"+Inf\"} 3"));
        assert!(out.contains("tokenrouter_ttft_ms_sum 358"));
        assert!(out.contains("tokenrouter_ttft_ms_count 3"));
    }

    #[test]
    fn throughput_accumulates() {
        let m = RuntimeMetrics::new();
        m.record_request("m", 200, 100, 10, 12.5);
        m.record_request("m", 200, 100, 10, 7.5);
        assert_eq!(m.throughput_tps_x100.load(Ordering::Relaxed), 2000);
    }
}
