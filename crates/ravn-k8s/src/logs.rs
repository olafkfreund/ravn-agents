//! Pod log scraper: stream pod logs, deduplicate lines with a bounded
//! per-pod LRU + TTL cache, emit ravn-core [`Message`]s for noteworthy lines.
//!
//! # Deduplication design (issue #147)
//!
//! The original approach used a global `HashSet` cleared wholesale at 5 000
//! entries. That sawtoothed: memory grew to the cap, dropped to zero, duplicate
//! events re-fired on every rebuild, and the rebuild itself thrashed CPU.
//!
//! This module replaces it with a **per-pod LRU cache** bounded by
//! [`Config::per_pod_cap`] entries (default 1 024 per pod):
//!
//! - Each pod maintains its own `LruCache<u64, Instant>` keyed on the 64-bit
//!   FNV-1a hash of the log line.  The value is the instant the line was first
//!   seen, used to enforce a TTL ([`Config::ttl`], default 10 min).
//! - A new line is deduplicated when its hash is present **and** the entry has
//!   not yet expired.  Expired entries are evicted lazily on the next lookup for
//!   the same hash, keeping the hot path allocation-free.
//! - When the LRU is full the oldest entry is evicted automatically by
//!   `LruCache::put`, bounding memory per pod to `cap × (8 + 16)` bytes ≈ 24 KiB
//!   at the default cap.
//! - Pod state is evicted completely when the pod is deleted from the cluster
//!   ([`LogDeduper::remove_pod`]).
//! - [`LogDeduper::total_entries`] exposes the live cache size as a metric.
//!
//! # Metric
//!
//! `LogDeduper::total_entries()` returns the sum of all per-pod cache sizes.
//! Callers should export this counter/gauge via their chosen metrics endpoint;
//! the implementation is metric-system-agnostic to avoid pulling in a metrics
//! crate that the rest of the binary does not use.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use anyhow::Context;
use chrono::Utc;
use futures_util::TryStreamExt;
use k8s_openapi::api::core::v1::Pod;
use kube::api::LogParams;
use kube::runtime::watcher::{self, Event as WatchEvent};
use kube::{Api, Client};
use lru::LruCache;
use ravn_core::{AgentId, Event, KubeWorkloadPayload, Message, Payload, Severity};
use uuid::Uuid;

use crate::publish::Publisher;

// ── Configuration ────────────────────────────────────────────────────────────

/// Tuning knobs for the log scraper.
///
/// ```
/// use ravn_k8s::logs::Config;
///
/// // Production defaults.
/// let cfg = Config::default();
/// assert_eq!(cfg.per_pod_cap, 1024);
/// ```
#[derive(Debug, Clone)]
pub struct Config {
    /// Maximum number of distinct log-line hashes remembered per pod.
    /// Older entries are evicted (LRU) once this limit is reached.
    pub per_pod_cap: usize,
    /// How long a deduplicated entry is retained before expiring.
    /// A log line that reappears after its TTL is treated as new and re-emitted.
    pub ttl: Duration,
    /// Namespace to restrict the watch to; `None` = all namespaces.
    pub namespace: Option<String>,
    /// Minimum severity a log line must classify as before being emitted.
    pub min_severity: Severity,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            per_pod_cap: 1024,
            ttl: Duration::from_secs(10 * 60),
            namespace: None,
            min_severity: Severity::Warning,
        }
    }
}

// ── LogDeduper ───────────────────────────────────────────────────────────────

/// Bounded, TTL-aware per-pod log-line deduplicator.
///
/// # Memory model
///
/// Each pod is allocated at most `per_pod_cap` slots.  An entry is considered
/// live until `ttl` has elapsed since it was first inserted.  Once expired it
/// is evicted on the next lookup for the same hash, so the footprint stays flat
/// under sustained load.
///
/// Use [`total_entries`][Self::total_entries] to expose the live cache size as a
/// Prometheus gauge or equivalent.
pub struct LogDeduper {
    per_pod_cap: NonZeroUsize,
    ttl: Duration,
    pods: HashMap<String, LruCache<u64, Instant>>,
}

