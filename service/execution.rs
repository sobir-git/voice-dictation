//! Bounded, fail-open Linux execution evidence on significant inference calls only.
//! Proc/sys intervals include sampling skew; the process clock brackets native work separately.
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

type Counters = BTreeMap<String, u64>;
const MAX_THREADS: usize = 64;
const MAX_ENTRIES: usize = 128;
const MAX_FILES: usize = 512;
const MAX_BYTES: usize = 262_144;
const MAX_FILE: usize = 16_384;

fn numbers(text: &str) -> Counters {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((
                fields.next()?.trim_end_matches(':').to_owned(),
                fields.next()?.parse().ok()?,
            ))
        })
        .take(32)
        .collect()
}
fn psi(text: &str) -> Counters {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let kind = fields.next()?;
            if !matches!(kind, "some" | "full") {
                return None;
            }
            let total = fields
                .find_map(|f| f.strip_prefix("total="))?
                .parse()
                .ok()?;
            Some((format!("{kind}_us"), total))
        })
        .collect()
}
fn deltas(after: &Counters, before: &Counters) -> Value {
    let keys: std::collections::BTreeSet<_> = before.keys().chain(after.keys()).collect();
    Value::Object(
        keys.into_iter()
            .map(|key| {
                (
                    key.clone(),
                    json!(after
                        .get(key)
                        .zip(before.get(key))
                        .and_then(|(a, b)| a.checked_sub(*b))),
                )
            })
            .collect(),
    )
}
fn named_number(text: &str, key: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        (fields.next()?.trim_end_matches(':') == key)
            .then(|| fields.next()?.parse().ok())
            .flatten()
    })
}
fn valid_cgroup(path: &str) -> bool {
    path.starts_with('/') && path.len() <= 256 && !path.split('/').any(|s| matches!(s, ".." | "."))
}
fn stat_fields(text: &str) -> Vec<&str> {
    text.rsplit_once(')')
        .map(|(_, tail)| tail.split_whitespace().collect())
        .unwrap_or_default()
}

