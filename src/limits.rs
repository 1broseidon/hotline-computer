//! Build parallelism from the container's real limits, not the host's CPU count.
//!
//! Every build tool already reads one of a handful of standard variables, and
//! the rest ask `nproc`, which GNU coreutils answers from `OMP_NUM_THREADS`.
//! Deciding those once from the cgroup means no per-tool list ever grows.
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
}