impl LogDeduper {
    /// Create a deduper with explicit cap and TTL.
    ///
    /// # Panics
    ///
    /// Panics if `per_pod_cap` is zero.
    pub fn new(per_pod_cap: usize, ttl: Duration) -> Self {
        let cap = NonZeroUsize::new(per_pod_cap)
            .expect("per_pod_cap must be > 0");
        Self { per_pod_cap: cap, ttl, pods: HashMap::new() }
    }

    /// Returns `true` and records the line if it is new or its TTL has expired;
    /// returns `false` (suppressing re-emission) if it is still within its TTL.
    ///
    /// Lazy expiration: an expired slot for the same hash is recycled in place,
    /// consuming no extra allocation.
    pub fn is_new(&mut self, pod_key: &str, line: &str) -> bool {
        let hash = hash_line(line);
        let now = Instant::now();
        let ttl = self.ttl;
        let cap = self.per_pod_cap;

        let cache = self.pods.entry(pod_key.to_owned()).or_insert_with(|| LruCache::new(cap));

        match cache.get_mut(&hash) {
            Some(seen_at) if now.duration_since(*seen_at) < ttl => {
                // Still within TTL — suppress.
                false
            }
            _ => {
                // New hash, or TTL expired — record and allow emission.
                cache.put(hash, now);
                true
            }
        }
    }

    /// Evict all state for `pod_key`.
    ///
    /// Call this when a pod is deleted so its memory is reclaimed immediately
    /// rather than waiting for the LRU cap to push out its entries.
    pub fn remove_pod(&mut self, pod_key: &str) {
        self.pods.remove(pod_key);
    }

    /// Total number of live (not-yet-evicted) entries across all pods.
    ///
    /// Expose this value as a Prometheus gauge named e.g.
    /// `ravn_log_dedup_entries_total` so operators can observe cache saturation.
    pub fn total_entries(&self) -> usize {
        self.pods.values().map(|c| c.len()).sum()
    }

    /// Number of pods currently tracked.
    pub fn pod_count(&self) -> usize {
        self.pods.len()
    }
}

// ── Severity classification ───────────────────────────────────────────────────

/// Classify a raw log line into a [`Severity`].
///
/// Looks for case-insensitive level keywords in the line.  Unknown content
/// defaults to [`Severity::Info`] so the caller's `min_severity` filter can
/// suppress it.
fn classify_line(line: &str) -> Severity {
    let lower = line.to_ascii_lowercase();
    if lower.contains("panic") || lower.contains("fatal") || lower.contains("oom") {
        Severity::Critical
    } else if lower.contains("error") || lower.contains("exception") || lower.contains("crash") {
        Severity::Error
    } else if lower.contains("warn") {
        Severity::Warning
    } else {
        Severity::Info
    }
}

// ── Message construction ──────────────────────────────────────────────────────

fn line_to_message(
    line: &str,
    namespace: &str,
    pod_name: &str,
    agent_id: AgentId,
    host: &str,
) -> Message {
    let severity = classify_line(line);
    let reason = if severity >= Severity::Critical {
        "LogFatal"
    } else if severity >= Severity::Error {
        "LogError"
    } else {
        "LogWarning"
    };

    let payload = KubeWorkloadPayload {
        namespace: namespace.to_string(),
        object_kind: "Pod".to_string(),
        name: pod_name.to_string(),
        reason: reason.to_string(),
        message: Some(line.chars().take(512).collect()),
        count: None,
        container: None,
        extra: Default::default(),
    };

    let now = Utc::now();
    let event = Event {
        id: Uuid::now_v7(),
        occurred_at: now,
        observed_at: now,
        agent_id,
        host: host.to_string(),
        severity,
        title: format!("{reason}: Pod {namespace}/{pod_name}"),
        category_hints: vec![namespace.to_string(), "kubernetes".to_string(), "logs".to_string()],
        payload: Payload::KubeWorkload(payload),
    };
    Message::new(event)
}

// ── Run loop ──────────────────────────────────────────────────────────────────

