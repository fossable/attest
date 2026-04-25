use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);
static SETUP_OK: OnceLock<bool> = OnceLock::new();

const ATTEST_CGROUP: &str = "/sys/fs/cgroup/attest";

/// Resource usage captured from cgroup v2 for a single test run.
/// Fields are `None` when the corresponding controller is unavailable.
#[derive(Debug, Clone, Default)]
pub struct ResourceStats {
    pub cpu_user_usec: Option<u64>,
    pub cpu_system_usec: Option<u64>,
    pub memory_peak: Option<u64>,
    pub io_read_bytes: Option<u64>,
    pub io_write_bytes: Option<u64>,
    pub pids_peak: Option<u64>,
}

/// A domain cgroup created for a single test. The whole process is moved into
/// it on creation and restored to the original cgroup on drop.
pub struct TestCgroup {
    path: PathBuf,
    original_procs: PathBuf,
    pid: String,
}

impl TestCgroup {
    /// Attempt to create and enter a per-test cgroup. Returns `None` when
    /// cgroups are unavailable or the process lacks permission.
    pub fn try_create(test_id: &str) -> Option<Self> {
        let ok = *SETUP_OK.get_or_init(|| match setup_parent_cgroup() {
            Ok(()) => true,
            Err(e) => {
                tracing::debug!("cgroup setup failed, resource tracking disabled: {e}");
                false
            }
        });
        if !ok {
            return None;
        }

        let original_cgroup = read_self_cgroup_path()?;
        let original_procs = PathBuf::from(format!(
            "/sys/fs/cgroup{original_cgroup}/cgroup.procs"
        ));
        if !original_procs.parent().map(|p| p.exists()).unwrap_or(false) {
            return None;
        }

        let safe_id: String = test_id
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir_name = format!("{safe_id}_{count}");

        let path = PathBuf::from(ATTEST_CGROUP).join(&dir_name);

        if let Err(e) = std::fs::create_dir(&path) {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                // Leftover from a previous run – try to reuse after cleanup
                let _ = std::fs::remove_dir(&path);
                if let Err(e2) = std::fs::create_dir(&path) {
                    tracing::debug!("failed to create test cgroup {dir_name}: {e2}");
                    return None;
                }
            } else {
                tracing::debug!("failed to create test cgroup {dir_name}: {e}");
                return None;
            }
        }

        let pid = std::process::id().to_string();
        if let Err(e) = std::fs::write(path.join("cgroup.procs"), &pid) {
            tracing::debug!("failed to enter test cgroup: {e}");
            let _ = std::fs::remove_dir(&path);
            return None;
        }

        Some(Self { path, original_procs, pid })
    }

    /// Read resource stats from the cgroup pseudo-files. Call this before
    /// dropping the cgroup handle (i.e., while the cgroup is still active).
    pub fn read_stats(&self) -> ResourceStats {
        ResourceStats {
            cpu_user_usec: read_stat_field(self.path.join("cpu.stat"), "user_usec"),
            cpu_system_usec: read_stat_field(self.path.join("cpu.stat"), "system_usec"),
            memory_peak: read_single_u64(self.path.join("memory.peak")).or_else(|| {
                tracing::debug!("memory.peak unavailable, falling back to memory.current");
                read_single_u64(self.path.join("memory.current"))
            }),
            io_read_bytes: read_io_field(&self.path, "rbytes"),
            io_write_bytes: read_io_field(&self.path, "wbytes"),
            pids_peak: read_single_u64(self.path.join("pids.peak")),
        }
    }
}

impl Drop for TestCgroup {
    fn drop(&mut self) {
        // Restore process to its original cgroup before removing the test cgroup.
        if let Err(e) = std::fs::write(&self.original_procs, &self.pid) {
            tracing::warn!("failed to restore process to original cgroup: {e}");
        }
        if let Err(e) = std::fs::remove_dir(&self.path) {
            tracing::warn!("failed to remove test cgroup {:?}: {e}", self.path);
        }
    }
}

/// Create the shared `/sys/fs/cgroup/attest/` parent cgroup and enable
/// resource controllers. Called at most once per process.
fn setup_parent_cgroup() -> anyhow::Result<()> {
    std::fs::create_dir_all(ATTEST_CGROUP)?;
    // Enable controllers one at a time; ignore individual failures for
    // controllers not available in this environment.
    for ctrl in ["cpu", "memory", "io", "pids"] {
        let _ = std::fs::write(
            format!("{ATTEST_CGROUP}/cgroup.subtree_control"),
            format!("+{ctrl}"),
        );
    }
    Ok(())
}

/// Read the cgroup v2 path for the current process from `/proc/self/cgroup`.
fn read_self_cgroup_path() -> Option<String> {
    let content = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    // cgroup v2 entries have the form "0::<path>"
    let path = content
        .lines()
        .find(|l| l.starts_with("0::"))?
        .strip_prefix("0::")?
        .trim()
        .to_string();
    Some(path)
}

fn read_single_u64(path: impl AsRef<std::path::Path>) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Parse a `key value` line from a stat file (e.g. `cpu.stat`).
fn read_stat_field(path: impl AsRef<std::path::Path>, field: &str) -> Option<u64> {
    let content = std::fs::read_to_string(path).ok()?;
    content.lines().find_map(|line| {
        let (k, v) = line.split_once(' ')?;
        if k == field { v.trim().parse().ok() } else { None }
    })
}

/// Sum a named field (e.g. `rbytes`) across all device lines in `io.stat`.
/// Returns `None` when the file is absent or the total is zero.
fn read_io_field(cgroup_path: &PathBuf, field: &str) -> Option<u64> {
    let content = std::fs::read_to_string(cgroup_path.join("io.stat")).ok()?;
    let prefix = format!("{field}=");
    let total: u64 = content
        .lines()
        .flat_map(|line| line.split_whitespace())
        .filter_map(|tok| tok.strip_prefix(prefix.as_str()))
        .filter_map(|v| v.parse::<u64>().ok())
        .sum();
    if total > 0 { Some(total) } else { None }
}
