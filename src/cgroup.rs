use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use tracing::{debug, trace, warn};

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
        let base = ATTEST_BASE.get_or_init(init_base).as_ref()?;

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
        let path = base.join(format!("{safe_id}_{count}"));

        if let Err(e) = std::fs::create_dir(&path) {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                let _ = std::fs::remove_dir(&path);
                if let Err(e2) = std::fs::create_dir(&path) {
                    trace!("failed to create test cgroup: {e2}");
                    return None;
                }
            } else {
                trace!("failed to create test cgroup: {e}");
                return None;
            }
        }

        Some(Self { path })
    }

    /// Path of the `cgroup.procs` file the child writes its pid into to join
    /// this cgroup. Used from `Command::pre_exec` after fork, before exec.
    pub fn procs_path(&self) -> PathBuf {
        self.path.join("cgroup.procs")
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
                trace!("memory.peak unavailable, falling back to memory.current");
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
            warn!("failed to remove test cgroup {:?}: {e}", self.path);
        }
    }
}

fn cgroup_type(path: &Path) -> String {
    std::fs::read_to_string(path.join("cgroup.type"))
        .unwrap_or_else(|_| "domain".to_string())
        .trim()
        .to_string()
}

fn is_domain(path: &Path) -> bool {
    cgroup_type(path) == "domain"
}

/// Ensure `path` is a usable "domain" cgroup, creating or recovering it as needed.
/// Returns false if the cgroup cannot be made usable.
fn ensure_domain_cgroup(path: &Path) -> bool {
    match std::fs::create_dir(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if !is_domain(path) {
                debug!(path=%path.display(), "stale cgroup (type='{}'): purging and recreating", cgroup_type(path));
                purge_cgroup_children(path);
                let _ = std::fs::remove_dir(path);
                if let Err(e2) = std::fs::create_dir(path) {
                    debug!(path=%path.display(), "recreate failed: {e2}");
                    return false;
                }
            }
        }
        Err(e) => {
            debug!(path=%path.display(), "create failed: {e}");
            return false;
        }
    }
    if !is_domain(path) {
        debug!(path=%path.display(), "newly created cgroup has unexpected type '{}'", cgroup_type(path));
        let _ = std::fs::remove_dir(path);
        return false;
    }
    true
}

/// Remove all empty child cgroup directories under `dir` (best-effort).
fn purge_cgroup_children(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            purge_cgroup_children(&p);
            let _ = std::fs::remove_dir(&p);
        }
    }
}

/// Move `ancestor` up one level within `/sys/fs/cgroup`. Returns false when
/// already at the cgroup root (no useful parent remains).
fn try_parent(ancestor: &mut PathBuf) -> bool {
    match ancestor.parent() {
        Some(p) if p.starts_with("/sys/fs/cgroup") && p != Path::new("/sys/fs/cgroup") => {
            *ancestor = p.to_path_buf();
            true
        }
        _ => false,
    }
}

fn init_base() -> Option<PathBuf> {
    let cg_content = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let rel = cg_content
        .lines()
        .find(|l| l.starts_with("0::"))?
        .strip_prefix("0::")?
        .trim()
        .to_string();

    let mut ancestor = PathBuf::from("/sys/fs/cgroup").join(rel.trim_start_matches('/'));

    loop {
        // Only "domain" cgroups can parent child cgroups that hold processes.
        // "domain threaded" and "domain invalid" ancestors yield unusable children.
        if !is_domain(&ancestor) {
            debug!(path=%ancestor.display(), "ancestor type is '{}'; skipping", cgroup_type(&ancestor));
            if !try_parent(&mut ancestor) {
                break;
            }
            continue;
        }

        let base = ancestor.join("attest");
        if !ensure_domain_cgroup(&base) {
            if !try_parent(&mut ancestor) {
                break;
            }
            continue;
        }

        // cgroup v2 no-internal-process constraint: a non-root cgroup that has
        // child cgroups cannot directly contain processes. Move the current process
        // into base/main (a leaf) so that any child we fork also starts in a leaf
        // and can freely migrate to a sibling test cgroup via cgroup.procs.
        let main_cgroup = base.join("main");
        if !ensure_domain_cgroup(&main_cgroup) {
            let _ = std::fs::remove_dir(&base);
            if !try_parent(&mut ancestor) {
                break;
            }
            continue;
        }

        let current_pid = std::process::id().to_string();
        if let Err(e) = std::fs::write(main_cgroup.join("cgroup.procs"), &current_pid) {
            if e.raw_os_error() == Some(libc::EOPNOTSUPP) {
                // The cgroup is in a threaded subtree; cgroup.procs is not valid
                // anywhere in this hierarchy. No point walking up.
                debug!(
                    "cgroup.procs not supported (type: '{}')",
                    cgroup_type(&main_cgroup)
                );
                let _ = std::fs::remove_dir(&main_cgroup);
                let _ = std::fs::remove_dir(&base);
                break;
            }
            debug!(path=%main_cgroup.display(), "failed to enter main cgroup: {e}");
            let _ = std::fs::remove_dir(&main_cgroup);
            let _ = std::fs::remove_dir(&base);
            if !try_parent(&mut ancestor) {
                break;
            }
            continue;
        }

        // Probe: fork a child that inherits base/main (a leaf) and verify it can
        // migrate to a sibling cgroup by writing to its cgroup.procs.
        let probe = base.join("_probe");
        let _ = std::fs::remove_dir(&probe); // clean up from a crashed prior run
        let probe_ok = std::fs::create_dir(&probe).is_ok() && {
            let result = probe_cgroup_procs(&probe);
            let _ = std::fs::remove_dir(&probe);
            result
        };

        if probe_ok {
            // Enable only the controllers already delegated to base by its parent
            // (visible in base/cgroup.controllers). Never write to
            // ancestor/cgroup.subtree_control — doing so while the ancestor has live
            // processes transitions it to "domain invalid" state on subsequent runs.
            let available =
                std::fs::read_to_string(base.join("cgroup.controllers")).unwrap_or_default();
            for ctrl in ["cpu", "memory", "io", "pids"] {
                if available.split_whitespace().any(|c| c == ctrl) {
                    let _ = std::fs::write(base.join("cgroup.subtree_control"), format!("+{ctrl}"));
                }
            }
            debug!(path=%base.display(), "selected cgroup base");
            return Some(base);
        }

        // Probe failed; restore the current process to the ancestor and walk up.
        let _ = std::fs::write(ancestor.join("cgroup.procs"), &current_pid);
        let _ = std::fs::remove_dir(&main_cgroup);
        let _ = std::fs::remove_dir(&base);
        debug!(path=%base.display(), "cgroup.procs probe failed; trying parent");
        if !try_parent(&mut ancestor) {
            break;
        }
    }

    debug!("no suitable cgroup found in hierarchy");
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
        let my_pid = std::process::id().to_string();
        match std::fs::write(dir.join("cgroup.procs"), &my_pid) {
            Ok(()) => unsafe { libc::_exit(0) },
            Err(e) => {
                let msg = format!(
                    "cgroup probe: write to {}/cgroup.procs failed: {e}\n",
                    dir.display()
                );
                unsafe {
                    libc::write(
                        2,
                        msg.as_ptr() as *const libc::c_void,
                        msg.len() as libc::size_t,
                    );
                    libc::_exit(1)
                }
            }
        }
    }
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
