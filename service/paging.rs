//! Cheap process/host counters, sampled only on the inference worker.
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Clone, Debug, Default, Serialize)]
pub struct Snapshot {
    vm_swap_kb: Option<u64>,
    rss_kb: Option<u64>,
    rss_anon_kb: Option<u64>,
    rss_file_kb: Option<u64>,
    minor_faults: Option<u64>,
    major_faults: Option<u64>,
    host_mem_available_kb: Option<u64>,
    host_swap_total_kb: Option<u64>,
    host_swap_free_kb: Option<u64>,
}
fn kb(text: &str, key: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name == key)
            .then(|| value.split_whitespace().next()?.parse().ok())
            .flatten()
    })
}
impl Snapshot {
    fn parse(status: &str, stat: &str, mem: &str) -> Self {
        // comm can contain spaces and parentheses. Fields after the last ')' start at field 3.
        let fields: Vec<_> = stat
            .rsplit_once(')')
            .map(|(_, s)| s.split_whitespace().collect())
            .unwrap_or_default();
        Self {
            vm_swap_kb: kb(status, "VmSwap"),
            rss_kb: kb(status, "VmRSS"),
            rss_anon_kb: kb(status, "RssAnon"),
            rss_file_kb: kb(status, "RssFile"),
            minor_faults: fields.get(7).and_then(|s| s.parse().ok()),
            major_faults: fields.get(9).and_then(|s| s.parse().ok()),
            host_mem_available_kb: kb(mem, "MemAvailable"),
            host_swap_total_kb: kb(mem, "SwapTotal"),
            host_swap_free_kb: kb(mem, "SwapFree"),
        }
    }
    pub fn read() -> Self {
        Self::read_process(std::process::id())
    }
    pub fn read_process(pid: u32) -> Self {
        Self::parse(
            &std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default(),
            &std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default(),
            &std::fs::read_to_string("/proc/meminfo").unwrap_or_default(),
        )
    }
    pub fn record(&self, stage: &str, phase: &str, before: Option<&Self>) -> Value {
        fn delta(a: Option<u64>, b: Option<u64>) -> Option<u64> {
            a.zip(b).and_then(|(a, b)| a.checked_sub(b))
        }
        json!({"event":"paging_snapshot", "stage":stage, "phase":phase, "process_pid":std::process::id(),
            "memory":self,
            "minor_faults_delta":before.and_then(|b| delta(self.minor_faults,b.minor_faults)),
            "major_faults_delta":before.and_then(|b| delta(self.major_faults,b.major_faults))})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_and_malformed_counters_are_null() {
        let s = Snapshot::parse("VmSwap: broken kB\nRssFile: 42 kB", "bad", "SwapFree: 7 kB");
        let r = s.record("idle_warmup", "after", Some(&Snapshot::default()));
        assert_eq!(r["event"], "paging_snapshot");
        assert_eq!(r["memory"]["rss_file_kb"], 42);
        assert_eq!(r["memory"]["host_swap_free_kb"], 7);
        assert!(r["memory"]["vm_swap_kb"].is_null());
        assert!(r["major_faults_delta"].is_null());
    }
    #[test]
    fn stat_comm_and_fault_deltas() {
        let a = Snapshot::parse(
            "VmSwap: 8 kB\nRssAnon: 99 kB",
            "12 (name ) with spaces) S 0 0 0 0 0 0 10 0 3 0",
            "MemAvailable: 123 kB",
        );
        let b = Snapshot::parse("", "12 (x) S 0 0 0 0 0 0 17 0 5 0", "");
        let r = b.record("stream_feed", "after", Some(&a));
        assert_eq!(a.vm_swap_kb, Some(8));
        assert_eq!(a.host_mem_available_kb, Some(123));
        assert_eq!(r["minor_faults_delta"], 7);
        assert_eq!(r["major_faults_delta"], 2);
        assert!(a.record("x", "before", None)["minor_faults_delta"].is_null());
    }
}
