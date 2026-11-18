//! Build parallelism from the container's real limits, not the host's CPU count.
//!
//! Every build tool already reads one of a handful of standard variables, and
//! the rest ask `nproc`, which GNU coreutils answers from `OMP_NUM_THREADS`.
//! Deciding those once from the cgroup means no per-tool list ever grows.
//!
//! The same limits, with the disk the home and the Nix store have left, are
//! what `state info` says the computer has ([`resources`]), so an agent plans
//! to them rather than finding them out.
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

/// Memory one parallel compile job is budgeted: rustc and C++ units peak
/// near this, so four jobs fit a 4 GiB desktop without thrashing.
const BYTES_PER_JOB: u64 = 1 << 30;

fn read(root: &Path, file: &str) -> Option<String> {
    std::fs::read_to_string(root.join(file))
        .ok()
        .map(|s| s.trim().to_owned())
}

/// CPUs the cgroup quota allows, bounded by the CPUs this process may use.
fn cpus(root: &Path, available: u64) -> u64 {
    let quota = read(root, "cpu.max").and_then(|line| {
        let mut parts = line.split_whitespace();
        let quota: u64 = parts.next()?.parse().ok()?;
        let period: u64 = parts.next()?.parse().ok()?;
        (period > 0).then(|| quota.div_ceil(period))
    });
    quota.map_or(available, |q| q.min(available)).max(1)
}

fn memory(root: &Path) -> Option<u64> {
    read(root, "memory.max")?.parse().ok()
}

/// How many jobs a build may run at once here, and how many CPUs there are.
pub fn parallelism(root: &Path, available: u64) -> (u64, u64) {
    let cpus = cpus(root, available);
    let jobs = memory(root).map_or(cpus, |bytes| (bytes / BYTES_PER_JOB).clamp(1, cpus));
    (jobs, cpus)
}

/// The variables every job starts with; a workspace or the job itself may override them.
pub fn environment() -> BTreeMap<String, String> {
    let available = std::thread::available_parallelism().map_or(1, |n| n.get() as u64);
    let (jobs, cpus) = parallelism(Path::new("/sys/fs/cgroup"), available);
    variables(jobs, cpus)
}

fn variables(jobs: u64, cpus: u64) -> BTreeMap<String, String> {
    [
        ("CARGO_BUILD_JOBS", jobs.to_string()),
        ("MAKEFLAGS", format!("-j{jobs}")),
        ("NIX_BUILD_CORES", jobs.to_string()),
        ("CMAKE_BUILD_PARALLEL_LEVEL", jobs.to_string()),
        // GNU nproc reports this, so tools that size themselves by nproc follow.
        ("OMP_NUM_THREADS", jobs.to_string()),
        ("GOMAXPROCS", cpus.to_string()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value))
    .collect()
}

/// A number from `/proc/meminfo`, in bytes.
fn meminfo(text: &str, key: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix(':'))
        .and_then(|rest| rest.split_whitespace().next()?.parse::<u64>().ok())
        .map(|kib| kib * 1024)
}

fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / f64::from(1u32 << 30))
}

/// Memory: the cgroup's limit and use when it sets one, the machine's otherwise.
fn memory_report(root: &Path, meminfo_text: &str) -> Value {
    let (limit, source) = match memory(root) {
        Some(bytes) => (Some(bytes), "cgroup"),
        None => (meminfo(meminfo_text, "MemTotal"), "machine"),
    };
    let used = match source {
        "cgroup" => read(root, "memory.current").and_then(|v| v.parse::<u64>().ok()),
        _ => meminfo(meminfo_text, "MemTotal")
            .zip(meminfo(meminfo_text, "MemAvailable"))
            .map(|(total, available)| total.saturating_sub(available)),
    };
    let events = read(root, "memory.events").unwrap_or_default();
    let count = |key: &str| {
        events.lines().find_map(|line| {
            let (name, value) = line.split_once(' ')?;
            (name == key).then(|| value.trim().parse::<u64>().ok())?
        })
    };
    json!({
        "limit_bytes": limit,
        "used_bytes": used,
        "peak_bytes": read(root, "memory.peak").and_then(|v| v.parse::<u64>().ok()),
        "limit_from": source,
        "oom_kills": count("oom_kill"),
    })
}

/// A file system's size and what is left of it, for whoever is not root.
fn disk(path: &Path) -> Option<(u64, u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &raw mut stat) } != 0 {
        return None;
    }
    let block = stat.f_frsize as u64;
    let total = stat.f_blocks as u64 * block;
    let free = stat.f_bavail as u64 * block;
    let used = total.saturating_sub(stat.f_bfree as u64 * block);
    Some((total, free, used))
}

/// What this computer has to work with: CPUs, memory, the disk under the
/// home and under the Nix store, and how it draws. Nothing here is asked of
/// anything but the kernel.
pub fn resources(home: &Path) -> Value {
    resources_from(
        Path::new("/sys/fs/cgroup"),
        &std::fs::read_to_string("/proc/meminfo").unwrap_or_default(),
        home,
        Path::new("/nix/store"),
    )
}