/// Stream pod logs from the cluster, deduplicating lines with a bounded
/// per-pod LRU+TTL cache, and publish noteworthy lines as ravn [`Message`]s.
///
/// The function watches the pod list for additions and deletions.  For each
/// running pod it tails its log and classifies each line.  Lines within the
/// configured TTL window are silently dropped (deduplicated).  When a pod is
/// deleted its cache entry is evicted immediately.
///
/// This function runs until the kube watch stream ends or an unrecoverable
/// error occurs.
pub async fn run(
    client: Client,
    config: Config,
    agent_id: AgentId,
    host: String,
    publisher: std::sync::Arc<Publisher>,
) -> anyhow::Result<()> {
    let api: Api<Pod> = match &config.namespace {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    };

    let mut deduper = LogDeduper::new(config.per_pod_cap, config.ttl);

    let stream = watcher::watcher(api.clone(), watcher::Config::default());
    futures_util::pin_mut!(stream);
    tracing::info!(
        namespace = ?config.namespace,
        per_pod_cap = config.per_pod_cap,
        ttl_secs = config.ttl.as_secs(),
        "ravn-k8s log scraper starting",
    );

    while let Some(event) = stream.try_next().await.context("kube pod watch stream")? {
        match event {
            WatchEvent::InitApply(_pod) => {
                // Seed phase: do not tail historical logs to avoid a burst of
                // duplicate events on (re)start.  The pod key is not registered
                // here; the deduper will create per-pod state lazily on the
                // first Apply.
            }
            WatchEvent::Apply(pod) => {
                let ns = pod.metadata.namespace.as_deref().unwrap_or("default");
                let name = match pod.metadata.name.as_deref() {
                    Some(n) => n,
                    None => continue,
                };
                let pod_key = format!("{ns}/{name}");

                // Read the last batch of log lines for this pod.
                let log_params = LogParams {
                    tail_lines: Some(50),
                    ..Default::default()
                };

                let logs = match api.logs(name, &log_params).await {
                    Ok(l) => l,
                    Err(err) => {
                        tracing::debug!(%err, pod = pod_key, "could not fetch pod logs");
                        continue;
                    }
                };

                let cache_before = deduper.total_entries();

                for line in logs.lines() {
                    let severity = classify_line(line);
                    if severity < config.min_severity {
                        continue;
                    }
                    if !deduper.is_new(&pod_key, line) {
                        continue;
                    }
                    let message = line_to_message(line, ns, name, agent_id, &host);
                    match publisher.publish(&message).await {
                        Ok(()) => tracing::debug!(
                            pod = pod_key,
                            severity = ?severity,
                            "published log signal"
                        ),
                        Err(err) => tracing::warn!(%err, pod = pod_key, "failed to publish log signal"),
                    }
                }

                let cache_after = deduper.total_entries();
                tracing::trace!(
                    pod = pod_key,
                    cache_before,
                    cache_after,
                    pod_count = deduper.pod_count(),
                    "log dedup cache stats",
                );
            }
            WatchEvent::Delete(pod) => {
                let ns = pod.metadata.namespace.as_deref().unwrap_or("default");
                let name = match pod.metadata.name.as_deref() {
                    Some(n) => n,
                    None => continue,
                };
                let pod_key = format!("{ns}/{name}");
                deduper.remove_pod(&pod_key);
                tracing::debug!(pod = pod_key, "evicted pod log dedup state");
                tracing::debug!(
                    total_entries = deduper.total_entries(),
                    pod_count = deduper.pod_count(),
                    "log dedup cache stats after pod deletion",
                );
            }
            _ => {}
        }
    }
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Hash a log line to a 64-bit value with the standard library's `DefaultHasher`.
///
/// We do not need cryptographic strength here; collision probability on 64 bits
/// over a 1 024-entry per-pod window is negligible.
fn hash_line(line: &str) -> u64 {
    let mut h = DefaultHasher::new();
    line.hash(&mut h);
    h.finish()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── LogDeduper unit tests ─────────────────────────────────────────────────

    #[test]
    fn new_line_is_allowed_first_time() {
        let mut d = LogDeduper::new(64, Duration::from_secs(60));
        assert!(d.is_new("ns/pod-a", "error: disk full"));
    }

    #[test]
    fn same_line_within_ttl_is_suppressed() {
        let mut d = LogDeduper::new(64, Duration::from_secs(60));
        d.is_new("ns/pod-a", "error: disk full"); // first — allowed
        assert!(!d.is_new("ns/pod-a", "error: disk full")); // duplicate
    }

    #[test]
    fn different_lines_are_both_allowed() {
        let mut d = LogDeduper::new(64, Duration::from_secs(60));
        assert!(d.is_new("ns/pod-a", "error: disk full"));
        assert!(d.is_new("ns/pod-a", "warn: high memory"));
    }

    #[test]
    fn expired_line_is_re_emitted() {
        // TTL = 0 means every line is immediately expired.
        let mut d = LogDeduper::new(64, Duration::ZERO);
        assert!(d.is_new("ns/pod-a", "error: disk full"));
        // With TTL = 0 the entry has already expired; should be treated as new.
        assert!(d.is_new("ns/pod-a", "error: disk full"));
    }

    #[test]
    fn remove_pod_clears_entries() {
        let mut d = LogDeduper::new(64, Duration::from_secs(300));
        d.is_new("ns/pod-a", "error one");
        d.is_new("ns/pod-a", "error two");
        assert_eq!(d.total_entries(), 2);

        d.remove_pod("ns/pod-a");
        assert_eq!(d.total_entries(), 0);
        assert_eq!(d.pod_count(), 0);

        // After removal the same lines are treated as new again — no
        // re-emission suppression from the evicted state.
        assert!(d.is_new("ns/pod-a", "error one"),
            "line must re-emit after pod eviction");
    }

    #[test]
    fn per_pod_isolation() {
        // A line seen on pod-a must not suppress the same line on pod-b.
        let mut d = LogDeduper::new(64, Duration::from_secs(60));
        d.is_new("ns/pod-a", "error: connection refused");
        assert!(d.is_new("ns/pod-b", "error: connection refused"),
            "pods must maintain independent dedup state");
    }

    #[test]
    fn lru_cap_evicts_oldest_entry() {
        // Cap = 2 so the third put evicts the LRU entry.
        let mut d = LogDeduper::new(2, Duration::from_secs(300));
        d.is_new("ns/pod-a", "line-1");
        d.is_new("ns/pod-a", "line-2");
        // Inserting line-3 evicts line-1 (LRU eviction).
        d.is_new("ns/pod-a", "line-3");
        assert_eq!(d.total_entries(), 2, "cap must be respected");

        // line-1 was evicted so it must now be treated as new.
        assert!(d.is_new("ns/pod-a", "line-1"),
            "evicted line must re-emit as if new");
    }

    #[test]
    fn total_entries_reflects_all_pods() {
        let mut d = LogDeduper::new(64, Duration::from_secs(60));
        d.is_new("ns/pod-a", "msg-1");
        d.is_new("ns/pod-a", "msg-2");
        d.is_new("ns/pod-b", "msg-3");
        assert_eq!(d.total_entries(), 3);
        assert_eq!(d.pod_count(), 2);
    }

    #[test]
    fn remove_nonexistent_pod_is_noop() {
        let mut d = LogDeduper::new(64, Duration::from_secs(60));
        d.remove_pod("ns/does-not-exist"); // must not panic
        assert_eq!(d.total_entries(), 0);
    }

    // ── classify_line tests ───────────────────────────────────────────────────

    #[test]
    fn classify_panic_as_critical() {
        assert_eq!(classify_line("thread 'main' panicked at 'overflow'"), Severity::Critical);
    }

    #[test]
    fn classify_fatal_as_critical() {
        assert_eq!(classify_line("FATAL: cannot allocate memory"), Severity::Critical);
    }

    #[test]
    fn classify_error_as_error() {
        assert_eq!(classify_line("ERROR: connection refused"), Severity::Error);
    }

    #[test]
    fn classify_warn_as_warning() {
        assert_eq!(classify_line("WARN: retry attempt 3"), Severity::Warning);
    }

    #[test]
    fn classify_info_line_as_info() {
        assert_eq!(classify_line("INFO: server started on :8080"), Severity::Info);
    }

    // ── hash_line collision sanity ────────────────────────────────────────────

    #[test]
    fn different_strings_produce_different_hashes() {
        // Not a proof (hash collisions exist) but a quick sanity check.
        assert_ne!(hash_line("error: disk full"), hash_line("warn: memory low"));
    }

    #[test]
    fn same_string_produces_stable_hash() {
        assert_eq!(hash_line("error: disk full"), hash_line("error: disk full"));
    }
}