struct Reader {
    started: Instant,
    files: usize,
    bytes: usize,
    capped: bool,
    unknown: BTreeMap<String, String>,
}
impl Reader {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            files: 0,
            bytes: 0,
            capped: false,
            unknown: BTreeMap::new(),
        }
    }
    fn available(&mut self) -> bool {
        let ok = self.files < MAX_FILES
            && self.bytes < MAX_BYTES
            && self.started.elapsed() < Duration::from_millis(20);
        self.capped |= !ok;
        ok
    }
    fn read(&mut self, path: &Path, label: &str) -> Option<String> {
        if !self.available() {
            return None;
        }
        self.files += 1;
        let mut bytes = Vec::new();
        let limit = (MAX_FILE + 1).min(MAX_BYTES - self.bytes);
        let result =
            fs::File::open(path).and_then(|f| f.take(limit as u64).read_to_end(&mut bytes));
        self.bytes += bytes.len();
        let result = match result {
            Ok(_) if bytes.len() <= MAX_FILE && bytes.len() < limit => {
                String::from_utf8(bytes).map_err(|_| "invalid_utf8".to_owned())
            }
            Ok(_) => {
                self.capped = true;
                Err("file_or_byte_budget_capped".to_owned())
            }
            Err(e) => Err(format!("{:?}", e.kind())),
        };
        match result {
            Ok(s) => Some(s),
            Err(e) => {
                self.unknown(label, &e);
                None
            }
        }
    }
    fn unknown(&mut self, label: &str, reason: &str) {
        if self.unknown.len() < 24 || self.unknown.contains_key(label) {
            self.unknown.insert(label.to_owned(), reason.to_owned());
        } else {
            self.capped = true;
        }
    }
    fn entries(&mut self, path: &Path, label: &str) -> Vec<PathBuf> {
        if !self.available() {
            return vec![];
        }
        match fs::read_dir(path) {
            Ok(entries) => {
                let mut result = Vec::new();
                for entry in entries.take(MAX_ENTRIES + 1) {
                    if result.len() == MAX_ENTRIES {
                        self.capped = true;
                        break;
                    }
                    match entry {
                        Ok(e) => result.push(e.path()),
                        Err(_) => self.unknown(label, "entry_unavailable"),
                    }
                }
                result.sort();
                result
            }
            Err(e) => {
                self.unknown(label, &format!("{:?}", e.kind()));
                vec![]
            }
        }
    }
    fn counter(&mut self, path: &Path, label: &str) -> Option<u64> {
        let text = self.read(path, label)?;
        let value = text.trim().parse().ok();
        if value.is_none() {
            self.unknown(label, "malformed");
        }
        value
    }
    fn group(&mut self, path: &Path, label: &str, pressure: bool) -> Counters {
        let Some(text) = self.read(path, label) else {
            return Counters::new();
        };
        let values = if pressure { psi(&text) } else { numbers(&text) };
        if values.is_empty() {
            self.unknown(label, "malformed_or_empty");
        }
        values
    }
}
#[derive(Clone, Default)]
struct Thread {
    start: Option<u64>,
    counters: Counters,
    cpu: Option<u64>,
    allowed: Option<String>,
}
fn thread(stat: &str, status: &str, sched: &str, enabled: bool) -> Thread {
    let fields = stat_fields(stat);
    let mut counters = Counters::new();
    let values: Vec<_> = sched
        .split_whitespace()
        .take(3)
        .map(str::parse::<u64>)
        .collect();
    if values.len() == 3 && values.iter().all(Result::is_ok) {
        counters.insert("runtime_ns".into(), values[0].as_ref().copied().unwrap());
        // Linux can return zero while schedstats is disabled. This is unavailable, not zero wait.
        if enabled {
            counters.insert(
                "runqueue_wait_ns".into(),
                values[1].as_ref().copied().unwrap(),
            );
            counters.insert("timeslices".into(), values[2].as_ref().copied().unwrap());
        }
    }
    for key in ["voluntary_ctxt_switches", "nonvoluntary_ctxt_switches"] {
        if let Some(value) = named_number(status, key) {
            counters.insert(key.into(), value);
        }
    }
    Thread {
        start: fields.get(19).and_then(|v| v.parse().ok()),
        cpu: fields.get(36).and_then(|v| v.parse().ok()),
        allowed: status
            .lines()
            .find_map(|l| l.strip_prefix("Cpus_allowed_list:"))
            .map(|s| s.trim().chars().take(128).collect()),
        counters,
    }
}
fn thread_delta(
    after: &BTreeMap<u32, Thread>,
    before: &BTreeMap<u32, Thread>,
    capped: bool,
) -> Value {
    let mut sums = BTreeMap::<String, Option<u64>>::new();
    let mut matched = 0;
    let mut reset = false;
    for (tid, a) in after {
        let Some(b) = before
            .get(tid)
            .filter(|b| a.start.is_some() && a.start == b.start)
        else {
            continue;
        };
        matched += 1;
        for key in [
            "runtime_ns",
            "runqueue_wait_ns",
            "timeslices",
            "voluntary_ctxt_switches",
            "nonvoluntary_ctxt_switches",
        ] {
            let delta = a
                .counters
                .get(key)
                .zip(b.counters.get(key))
                .and_then(|(a, b)| a.checked_sub(*b));
            reset |= a
                .counters
                .get(key)
                .zip(b.counters.get(key))
                .is_some_and(|(a, b)| a < b);
            let sum = sums.entry(key.into()).or_insert(Some(0));
            *sum = sum.zip(delta).and_then(|(s, d)| s.checked_add(d));
        }
    }
    json!({"matched_tids":matched,"before_tids":before.len(),"after_tids":after.len(),
        "partial":capped || reset || matched == 0 || matched != before.len() || matched != after.len() || sums.values().any(Option::is_none),
        "thread_churn":matched != before.len() || matched != after.len(),"counter_reset":reset,"matched_tid_sums":if matched == 0 { Value::Null } else { json!(sums) }})
}