fn resources_from(root: &Path, meminfo_text: &str, home: &Path, store: &Path) -> Value {
    use std::os::unix::fs::MetadataExt;
    let available = std::thread::available_parallelism().map_or(1, |n| n.get() as u64);
    let (jobs, cpus) = parallelism(root, available);
    let memory = memory_report(root, meminfo_text);
    let device = |path: &Path| std::fs::metadata(path).ok().map(|m| m.dev());
    let shared = device(home).is_some() && device(home) == device(store);
    let mut disks = serde_json::Map::new();
    let mut said = Vec::new();
    for (name, path) in [("home", home), ("nix_store", store)] {
        let Some((total, free, used)) = disk(path) else {
            continue;
        };
        disks.insert(
            name.into(),
            json!({"path": path, "total_bytes": total, "free_bytes": free, "used_bytes": used}),
        );
        if name == "home" {
            said.push(format!("home {} free of {}", gib(free), gib(total)));
        } else if shared {
            said.push("the Nix store on the same disk".to_owned());
        } else {
            said.push(format!("Nix store {} free of {}", gib(free), gib(total)));
        }
    }
    if let Some(shared_disk) = disks.get_mut("nix_store") {
        shared_disk["same_disk_as_home"] = json!(shared);
    }
    let memory_said = match (
        memory["limit_bytes"].as_u64(),
        memory["used_bytes"].as_u64(),
    ) {
        (Some(limit), Some(used)) => format!("{} memory, {} in use", gib(limit), gib(used)),
        (Some(limit), None) => format!("{} memory", gib(limit)),
        _ => "memory unknown".to_owned(),
    };
    json!({
        "cpus": cpus,
        "build_jobs": jobs,
        "memory": memory,
        "disk": disks,
        "graphics": {
            "rendering": "software",
            "renderer": "Mesa llvmpipe on the CPU",
            "gpu": false,
            "note": "There is no GPU: OpenGL and Vulkan are drawn by the CPU, and CUDA, OpenCL on a GPU and other GPU compute are absent. A heavy real-time 3D viewport is slow; offline CPU renders work.",
        },
        "summary": format!(
            "{cpus} CPUs ({jobs} build jobs at once); {memory_said}; {}; software rendering, no GPU",
            said.join(", ")
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cgroup(cpu: &str, memory: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("cpu.max"), cpu).unwrap();
        std::fs::write(root.path().join("memory.max"), memory).unwrap();
        root
    }

    #[test]
    fn memory_bounds_jobs_even_when_the_host_reports_many_cpus() {
        let root = cgroup("max 100000\n", "4294967296\n");
        assert_eq!(parallelism(root.path(), 24), (4, 24));
    }

    #[test]
    fn a_cpu_quota_bounds_both_and_unlimited_memory_follows_cpus() {
        let root = cgroup("200000 100000\n", "max\n");
        assert_eq!(parallelism(root.path(), 24), (2, 2));
        let root = cgroup("150000 100000\n", "536870912\n");
        assert_eq!(parallelism(root.path(), 8), (1, 2));
    }

    #[test]
    fn without_a_cgroup_the_available_cpus_decide() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(parallelism(root.path(), 6), (6, 6));
        let env = variables(4, 24);
        assert_eq!(env["MAKEFLAGS"], "-j4");
        assert_eq!(env["GOMAXPROCS"], "24");
    }

    #[test]
    fn resources_say_the_cgroups_limits_and_the_disk_left() {
        let root = cgroup("200000 100000\n", "4294967296\n");
        std::fs::write(root.path().join("memory.current"), "1073741824\n").unwrap();
        std::fs::write(
            root.path().join("memory.events"),
            "low 0\nhigh 0\nmax 2\noom 1\noom_kill 1\n",
        )
        .unwrap();
        let home = tempfile::tempdir().unwrap();
        let value = resources_from(root.path(), "", home.path(), home.path());
        assert_eq!(value["cpus"], 2);
        assert_eq!(value["memory"]["limit_bytes"], 4_294_967_296_u64);
        assert_eq!(value["memory"]["used_bytes"], 1_073_741_824_u64);
        assert_eq!(value["memory"]["limit_from"], "cgroup");
        assert_eq!(value["memory"]["oom_kills"], 1);
        let disk = &value["disk"]["home"];
        assert!(disk["total_bytes"].as_u64().unwrap() > 0);
        assert!(disk["free_bytes"].as_u64().unwrap() <= disk["total_bytes"].as_u64().unwrap());
        assert_eq!(value["disk"]["nix_store"]["same_disk_as_home"], true);
        assert_eq!(value["graphics"]["gpu"], false);
        let summary = value["summary"].as_str().unwrap();
        assert!(summary.starts_with("2 CPUs"), "{summary}");
        assert!(
            summary.contains("4.0 GiB memory, 1.0 GiB in use"),
            "{summary}"
        );
        assert!(summary.contains("Nix store on the same disk"), "{summary}");
    }

    #[test]
    fn without_a_memory_limit_the_machines_memory_is_said() {
        let root = tempfile::tempdir().unwrap();
        let meminfo =
            "MemTotal:        8000000 kB\nMemFree:  100 kB\nMemAvailable:    6000000 kB\n";
        let value = memory_report(root.path(), meminfo);
        assert_eq!(value["limit_bytes"], 8_000_000_u64 * 1024);
        assert_eq!(value["used_bytes"], 2_000_000_u64 * 1024);
        assert_eq!(value["limit_from"], "machine");
        assert!(value["oom_kills"].is_null());
        let missing = tempfile::tempdir().unwrap();
        let value = resources_from(
            root.path(),
            "",
            missing.path(),
            &missing.path().join("none"),
        );
        assert!(value["disk"].get("nix_store").is_none());
        assert!(
            value["summary"]
                .as_str()
                .unwrap()
                .contains("memory unknown")
        );
    }
}
