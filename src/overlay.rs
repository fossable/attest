//! Per-test overlayfs isolation.
//!
//! Each test runs with its working directory set to the merged view of an
//! overlay mount: the lower (read-only) layer is attest's invocation directory,
//! so tests see the real project files, while writes land in a per-test upper
//! layer and never touch the real tree. The mount is performed inside the
//! forked child in a private mount namespace, so it is torn down automatically
//! when the child exits (even on kill/timeout) and never pollutes the host.
//!
//! Mounting overlayfs needs `CAP_SYS_ADMIN`. When attest already has it
//! ([`Mode::Privileged`]) we just unshare a mount namespace. Otherwise we
//! unshare a user namespace too ([`Mode::Userns`]), which grants `CAP_SYS_ADMIN`
//! within it — at the cost of the test seeing itself as uid 0 inside that
//! namespace. If neither works the caller falls back to running without an overlay.

use std::ffi::{CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

/// How a per-test overlay can be mounted in this environment.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Have `CAP_SYS_ADMIN`: unshare only a mount namespace (test keeps its uid).
    Privileged,
    /// Unprivileged: unshare a user + mount namespace and map the caller to uid 0
    /// inside it (the test runs as root within its own namespace).
    Userns,
}

/// Convert a path to a `CString` for use with libc mount/chdir calls.
fn cstr(path: &Path) -> Option<CString> {
    CString::new(path.as_os_str().as_bytes()).ok()
}

/// Build the overlay mount option string `lowerdir=...,upperdir=...,workdir=...`.
fn mount_options(lower: &Path, upper: &Path, work: &Path) -> Option<CString> {
    let opts = format!(
        "lowerdir={},upperdir={},workdir={}",
        lower.display(),
        upper.display(),
        work.display()
    );
    CString::new(opts).ok()
}

/// Write `data` to `path` via raw libc (async-signal-safe; no allocation).
unsafe fn write_proc(path: &CStr, data: &[u8]) -> bool {
    unsafe {
        let fd = libc::open(path.as_ptr(), libc::O_WRONLY);
        if fd < 0 {
            return false;
        }
        let n = libc::write(fd, data.as_ptr().cast(), data.len());
        libc::close(fd);
        n == data.len() as isize
    }
}

/// Enter the namespace(s) required for `mode`. Returns `true` on success. Only
/// async-signal-safe libc calls are used so this is safe after `fork`/in `pre_exec`.
unsafe fn enter_namespace(mode: Mode, uid_map: &CStr, gid_map: &CStr) -> bool {
    unsafe {
        match mode {
            Mode::Privileged => libc::unshare(libc::CLONE_NEWNS) == 0,
            Mode::Userns => {
                if libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNS) != 0 {
                    return false;
                }
                // setgroups must be denied before writing gid_map (kernel rule);
                // the file is absent on some kernels, so ignore its result.
                write_proc(c"/proc/self/setgroups", b"deny");
                write_proc(c"/proc/self/uid_map", uid_map.to_bytes())
                    && write_proc(c"/proc/self/gid_map", gid_map.to_bytes())
            }
        }
    }
}

/// Make mounts private (so the overlay never propagates to the host) and mount
/// the overlay at `merged`. Returns `true` on success.
unsafe fn mount_overlay(merged: &CStr, opts: &CStr) -> bool {
    unsafe {
        if libc::mount(
            std::ptr::null(),
            c"/".as_ptr(),
            std::ptr::null(),
            libc::MS_REC | libc::MS_PRIVATE,
            std::ptr::null(),
        ) != 0
        {
            return false;
        }
        libc::mount(
            c"overlay".as_ptr(),
            merged.as_ptr(),
            c"overlay".as_ptr(),
            0,
            opts.as_ptr().cast(),
        ) == 0
    }
}

/// uid/gid map contents (`0 <id> 1`) that map the caller to root inside a new
/// user namespace. Built in the parent so the `pre_exec` closure never allocates.
fn id_maps() -> (CString, CString) {
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    (
        CString::new(format!("0 {uid} 1")).unwrap(),
        CString::new(format!("0 {gid} 1")).unwrap(),
    )
}

/// Register a `pre_exec` hook on `cmd` that mounts a per-test overlay (using
/// `mode`) and `chdir`s into the merged view. Best-effort: if setup fails
/// unexpectedly the child keeps its inherited working directory (test still runs).
pub fn register_mount(
    cmd: &mut Command,
    mode: Mode,
    lower: &Path,
    upper: &Path,
    work: &Path,
    merged: &Path,
) {
    let (Some(opts), Some(merged_c)) = (mount_options(lower, upper, work), cstr(merged)) else {
        return;
    };
    let (uid_map, gid_map) = id_maps();

    unsafe {
        cmd.pre_exec(move || {
            if enter_namespace(mode, &uid_map, &gid_map) && mount_overlay(&merged_c, &opts) {
                libc::chdir(merged_c.as_ptr());
            }
            Ok(())
        });
    }
}

/// Attempt a throwaway overlay mount with `mode` under `parent`, in a forked
/// child so the caller's namespaces are never touched. Returns whether it worked.
fn probe_mode(parent: &Path, mode: Mode, uid_map: &CStr, gid_map: &CStr) -> bool {
    let base = parent.join(match mode {
        Mode::Privileged => ".attest-ovl-priv",
        Mode::Userns => ".attest-ovl-userns",
    });
    let _ = std::fs::remove_dir_all(&base);
    for sub in ["lower", "upper", "work", "merged"] {
        if std::fs::create_dir_all(base.join(sub)).is_err() {
            let _ = std::fs::remove_dir_all(&base);
            return false;
        }
    }

    let ok = match (
        mount_options(&base.join("lower"), &base.join("upper"), &base.join("work")),
        cstr(&base.join("merged")),
    ) {
        (Some(opts), Some(merged_c)) => unsafe {
            let pid = libc::fork();
            if pid < 0 {
                false
            } else if pid == 0 {
                let r = enter_namespace(mode, uid_map, gid_map) && mount_overlay(&merged_c, &opts);
                libc::_exit(i32::from(!r));
            } else {
                let mut status = 0;
                libc::waitpid(pid, &mut status, 0);
                libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0
            }
        },
        _ => false,
    };

    let _ = std::fs::remove_dir_all(&base);
    ok
}

/// Probe how (if at all) overlay mounts work for upper dirs created under
/// `scratch_parent` (the same filesystem that will host the real per-test
/// contexts). Performed once. Prefers the privileged mode so tests keep their
/// real uid when attest has `CAP_SYS_ADMIN`.
pub fn probe_support(scratch_parent: &Path) -> Option<Mode> {
    let (uid_map, gid_map) = id_maps();
    [Mode::Privileged, Mode::Userns]
        .into_iter()
        .find(|&mode| probe_mode(scratch_parent, mode, &uid_map, &gid_map))
}

/// Recursively copy `src` into `dst`, creating `dst` if needed. Regular files,
/// directories, and symlinks are reproduced; other special files (e.g. overlay
/// whiteout device nodes for deletions) are skipped.
pub fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if file_type.is_symlink() {
            let target = std::fs::read_link(&from)?;
            std::os::unix::fs::symlink(target, &to)?;
        } else if file_type.is_file() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}
