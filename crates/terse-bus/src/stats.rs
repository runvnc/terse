//! Latency histograms: logarithmic buckets (32 per doubling) of
//! nanoseconds, so millions of samples cost a few kilobytes.

use std::time::Duration;

const PER_DOUBLING: f64 = 32.0;
const BUCKETS: usize = 32 * 40;

#[derive(Clone)]
pub struct Histogram {
    counts: Vec<u64>,
    pub n: u64,
    pub max_ns: u64,
    pub sum_ns: u128,
}

impl Default for Histogram {
    fn default() -> Self {
        Histogram { counts: vec![0; BUCKETS], n: 0, max_ns: 0, sum_ns: 0 }
    }
}

fn bucket(ns: u64) -> usize {
    if ns <= 1 {
        return 0;
    }
    (((ns as f64).log2() * PER_DOUBLING) as usize).min(BUCKETS - 1)
}

fn bucket_top(i: usize) -> f64 {
    2f64.powf((i as f64 + 1.0) / PER_DOUBLING)
}

impl Histogram {
    pub fn add(&mut self, d: Duration) {
        self.add_ns(d.as_nanos() as u64);
    }

    pub fn add_ns(&mut self, ns: u64) {
        self.counts[bucket(ns)] += 1;
        self.n += 1;
        self.max_ns = self.max_ns.max(ns);
        self.sum_ns += ns as u128;
    }

    /// The `q` quantile, in nanoseconds (the top of its bucket, within
    /// about 2 %).
    pub fn quantile(&self, q: f64) -> f64 {
        if self.n == 0 {
            return 0.0;
        }
        let want = ((self.n as f64 * q).ceil() as u64).max(1);
        let mut seen = 0;
        for (i, &c) in self.counts.iter().enumerate() {
            seen += c;
            if seen >= want {
                return bucket_top(i).min(self.max_ns as f64);
            }
        }
        self.max_ns as f64
    }

    pub fn mean_ns(&self) -> f64 {
        if self.n == 0 { 0.0 } else { self.sum_ns as f64 / self.n as f64 }
    }

    /// "p50 … p99 … max …" in µs.
    pub fn summary_us(&self) -> String {
        format!(
            "n {}, p50 {:.2} µs, p99 {:.2} µs, max {:.2} µs, mean {:.2} µs",
            self.n,
            self.quantile(0.5) / 1000.0,
            self.quantile(0.99) / 1000.0,
            self.max_ns as f64 / 1000.0,
            self.mean_ns() / 1000.0
        )
    }
}

/// p50, p99 and max of `values`.
pub fn spread(values: &mut [f64]) -> (f64, f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    values.sort_by(f64::total_cmp);
    let at = |q: f64| values[((values.len() - 1) as f64 * q).round() as usize];
    (at(0.5), at(0.99), values[values.len() - 1])
}

#[cfg(test)]
mod tests {
    #[test]
    fn quantiles_within_their_bucket() {
        let mut h = super::Histogram::default();
        for ns in 1..=10_000u64 {
            h.add_ns(ns);
        }
        let p50 = h.quantile(0.5);
        assert!((p50 - 5000.0).abs() / 5000.0 < 0.03, "{p50}");
        assert_eq!(h.quantile(1.0), 10_000.0);
        assert_eq!(h.n, 10_000);
    }
}