pub struct Snapshot {
    threads: BTreeMap<u32, Thread>,
    groups: BTreeMap<String, Counters>,
    settings: Value,
    hardware: Value,
    schedstats: Option<u64>,
    cgroup: Option<String>,
    cgroup_identity: Option<(u64, u64)>,
    unknown: BTreeMap<String, String>,
    capped: bool,
    threads_capped: bool,
    pub sample_ms: f64,
}
impl Snapshot {
    pub fn read() -> Self {
        Self::read_at(Path::new("/proc"), Path::new("/sys"))
    }
    fn read_at(proc: &Path, sys: &Path) -> Self {
        let mut r = Reader::new();
        let schedstats = r.counter(
            &proc.join("sys/kernel/sched_schedstats"),
            "sched_schedstats",
        );
        let mut threads = BTreeMap::new();
        let tasks = r.entries(&proc.join("self/task"), "tasks");
        let threads_capped = tasks.len() > MAX_THREADS;
        r.capped |= threads_capped;
        for path in tasks
            .into_iter()
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.parse::<u32>().is_ok())
            })
            .take(MAX_THREADS)
        {
            let tid = path.file_name().unwrap().to_str().unwrap().parse().unwrap();
            let stat = r
                .read(&path.join("stat"), "thread_stat")
                .unwrap_or_default();
            let status = r
                .read(&path.join("status"), "thread_status")
                .unwrap_or_default();
            let sched = r
                .read(&path.join("schedstat"), "thread_schedstat")
                .unwrap_or_default();
            let t = thread(&stat, &status, &sched, schedstats == Some(1));
            if t.start.is_none() || t.counters.is_empty() {
                r.unknown("threads", "partial_or_malformed");
            }
            threads.insert(tid, t);
        }
        let threads_capped = threads_capped || r.capped || r.unknown.contains_key("tasks");
        let mut groups = BTreeMap::new();
        let host_stat = r.read(&proc.join("stat"), "host_stat").unwrap_or_default();
        let cpu: Vec<_> = host_stat
            .lines()
            .find_map(|l| l.strip_prefix("cpu "))
            .unwrap_or_default()
            .split_whitespace()
            .take(8)
            .map(str::parse::<u64>)
            .collect();
        let mut host = Counters::new();
        if cpu.len() == 8 && cpu.iter().all(Result::is_ok) {
            for (key, i) in [("idle_ticks", 3), ("iowait_ticks", 4), ("steal_ticks", 7)] {
                host.insert(key.into(), *cpu[i].as_ref().unwrap());
            }
            let busy = [0, 1, 2, 5, 6]
                .into_iter()
                .try_fold(0_u64, |s, i| s.checked_add(*cpu[i].as_ref().unwrap()));
            if let Some(busy) = busy {
                host.insert("busy_ticks".into(), busy);
            }
        } else {
            r.unknown("host_stat", "malformed");
        }
        groups.insert("host_cpu".into(), host);
        for resource in ["cpu", "memory", "io"] {
            groups.insert(
                format!("host_{resource}_pressure"),
                r.group(
                    &proc.join(format!("pressure/{resource}")),
                    &format!("host_{resource}_pressure"),
                    true,
                ),
            );
        }
        let load = r.read(&proc.join("loadavg"), "loadavg").unwrap_or_default();
        let load: Vec<_> = load.split_whitespace().take(4).collect();
        let loadavg: Vec<_> = load
            .iter()
            .take(3)
            .map(|v| v.parse::<f64>().ok().filter(|v| v.is_finite()))
            .collect();
        let runqueue = load
            .get(3)
            .and_then(|s| s.split_once('/'))
            .map(|(a, b)| json!({"runnable":a.parse::<u64>().ok(),"tasks":b.parse::<u64>().ok()}));

        let cg_text = r
            .read(&proc.join("self/cgroup"), "cgroup_path")
            .unwrap_or_default();
        let cgroup = cg_text
            .lines()
            .find_map(|l| l.strip_prefix("0::"))
            .filter(|s| valid_cgroup(s))
            .map(str::to_owned);
        let mut limits = vec![];
        let mut cgroup_identity = None;
        if let Some(cg) = &cgroup {
            let root = sys.join("fs/cgroup");
            // Standard unified mount only. Nonstandard mounts are explicitly unknown.
            let mut path = root.join(cg.trim_start_matches('/'));
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                cgroup_identity = fs::metadata(&path).ok().map(|m| (m.dev(), m.ino()));
            }
            for depth in 0..8 {
                let label = format!("cgroup_{depth}");
                let max = r
                    .read(&path.join("cpu.max"), &format!("{label}_cpu_max"))
                    .map(|s| s.trim().chars().take(128).collect::<String>());
                limits.push(json!({"depth":depth,"cpu_max":max}));
                groups.insert(
                    format!("{label}_cpu_stat"),
                    r.group(&path.join("cpu.stat"), &format!("{label}_cpu_stat"), false),
                );
                if depth == 0 {
                    groups.insert(
                        "cgroup_memory_events".into(),
                        r.group(&path.join("memory.events"), "cgroup_memory_events", false),
                    );
                    for resource in ["cpu", "memory", "io"] {
                        groups.insert(
                            format!("cgroup_{resource}_pressure"),
                            r.group(
                                &path.join(format!("{resource}.pressure")),
                                &format!("cgroup_{resource}_pressure"),
                                true,
                            ),
                        );
                    }
                }
                if path == root {
                    break;
                }
                if depth == 7 {
                    r.capped = true;
                }
                if !path.pop() || !path.starts_with(&root) {
                    break;
                }
            }
        } else {
            r.unknown("cgroup_path", "v2_unavailable_or_malformed");
        }
        let settings = json!({"loadavg":loadavg,"runqueue":runqueue,"procs_running":named_number(&host_stat,"procs_running"),"procs_blocked":named_number(&host_stat,"procs_blocked"),"cgroup_cpu_limits":limits});
        let mut frequencies = vec![];
        let policies = r.entries(&sys.join("devices/system/cpu/cpufreq"), "cpufreq");
        if policies.len() > 16 {
            r.capped = true;
        }
        for path in policies.into_iter().take(16) {
            let mut values = serde_json::Map::new();
            values.insert(
                "policy".into(),
                json!(path.file_name().unwrap().to_string_lossy()),
            );
            for name in [
                "scaling_cur_freq",
                "cpuinfo_cur_freq",
                "scaling_min_freq",
                "scaling_max_freq",
                "scaling_governor",
                "scaling_driver",
                "affected_cpus",
            ] {
                values.insert(
                    name.into(),
                    json!(r.read(&path.join(name), "cpufreq_field").map(|s| s
                        .trim()
                        .chars()
                        .take(128)
                        .collect::<String>())),
                );
            }
            frequencies.push(Value::Object(values));
        }
        let mut temperatures = vec![];
        let zones = r
            .entries(&sys.join("class/thermal"), "thermal")
            .into_iter()
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("thermal_zone")
            })
            .collect::<Vec<_>>();
        if zones.len() > 8 {
            r.capped = true;
        }
        for path in zones.into_iter().take(8) {
            temperatures.push(json!({"zone":path.file_name().unwrap().to_string_lossy(),"type":r.read(&path.join("type"),"thermal_type").map(|s| s.trim().chars().take(64).collect::<String>()),"millidegrees_c":r.read(&path.join("temp"),"thermal_temp").and_then(|s| s.trim().parse::<i64>().ok())}));
        }
        // Package counters are shared across CPUs: retain individual identities, never sum them.
        let cpus = r
            .entries(&sys.join("devices/system/cpu"), "cpu_thermal")
            .into_iter()
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .strip_prefix("cpu")
                    .is_some_and(|s| s.parse::<u32>().is_ok())
            })
            .collect::<Vec<_>>();
        if cpus.len() > 16 {
            r.capped = true;
        }
        for path in cpus.into_iter().take(16) {
            let name = path.file_name().unwrap().to_string_lossy();
            let mut thermal = Counters::new();
            for field in ["core_throttle_count", "package_throttle_count"] {
                if let Some(value) = r.counter(
                    &path.join(format!("thermal_throttle/{field}")),
                    &format!("thermal_{field}"),
                ) {
                    thermal.insert(field.into(), value);
                }
            }
            groups.insert(format!("thermal_{name}"), thermal);
        }
        let mut power = vec![];
        let supplies = r.entries(&sys.join("class/power_supply"), "power_supply");
        if supplies.len() > 4 {
            r.capped = true;
        }
        for path in supplies.into_iter().take(4) {
            power.push(json!({"supply":path.file_name().unwrap().to_string_lossy(),"type":r.read(&path.join("type"),"power_type").map(|s| s.trim().chars().take(32).collect::<String>()),"online":r.counter(&path.join("online"),"power_online"),"status":r.read(&path.join("status"),"power_status").map(|s| s.trim().chars().take(32).collect::<String>())}));
        }
        Self {
            threads,
            groups,
            settings,
            hardware: json!({"frequency_khz":frequencies,"temperature":temperatures,"power":power}),
            schedstats,
            cgroup,
            cgroup_identity,
            unknown: r.unknown,
            capped: r.capped,
            threads_capped,
            sample_ms: r.started.elapsed().as_secs_f64() * 1000.,
        }
    }
    pub fn record(&self, stage: &str, phase: &str, before: Option<&Self>) -> Value {
        let build_started = Instant::now();
        let mut intervals = serde_json::Map::new();
        if let Some(b) = before {
            for (key, group) in &self.groups {
                let cg_valid = !key.starts_with("cgroup")
                    || (self.cgroup == b.cgroup
                        && self.cgroup_identity.is_some()
                        && self.cgroup_identity == b.cgroup_identity);
                intervals.insert(
                    key.clone(),
                    if cg_valid {
                        b.groups
                            .get(key)
                            .map(|prior| deltas(group, prior))
                            .unwrap_or(Value::Null)
                    } else {
                        Value::Null
                    },
                );
            }
        }
        let placement: Vec<_> = self
            .threads
            .iter()
            .take(16)
            .map(|(tid, t)| json!({"tid":tid,"last_cpu":t.cpu,"allowed_cpus":t.allowed}))
            .collect();
        let wait_available = self.schedstats == Some(1)
            && !self.threads.is_empty()
            && !self.threads_capped
            && self
                .threads
                .values()
                .all(|t| t.counters.contains_key("runqueue_wait_ns"));
        let wait_delta_available = before.map(|b| {
            wait_available
                && b.schedstats == Some(1)
                && !b.threads.is_empty()
                && !b.threads_capped
                && b.threads
                    .values()
                    .all(|t| t.counters.contains_key("runqueue_wait_ns"))
        });
        let mut result = json!({"event":"execution_snapshot","stage":stage.chars().take(64).collect::<String>(),"phase":phase,"process_pid":std::process::id(),"sample_ms":self.sample_ms,
            "sampling_capped":self.capped,"thread_scan_capped":self.threads_capped,"unknown":self.unknown,"sched_schedstats":self.schedstats,
            "schedstat_wait_available":wait_available,"schedstat_wait_delta_available":wait_delta_available,"thread_delta":before.map(|b| thread_delta(&self.threads,&b.threads,self.threads_capped || b.threads_capped)),
            "thread_placement":placement,"placement_capped":self.threads.len()>16,
            "cgroup_path":self.cgroup,"settings":self.settings,"counters":self.groups,"interval_deltas":intervals,"hardware":self.hardware});
        // Leave room for supervisor/job framing in the logger's 16 KiB record budget.
        for field in [
            "hardware",
            "thread_placement",
            "counters",
            "settings",
            "interval_deltas",
            "unknown",
            "cgroup_path",
        ] {
            if result.to_string().len() <= 12_000 {
                break;
            }
            result[field] = Value::Null;
            result["payload_partial"] = true.into();
        }
        result["record_build_ms"] = (build_started.elapsed().as_secs_f64() * 1000.).into();
        result
    }
}

