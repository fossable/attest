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

/// A cgroup directory created for a single test. The forked child calls
/// `enter()` to place itself inside it; the parent reads stats after the child
/// exits and the cgroup is cleaned up on drop.
pub struct TestCgroup {
    path: PathBuf,
}

impl TestCgroup {
    /// Attempt to create a per-test cgroup directory. Returns `None` when
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

        Some(Self { path })
    }

    /// Place the current process into this cgroup. Call this from the forked
    /// child immediately after fork, before running the test.
    pub fn enter(&self) {
        let pid = std::process::id().to_string();
        if let Err(e) = std::fs::write(self.path.join("cgroup.procs"), &pid) {
            tracing::debug!("failed to enter test cgroup: {e}");
        }
    }

    /// Read resource stats from the cgroup pseudo-files. Call this after the
    /// child has exited (waitpid returned) but before dropping the handle.
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

fn read_single_u64(path: impl AsRef<std::path::Path>) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Parse a `key value` line from a stat file (e.g. `cpu.stat`).
fn read_stat_field(path: impl AsRef<std::path::Path>, field: &str) -> Option<u64> {
    let content = std::fs::read_to_string(path).ok()?;
    content.lines().find_map(|line| {
        let (k, v) = line.split_once(' ')?;
        if k == field {
            v.trim().parse().ok()
        } else {
            None
        }
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
