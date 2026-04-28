use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);
/// Resolved once per process: the `/sys/fs/cgroup/.../attest` directory that
/// belongs to this user. `None` if cgroups are unavailable or unwritable.
static ATTEST_BASE: OnceLock<Option<PathBuf>> = OnceLock::new();

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
        let base = ATTEST_BASE.get_or_init(init_attest_base).as_ref()?;

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
        let path = base.join(&dir_name);

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

    /// Read total CPU time (user + system) from the cgroup. Returns `None`
    /// when the cpu controller is unavailable.
    pub fn read_cpu_time(&self) -> Option<std::time::Duration> {
        let user = read_stat_field(self.path.join("cpu.stat"), "user_usec")?;
        let system = read_stat_field(self.path.join("cpu.stat"), "system_usec").unwrap_or(0);
        Some(std::time::Duration::from_micros(user + system))
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

/// Find the nearest ancestor cgroup where `cgroup.procs` works, create the
/// `attest` directory there, and enable resource controllers.
///
/// Rather than guessing from `cgroup.type` (whose semantics around
/// "domain threaded" vary across kernel versions), we fork a throw-away probe
/// child that actually attempts the write. EOPNOTSUPP at one level means we
/// walk up and try the parent.
fn init_attest_base() -> Option<PathBuf> {
    let cg_content = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let rel = cg_content
        .lines()
        .find(|l| l.starts_with("0::"))?
        .strip_prefix("0::")?
        .trim()
        .to_string();

    let mut ancestor = PathBuf::from("/sys/fs/cgroup").join(rel.trim_start_matches('/'));

    loop {
        let base = ancestor.join("attest");

        // Create base dir; tolerate AlreadyExists from prior runs.
        match std::fs::create_dir(&base) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                tracing::debug!("cannot create {}: {e}", base.display());
                match ancestor.parent() {
                    Some(p) if p != Path::new("/sys/fs/cgroup") => {
                        ancestor = p.to_path_buf();
                        continue;
                    }
                    _ => break,
                }
            }
        }

        // Probe: create a temporary child cgroup and fork a process that
        // writes its own PID to cgroup.procs, exiting 0 on success and 1
        // on EOPNOTSUPP or any other error.
        let probe = base.join("_probe");
        let _ = std::fs::remove_dir(&probe); // clean up from a crashed prior run
        let probe_ok = if std::fs::create_dir(&probe).is_ok() {
            let result = probe_cgroup_procs(&probe);
            let _ = std::fs::remove_dir(&probe); // child exited, cgroup is empty
            result
        } else {
            false
        };

        if probe_ok {
            for ctrl in ["cpu", "memory", "io", "pids"] {
                let _ = std::fs::write(ancestor.join("cgroup.subtree_control"), format!("+{ctrl}"));
                let _ = std::fs::write(base.join("cgroup.subtree_control"), format!("+{ctrl}"));
            }
            tracing::debug!("cgroup base: {}", base.display());
            return Some(base);
        }

        tracing::debug!(
            "cgroup.procs probe failed at {}; trying parent",
            base.display()
        );
        let _ = std::fs::remove_dir(&base); // may fail if non-empty, that's ok
        match ancestor.parent() {
            Some(p) if p != Path::new("/sys/fs/cgroup") => ancestor = p.to_path_buf(),
            _ => break,
        }
    }

    tracing::debug!("cgroup setup failed: no suitable cgroup found in hierarchy");
    None
}

/// Fork a child that writes its own PID to `dir/cgroup.procs` and exits 0 on
/// success or 1 on failure. Returns true if the child exited with 0.
fn probe_cgroup_procs(dir: &Path) -> bool {
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return false;
    }
    if pid == 0 {
        // Child process.
        let my_pid = std::process::id().to_string();
        let ok = std::fs::write(dir.join("cgroup.procs"), my_pid).is_ok();
        unsafe { libc::_exit(if ok { 0 } else { 1 }) };
    }
    // Parent: wait for probe child.
    let mut status = 0;
    unsafe { libc::waitpid(pid, &mut status, 0) };
    libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0
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
fn read_io_field(cgroup_path: &Path, field: &str) -> Option<u64> {
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