pub fn process_cpu_ns() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // All process threads, with kernel CPU accounting; this is CPU time, not elapsed time.
        if unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) } == 0 {
            return u64::try_from(ts.tv_sec)
                .ok()?
                .checked_mul(1_000_000_000)?
                .checked_add(u64::try_from(ts.tv_nsec).ok()?);
        }
    }
    None
}
/// Capture the end clock before any filesystem reads, serialization or IPC emission.
pub struct CallClock {
    started: Instant,
    cpu: Option<u64>,
}
impl CallClock {
    pub fn start() -> Self {
        let cpu = process_cpu_ns();
        Self {
            started: Instant::now(),
            cpu,
        }
    }
    pub fn finish(self) -> Measurement {
        let wall_ms = self.started.elapsed().as_secs_f64() * 1000.;
        let cpu_ms = process_cpu_ns()
            .zip(self.cpu)
            .and_then(|(a, b)| a.checked_sub(b))
            .map(|n| n as f64 / 1_000_000.);
        Measurement { wall_ms, cpu_ms }
    }
}
pub struct Measurement {
    pub wall_ms: f64,
    cpu_ms: Option<f64>,
}
impl Measurement {
    pub fn attach(&self, record: &mut Value) {
        record["native_wall_ms"] = self.wall_ms.into();
        record["process_cpu_ms_all_threads"] = json!(self.cpu_ms);
        record["effective_parallelism"] = json!(self
            .cpu_ms
            .filter(|_| self.wall_ms > 0.)
            .map(|cpu| cpu / self.wall_ms));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parsers_and_reset_units() {
        assert_eq!(
            psi("some avg10=1.0 total=300\nfull total=12")["some_us"],
            300
        );
        assert!(psi("some total=broken\nweird total=12").is_empty());
        assert_eq!(
            deltas(&numbers("a 8\nb 2"), &numbers("a 3\nb 5\nc 1")),
            json!({"a":5,"b":null,"c":null})
        );
        let mut fields = vec!["0"; 37];
        fields[19] = "123";
        fields[36] = "4";
        let t = thread(
            &format!("1 (weird ) name) {}", fields.join(" ")),
            "Cpus_allowed_list:\t0-7\nvoluntary_ctxt_switches: 2\nnonvoluntary_ctxt_switches: 3",
            "1000000 2000000 4",
            false,
        );
        assert_eq!(t.start, Some(123));
        assert_eq!(t.cpu, Some(4));
        assert_eq!(t.allowed.as_deref(), Some("0-7"));
        assert_eq!(t.counters["runtime_ns"], 1_000_000);
        assert!(!t.counters.contains_key("runqueue_wait_ns"));
    }
    #[test]
    fn late_status_fields_and_host_runqueue() {
        let prefix = (0..100)
            .map(|i| format!("Field{i}: {i}\n"))
            .collect::<String>();
        let status = format!("{prefix}voluntary_ctxt_switches: 11\nnonvoluntary_ctxt_switches: 17");
        let t = thread("bad", &status, "0 0 0", false);
        assert_eq!(t.counters["voluntary_ctxt_switches"], 11);
        assert_eq!(t.counters["nonvoluntary_ctxt_switches"], 17);
        assert_eq!(
            named_number(
                &format!("{prefix}procs_running 9\nprocs_blocked 2"),
                "procs_running"
            ),
            Some(9)
        );
        for path in ["relative", "/../bad", "/x/./y"] {
            assert!(!valid_cgroup(path));
        }
        assert!(valid_cgroup("/user.slice/service"));
    }
    #[test]
    fn payload_is_bounded_even_when_optional_data_is_large() {
        let root = tempfile::tempdir().unwrap();
        let mut a = Snapshot::read_at(root.path(), root.path());
        for i in 0..1000 {
            a.groups
                .insert(format!("group_{i}"), numbers("a 123456789\nb 987654321"));
        }
        let r = a.record("worst", "after", Some(&a));
        assert!(r.to_string().len() <= 12_000);
        assert_eq!(r["payload_partial"], true);
    }
    #[test]
    fn matched_threads_only_and_disabled_wait() {
        let t = |start, runtime, wait| {
            Thread {start:Some(start),counters:numbers(&format!("runtime_ns {runtime}\nrunqueue_wait_ns {wait}\ntimeslices 3\nvoluntary_ctxt_switches 0\nnonvoluntary_ctxt_switches 0")),..Default::default()}
        };
        let before = BTreeMap::from([(1, t(1, 10, 20)), (2, t(2, 20, 30))]);
        let after = BTreeMap::from([(1, t(1, 40, 70)), (2, t(2, 50, 90))]);
        let d = thread_delta(&after, &before, false);
        assert_eq!(d["matched_tid_sums"]["runtime_ns"], 60);
        assert_eq!(d["matched_tid_sums"]["runqueue_wait_ns"], 110);
        assert_eq!(d["partial"], false);
        let churn = BTreeMap::from([(1, t(9, 0, 0)), (2, t(2, 10, 90))]);
        let d = thread_delta(&churn, &before, false);
        assert_eq!(d["matched_tids"], 1);
        assert_eq!(d["counter_reset"], true);
        assert!(d["matched_tid_sums"]["runtime_ns"].is_null());
        assert_eq!(d["partial"], true);
        assert_eq!(thread_delta(&after, &before, true)["partial"], true);
        let t = thread("bad", "", "10 0 0", false);
        let map = BTreeMap::from([(
            1,
            Thread {
                start: Some(1),
                ..t
            },
        )]);
        assert!(thread_delta(&map, &map, false)["matched_tid_sums"]["runqueue_wait_ns"].is_null());
    }
    #[test]
    fn temporary_missing_and_cgroup_migration() {
        let root = tempfile::tempdir().unwrap();
        let a = Snapshot::read_at(root.path(), root.path());
        let r = a.record("test", "after", Some(&a));
        assert!(r["sched_schedstats"].is_null());
        assert_eq!(r["schedstat_wait_available"], false);
        assert!(r["unknown"].as_object().unwrap().contains_key("tasks"));
        fs::create_dir_all(root.path().join("self")).unwrap();
        fs::write(root.path().join("self/cgroup"), "0::/../../escape").unwrap();
        assert!(Snapshot::read_at(root.path(), root.path()).cgroup.is_none());
        let mut b = Snapshot::read_at(root.path(), root.path());
        b.groups
            .insert("cgroup_0_cpu_stat".into(), numbers("throttled_usec 10"));
        b.cgroup = Some("/new".into());
        b.cgroup_identity = Some((1, 2));
        assert!(b.record("x", "after", Some(&a))["interval_deltas"]["cgroup_0_cpu_stat"].is_null());
    }
    #[test]
    fn proc_sys_fixture_reports_units_and_disabled_scheduler() {
        let root = tempfile::tempdir().unwrap();
        let put = |name: &str, text: &str| {
            let p = root.path().join(name);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        };
        let mut fields = vec!["0"; 37];
        fields[19] = "123";
        fields[36] = "2";
        put(
            "self/task/17/stat",
            &format!("17 (thread) {}", fields.join(" ")),
        );
        put(
            "self/task/17/status",
            "Cpus_allowed_list: 0-3\nvoluntary_ctxt_switches: 2\nnonvoluntary_ctxt_switches: 3",
        );
        put("self/task/17/schedstat", "1000000 0 0");
        put("sys/kernel/sched_schedstats", "0");
        put(
            "stat",
            "cpu  1 2 3 4 5 6 7 8 9 10\nprocs_running 2\nprocs_blocked 1",
        );
        put("loadavg", "0.1 0.2 0.3 2/100 99");
        put("pressure/cpu", "some total=100");
        put("self/cgroup", "0::/service");
        put("fs/cgroup/service/cpu.max", "200000 100000");
        put(
            "fs/cgroup/service/cpu.stat",
            "nr_throttled 2\nthrottled_usec 10",
        );
        put("fs/cgroup/cpu.max", "max 100000");
        put("fs/cgroup/cpu.stat", "nr_throttled 0");
        put("class/thermal/thermal_zone0/temp", "-1000");
        put("class/thermal/thermal_zone0/type", "test");
        let a = Snapshot::read_at(root.path(), root.path());
        put("self/task/17/schedstat", "3000000 0 0");
        put("pressure/cpu", "some total=350");
        put(
            "fs/cgroup/service/cpu.stat",
            "nr_throttled 3\nthrottled_usec 20",
        );
        let b = Snapshot::read_at(root.path(), root.path());
        let r = b.record("fixture", "after", Some(&a));
        assert_eq!(
            r["thread_delta"]["matched_tid_sums"]["runtime_ns"],
            2_000_000
        );
        assert!(r["thread_delta"]["matched_tid_sums"]["runqueue_wait_ns"].is_null());
        assert_eq!(r["interval_deltas"]["host_cpu_pressure"]["some_us"], 250);
        assert_eq!(
            r["interval_deltas"]["cgroup_0_cpu_stat"]["throttled_usec"],
            10
        );
        assert_eq!(r["counters"]["host_cpu"]["busy_ticks"], 19);
        assert_eq!(r["settings"]["procs_running"], 2);
        assert_eq!(r["hardware"]["temperature"][0]["millidegrees_c"], -1000);
    }
    #[test]
    fn enabled_but_missing_schedstat_is_unavailable() {
        let root = tempfile::tempdir().unwrap();
        let mut a = Snapshot::read_at(root.path(), root.path());
        a.schedstats = Some(1);
        a.threads_capped = false;
        let t = Thread {
            start: Some(1),
            counters: numbers("voluntary_ctxt_switches 10"),
            ..Default::default()
        };
        a.threads.insert(1, t);
        let r = a.record("missing_schedstat", "after", Some(&a));
        assert_eq!(r["sched_schedstats"], 1);
        assert_eq!(r["schedstat_wait_available"], false);
        assert_eq!(r["schedstat_wait_delta_available"], false);
        assert!(r["thread_delta"]["matched_tid_sums"]["runqueue_wait_ns"].is_null());
    }
    #[test]
    fn bounded_reads_and_measurement_excludes_sampling() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("large");
        fs::write(&path, vec![b'x'; MAX_FILE + 1]).unwrap();
        let mut r = Reader::new();
        assert!(r.read(&path, "large").is_none());
        assert!(r.capped);
        let clock = CallClock::start();
        let m = clock.finish();
        std::thread::sleep(Duration::from_millis(30));
        let mut record = json!({});
        m.attach(&mut record);
        assert!(record["native_wall_ms"].as_f64().unwrap() < 30.);
        assert!(record["process_cpu_ms_all_threads"].as_f64().unwrap() >= 0.);
    }
}
