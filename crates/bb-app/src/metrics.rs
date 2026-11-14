//! Process counters exposed at `GET /metrics` (Prometheus text format, `admin` scope;
//! TARGET_ARCHITECTURE §15). Hand-rolled atomics, no metrics crate.

use std::{
    collections::BTreeMap,
    fmt::Write as _,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Default)]
pub struct Metrics {
    pub jobs_submitted_total: AtomicU64,
    pub jobs_deduplicated_total: AtomicU64,
    /// Simulations actually started by a worker (AT-4: N identical submissions start one).
    pub simulations_started_total: AtomicU64,
    pub lease_reclaims_total: AtomicU64,
    pub integrity_failures_total: AtomicU64,
    /// Runs this process quarantined. Part of the public read version
    /// (`AppState::data_generation`), so cached demo responses never outlive a quarantine.
    pub runs_quarantined_total: AtomicU64,
    pub nondeterminism_total: AtomicU64,
    pub artifact_cache_hits_total: AtomicU64,
    pub artifact_cache_misses_total: AtomicU64,
    /// Connections acquired from the Postgres pool (every query or transaction acquires one).
    pub db_acquires_total: AtomicU64,
    /// Writes of the daily egress counter to Postgres (one per 5 MB of responses per class):
    /// the only database work warm anonymous traffic causes (FINAL_AUDIT §4 item 6).
    pub egress_flushes_total: AtomicU64,
    pub worker_busy: AtomicU64,
    pub queue_depth: AtomicU64,
    labelled: Mutex<BTreeMap<(String, String), u64>>,
}

impl Metrics {
    pub fn inc(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn get(counter: &AtomicU64) -> u64 {
        counter.load(Ordering::Relaxed)
    }

    /// Increments a labelled counter, e.g. `http_requests_total{route,status}`.
    pub fn inc_labelled(&self, name: &str, labels: String) {
        let mut map = self.labelled.lock().unwrap_or_else(PoisonError::into_inner);
        *map.entry((name.to_string(), labels)).or_insert(0) += 1;
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        let plain = [
            (
                "jobs_submitted_total",
                &self.jobs_submitted_total,
                "counter",
            ),
            (
                "jobs_deduplicated_total",
                &self.jobs_deduplicated_total,
                "counter",
            ),
            (
                "simulations_started_total",
                &self.simulations_started_total,
                "counter",
            ),
            (
                "lease_reclaims_total",
                &self.lease_reclaims_total,
                "counter",
            ),
            (
                "integrity_failures_total",
                &self.integrity_failures_total,
                "counter",
            ),
            (
                "runs_quarantined_total",
                &self.runs_quarantined_total,
                "counter",
            ),
            (
                "nondeterminism_total",
                &self.nondeterminism_total,
                "counter",
            ),
            (
                "artifact_cache_hits_total",
                &self.artifact_cache_hits_total,
                "counter",
            ),
            (
                "artifact_cache_misses_total",
                &self.artifact_cache_misses_total,
                "counter",
            ),
            ("db_acquires_total", &self.db_acquires_total, "counter"),
            (
                "egress_flushes_total",
                &self.egress_flushes_total,
                "counter",
            ),
            ("worker_busy", &self.worker_busy, "gauge"),
            ("queue_depth", &self.queue_depth, "gauge"),
        ];
        for (name, value, kind) in plain {
            let _ = writeln!(out, "# TYPE {name} {kind}");
            let _ = writeln!(out, "{name} {}", Self::get(value));
        }
        let map = self.labelled.lock().unwrap_or_else(PoisonError::into_inner);
        let mut last = "";
        for ((name, labels), value) in map.iter() {
            if name != last {
                let _ = writeln!(out, "# TYPE {name} counter");
                last = name;
            }
            let _ = writeln!(out, "{name}{{{labels}}} {value}");
        }
        if let Some((rss, hwm)) = resident_bytes() {
            let _ = writeln!(out, "# TYPE process_resident_bytes gauge");
            let _ = writeln!(out, "process_resident_bytes {rss}");
            let _ = writeln!(out, "# TYPE process_peak_resident_bytes gauge");
            let _ = writeln!(out, "process_peak_resident_bytes {hwm}");
        }
        out
    }
}

/// `VmRSS` and `VmHWM` from `/proc/self/status` (Linux only).
pub fn resident_bytes() -> Option<(u64, u64)> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let field = |name: &str| {
        status
            .lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|kb| kb.parse::<u64>().ok())
            .map(|kb| kb * 1024)
    };
    Some((field("VmRSS:")?, field("VmHWM:")?))
}
