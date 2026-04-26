use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use brush_parser::ast::FunctionDefinition;

use crate::output;

/// Tracks live xtrace streaming state: which test holds the output lock and how
/// far we've read into its xtrace.log.
struct XtraceStreamer {
    /// Name of the test currently holding the xtrace output lock.
    holder: Option<String>,
    /// File handle for the current holder's xtrace.log.
    file: Option<std::fs::File>,
    /// How many bytes have been printed so far.
    offset: u64,
}

impl XtraceStreamer {
    fn new() -> Self {
        Self {
            holder: None,
            file: None,
            offset: 0,
        }
    }

    /// Acquire the lock for a test if no one currently holds it.
    fn try_acquire(&mut self, pending: &PendingTest) {
        if self.holder.is_some() {
            return;
        }
        let xtrace_path = pending.tmp_dir.as_ref().unwrap().join("xtrace.log");
        if let Ok(f) = std::fs::File::open(&xtrace_path) {
            eprintln!("\x1b[2m--- xtrace: {} ---\x1b[0m", pending.name);
            self.holder = Some(pending.name.clone());
            self.file = Some(f);
            self.offset = 0;
        }
    }

    /// Check whether the current holder's xtrace.log has new data.
    fn has_new(&mut self) -> bool {
        let Some(ref mut f) = self.file else {
            return false;
        };
        f.metadata().map_or(false, |m| m.len() > self.offset)
    }

    /// Print any new bytes from the current holder's xtrace.log.
    fn flush_new(&mut self) {
        let Some(ref mut f) = self.file else { return };
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut buf = Vec::new();
        if f.read_to_end(&mut buf).is_ok() && !buf.is_empty() {
            self.offset += buf.len() as u64;
            use std::io::Write;
            let _ = write!(std::io::stderr(), "\x1b[2m");
            let _ = std::io::stderr().write_all(&buf);
            let _ = write!(std::io::stderr(), "\x1b[0m");
        }
    }

    /// Release the lock (flush remaining output first).
    fn release(&mut self) {
        self.flush_new();
        self.holder = None;
        self.file = None;
        self.offset = 0;
    }

    /// Check if the named test currently holds the lock.
    fn is_holder(&self, name: &str) -> bool {
        self.holder.as_deref() == Some(name)
    }

    /// Dump the full xtrace log for a test that was never streamed (e.g. it
    /// finished before the parent could open the file).
    fn dump_missed(&self, pending: &PendingTest) {
        if self.is_holder(&pending.name) {
            return; // Will be flushed via release()
        }
        let xtrace_path = pending.tmp_dir.as_ref().unwrap().join("xtrace.log");
        if let Ok(content) = std::fs::read(&xtrace_path) {
            if !content.is_empty() {
                use std::io::Write;
                eprintln!("\x1b[2m--- xtrace: {} ---\x1b[0m", pending.name);
                let _ = write!(std::io::stderr(), "\x1b[2m");
                let _ = std::io::stderr().write_all(&content);
                let _ = write!(std::io::stderr(), "\x1b[0m");
            }
        }
    }
}

pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub duration: Duration,
    pub tmp_dir: PathBuf,
    pub source_path: PathBuf,
    #[cfg(feature = "cgroup")]
    pub resources: Option<crate::cgroup::ResourceStats>,
}

/// State held by the parent for a forked test child that has not yet been
/// waited on. Dropping this kills the child (if still running) and cleans up.
struct PendingTest {
    pid: libc::pid_t,
    /// Set to `true` once `waitpid` has reaped the child so `Drop` skips it.
    reaped: bool,
    name: String,
    start: Instant,
    /// `None` after the path has been transferred to `TestResult`.
    tmp_dir: Option<PathBuf>,
    source_path: PathBuf,
    #[cfg(feature = "cgroup")]
    cgroup: Option<crate::cgroup::TestCgroup>,
}

impl Drop for PendingTest {
    fn drop(&mut self) {
        if !self.reaped {
            unsafe {
                libc::kill(self.pid, libc::SIGKILL);
                let mut s = 0;
                libc::waitpid(self.pid, &mut s, 0);
            }
        }
        if let Some(ref dir) = self.tmp_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
        // cgroup field drops here, removing the cgroup directory
    }
}

pub struct RunConfig {
    pub parallel: usize,
    pub bail: bool,
    pub xtrace: bool,
    pub results: Option<PathBuf>,
    pub results_failed: Option<PathBuf>,
    pub add_path: Vec<PathBuf>,
    pub strace: Vec<String>,
}

pub fn run_all_tests(
    tests: Vec<(&str, &[FunctionDefinition], &Path)>,
    config: &RunConfig,
) -> anyhow::Result<Vec<TestResult>> {
    let mut results = Vec::new();
    let total = tests.len();
    let status = output::StatusDisplay::new(total);
    let wall_start = Instant::now();

    let max_parallel = config.parallel.max(1);
    let mut test_iter = tests.iter();
    let mut pending_list: Vec<PendingTest> = Vec::new();
    let mut bail_flag = false;
    let mut xtrace = if config.xtrace {
        Some(XtraceStreamer::new())
    } else {
        None
    };

    // Seed the initial batch up to max_parallel.
    while pending_list.len() < max_parallel {
        if let Some((test_name, all_functions, source_path)) = test_iter.next() {
            pending_list.push(fork_test(
                test_name,
                all_functions,
                source_path,
                &config.add_path,
                &config.strace,
            )?);
        } else {
            break;
        }
    }

    // If xtrace is enabled, acquire the lock for the first pending test.
    if let Some(ref mut xt) = xtrace {
        if let Some(p) = pending_list.first() {
            status.suspend(|| xt.try_acquire(p));
        }
    }

    // Poll loop: non-blocking reap, process completions, update status.
    while !pending_list.is_empty() {
        // Stream xtrace output from the current holder.
        if let Some(ref mut xt) = xtrace {
            if xt.has_new() {
                status.suspend(|| xt.flush_new());
            }
        }

        // Non-blocking reap: check all pending tests with WNOHANG.
        let mut reaped: Vec<(usize, libc::c_int)> = Vec::new();
        for (i, pending) in pending_list.iter().enumerate() {
            let mut status_code: libc::c_int = 0;
            let ret = unsafe { libc::waitpid(pending.pid, &mut status_code, libc::WNOHANG) };
            if ret > 0 {
                reaped.push((i, status_code));
            } else if ret == -1 {
                return Err(anyhow::anyhow!(
                    "waitpid failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            // ret == 0 means child still running
        }

        // Process reaped tests in reverse index order so removal doesn't shift indices.
        reaped.sort_by(|a, b| b.0.cmp(&a.0));
        let mut completed: Vec<TestResult> = Vec::new();
        for (i, exit_status) in reaped {
            if let Some(ref mut xt) = xtrace {
                // If this test held the xtrace lock, release it (flushes remaining output).
                // Otherwise dump the full log for tests that finished before we could stream.
                status.suspend(|| {
                    if xt.is_holder(&pending_list[i].name) {
                        xt.release();
                    } else {
                        xt.dump_missed(&pending_list[i]);
                    }
                });
            }
            let mut pending = pending_list.remove(i);
            if bail_flag {
                continue; // Drop kills + cleans up
            }
            pending.reaped = true;
            let result = build_result(pending, exit_status);
            completed.push(result);
        }

        // Print results and start new tests.
        // Sort completed by name for deterministic output within a reap batch.
        completed.sort_by(|a, b| a.name.cmp(&b.name));
        for result in completed {
            status.suspend(|| output::print_test_result(&result));
            if !result.passed && config.bail {
                bail_flag = true;
            }
            results.push(result);

            if !bail_flag {
                if let Some((test_name, all_functions, source_path)) = test_iter.next() {
                    pending_list.push(fork_test(
                        test_name,
                        all_functions,
                        source_path,
                        &config.add_path,
                        &config.strace,
                    )?);
                }
            }
        }

        if pending_list.is_empty() {
            break;
        }

        // If xtrace lock is free, acquire the next pending test.
        if let Some(ref mut xt) = xtrace {
            if xt.holder.is_none() {
                if let Some(p) = pending_list.first() {
                    status.suspend(|| xt.try_acquire(p));
                }
            }
        }

        // Update status line with currently running tests.
        let running: Vec<(&str, std::time::Duration)> = pending_list
            .iter()
            .map(|p| {
                #[cfg(feature = "cgroup")]
                let duration = p
                    .cgroup
                    .as_ref()
                    .and_then(|cg| cg.read_cpu_time())
                    .unwrap_or_else(|| p.start.elapsed());
                #[cfg(not(feature = "cgroup"))]
                let duration = p.start.elapsed();
                (p.name.as_str(), duration)
            })
            .collect();
        status.update(&running, results.len());

        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    status.finish();

    if let Some(ref dir) = config.results {
        copy_results_dirs(&results, dir, false)?;
    }
    if let Some(ref dir) = config.results_failed {
        copy_results_dirs(&results, dir, true)?;
    }

    output::print_summary(&results, wall_start.elapsed());
    Ok(results)
}

/// Fork a child process that will run the test. Returns a `PendingTest` that
/// the caller must pass to `wait_and_collect` (or simply drop to kill+clean up).
fn fork_test(
    test_name: &str,
    all_functions: &[FunctionDefinition],
    source_path: &Path,
    add_path: &[PathBuf],
    strace: &[String],
) -> anyhow::Result<PendingTest> {
    let tmp_dir = tempfile::TempDir::new()?;
    let tmp_path = tmp_dir.keep();

    // Write all functions to a temporary script before forking.
    let script_path = tmp_path.join("functions.sh");
    let mut script = String::new();
    for func in all_functions {
        script.push_str(&func.to_string());
        script.push('\n');
    }
    std::fs::write(&script_path, &script)?;

    if !strace.is_empty() {
        create_strace_wrappers(&tmp_path, strace)?;
    }

    // Clone data the child will need (fork copies memory, but owned values
    // need to be independent so both sides can operate without aliasing).
    let test_name_owned = test_name.to_string();
    let source_path_owned = source_path
        .canonicalize()
        .unwrap_or_else(|_| source_path.to_path_buf());
    let add_path_owned = add_path.to_vec();
    let strace_owned = strace.to_vec();
    let tmp_path_child = tmp_path.clone();

    #[cfg(feature = "cgroup")]
    let cgroup = crate::cgroup::TestCgroup::try_create(test_name);

    let start = Instant::now();

    let pid = unsafe { libc::fork() };
    match pid {
        -1 => Err(anyhow::anyhow!(
            "fork failed: {}",
            std::io::Error::last_os_error()
        )),
        0 => {
            // ── Child process ──────────────────────────────────────────────
            // Enter the cgroup before doing anything else so all child
            // processes are attributed to this test.
            #[cfg(feature = "cgroup")]
            if let Some(ref cg) = cgroup {
                cg.enter();
            }

            let runner_content = build_runner_script(
                &test_name_owned,
                &script_path,
                &tmp_path_child,
                &add_path_owned,
                &strace_owned,
            );

            // Exec /bin/sh -c <script> <source_path>: replaces this child
            // image entirely. Passing source_path as argv[0] makes $0 inside
            // the test functions refer to the original script, not the runner.
            use std::os::unix::process::CommandExt;
            let source_str = source_path_owned.to_str().unwrap_or("sh").to_string();
            let err = std::process::Command::new("/bin/sh")
                .args(["-c", &runner_content, &source_str])
                .exec();
            eprintln!("exec /bin/sh failed: {err}");
            unsafe { libc::_exit(1) };
        }
        child_pid => {
            // ── Parent process ─────────────────────────────────────────────
            Ok(PendingTest {
                pid: child_pid,
                reaped: false,
                name: test_name.to_string(),
                start,
                tmp_dir: Some(tmp_path),
                source_path: source_path_owned,
                #[cfg(feature = "cgroup")]
                cgroup,
            })
        }
    }
}

/// Build a `TestResult` from a `PendingTest` whose child has already been
/// reaped with the given raw `waitpid` status.
fn build_result(mut pending: PendingTest, status: libc::c_int) -> TestResult {
    let duration = pending.start.elapsed();
    let passed = libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0;

    // Read stats before dropping cgroup (which removes the directory).
    #[cfg(feature = "cgroup")]
    let resources = pending.cgroup.as_ref().map(|cg| cg.read_stats());

    let tmp_dir = pending.tmp_dir.take().unwrap();

    TestResult {
        name: pending.name.clone(),
        passed,
        duration,
        tmp_dir,
        source_path: pending.source_path.clone(),
        #[cfg(feature = "cgroup")]
        resources,
    }
    // pending drops here: reaped=true skips kill/wait, tmp_dir=None skips
    // dir removal, cgroup drops removing the cgroup directory.
}

/// Build the shell script content that sources the function definitions and
/// runs the named test. Used with `/bin/sh -c <content> <source_path>` so
/// that `$0` inside test functions refers to the original script.
fn build_runner_script(
    test_name: &str,
    functions_path: &Path,
    working_dir: &Path,
    add_path: &[PathBuf],
    strace: &[String],
) -> String {
    let mut s = String::new();

    // Strace wrappers dir must precede add_path so wrappers intercept calls.
    if !strace.is_empty() {
        let strace_bin = working_dir.join("strace_bin");
        s.push_str(&format!("export PATH={}:$PATH\n", strace_bin.display()));
    }

    if !add_path.is_empty() {
        let prefix = add_path
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(":");
        s.push_str(&format!("export PATH={prefix}:$PATH\n"));
    }

    // Redirect both stdout and stderr to log files, then enable xtrace.
    let stdout = working_dir.join("stdout.log");
    let xtrace = working_dir.join("xtrace.log");
    s.push_str(&format!(
        "exec 1>{} 2>{}\n",
        stdout.display(),
        xtrace.display()
    ));
    s.push_str("set -e\n");

    // Source function definitions, then enable xtrace and invoke the test function.
    s.push_str(&format!(". {}\n", functions_path.display()));
    s.push_str("PS4='+$LINENO: '\n");
    s.push_str("set -x\n");
    s.push_str(test_name);
    s.push('\n');

    s
}

fn create_strace_wrappers(working_dir: &Path, commands: &[String]) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let strace_bin = working_dir.join("strace_bin");
    std::fs::create_dir_all(&strace_bin)?;

    let strace_dir = working_dir.join("strace");
    std::fs::create_dir_all(&strace_dir)?;

    for cmd in commands {
        let real_path =
            which::which(cmd).map_err(|_| anyhow::anyhow!("--strace: command not found: {cmd}"))?;

        let wrapper = strace_bin.join(cmd);
        let strace_out = strace_dir.join(format!("{cmd}.log"));
        let script = format!(
            "#!/bin/sh\nexec strace -f -o {} {} \"$@\"\n",
            strace_out.display(),
            real_path.display(),
        );
        std::fs::write(&wrapper, script)?;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755))?;
    }

    Ok(())
}

fn copy_results_dirs(
    results: &[TestResult],
    dest_dir: &Path,
    failed_only: bool,
) -> anyhow::Result<()> {
    let iter = results.iter().filter(|r| !failed_only || !r.passed);

    if failed_only && !results.iter().any(|r| !r.passed) {
        return Ok(());
    }

    if dest_dir.exists() {
        std::fs::remove_dir_all(dest_dir)?;
    }
    std::fs::create_dir_all(dest_dir)?;

    for result in iter {
        let dest = dest_dir.join(&result.name);
        copy_dir_recursive(&result.tmp_dir, &dest)?;
    }

    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Blocking wait + build result, for tests only.
    fn wait_and_collect(mut pending: PendingTest) -> TestResult {
        let mut status: libc::c_int = 0;
        let ret = unsafe { libc::waitpid(pending.pid, &mut status, 0) };
        assert!(ret > 0, "waitpid failed");
        pending.reaped = true;
        build_result(pending, status)
    }

    /// Parse `script` content and run `test_name` via fork_test + wait_and_collect.
    fn run_inline(script: &str, test_name: &str) -> TestResult {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("t.sh");
        fs::write(&path, script).unwrap();
        let tf = crate::parser::parse_test_file(&path).unwrap();
        let pending = fork_test(test_name, &tf.functions, &path, &[], &[]).unwrap();
        wait_and_collect(pending)
    }

    #[test]
    fn execute_passing_test() {
        assert!(run_inline("test_pass() {\n  true\n}\n", "test_pass").passed);
    }

    #[test]
    fn execute_failing_test() {
        assert!(!run_inline("test_fail() {\n  false\n}\n", "test_fail").passed);
    }

    #[test]
    fn execute_test_with_helper() {
        assert!(run_inline(
            "get_value() {\n  echo 42\n}\ntest_helper() {\n  val=$(get_value)\n  test \"$val\" = \"42\"\n}\n",
            "test_helper",
        ).passed);
    }

    #[test]
    fn execute_test_stdout_captured() {
        let r = run_inline("test_echo() {\n  echo captured_output\n}\n", "test_echo");
        let stdout = fs::read_to_string(r.tmp_dir.join("stdout.log")).unwrap();
        assert!(stdout.contains("captured_output"));
    }

    #[test]
    fn execute_test_with_add_path() {
        let tmp = TempDir::new().unwrap();
        let bin_dir = tmp.path().join("mybin");
        fs::create_dir(&bin_dir).unwrap();

        let script_content = "test_path() {\n  echo \"$PATH\" | grep -q mybin\n}\n";
        let path = tmp.path().join("t.sh");
        fs::write(&path, script_content).unwrap();
        let tf = crate::parser::parse_test_file(&path).unwrap();
        let pending = fork_test("test_path", &tf.functions, &path, &[bin_dir], &[]).unwrap();
        let result = wait_and_collect(pending);
        assert!(result.passed);
    }

    #[test]
    fn copy_dir_recursive_works() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src_dir");
        let dst = tmp.path().join("dst_dir");

        fs::create_dir(&src).unwrap();
        fs::write(src.join("file.txt"), "content").unwrap();
        let sub = src.join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("nested.txt"), "nested").unwrap();

        copy_dir_recursive(&src, &dst).unwrap();

        assert!(dst.join("file.txt").exists());
        assert_eq!(fs::read_to_string(dst.join("file.txt")).unwrap(), "content");
        assert!(dst.join("sub/nested.txt").exists());
        assert_eq!(
            fs::read_to_string(dst.join("sub/nested.txt")).unwrap(),
            "nested"
        );
    }

    #[test]
    fn copy_failed_dirs_creates_output() {
        let tmp = TempDir::new().unwrap();

        // Create a fake test tmp dir with a log file
        let test_tmp = tmp.path().join("test_tmp");
        fs::create_dir(&test_tmp).unwrap();
        fs::write(test_tmp.join("stdout.log"), "output").unwrap();

        let results = vec![TestResult {
            name: "test_failure".to_string(),
            passed: false,
            duration: Duration::from_millis(10),
            tmp_dir: test_tmp.clone(),
            source_path: PathBuf::from("test.sh"),
            #[cfg(feature = "cgroup")]
            resources: None,
        }];

        let failed_dir = tmp.path().join("failed");
        copy_results_dirs(&results, &failed_dir, true).unwrap();
        assert!(failed_dir.join("test_failure/stdout.log").exists());

        // Prevent Drop from cleaning up test_tmp since we reference it in results
        std::mem::forget(results);
    }

    #[test]
    fn create_strace_wrappers_creates_scripts() {
        // Only run if strace and ls are available
        if which::which("ls").is_err() {
            return;
        }

        let tmp = TempDir::new().unwrap();
        let commands = vec!["ls".to_string()];

        create_strace_wrappers(tmp.path(), &commands).unwrap();

        let wrapper = tmp.path().join("strace_bin/ls");
        assert!(wrapper.exists());

        let content = fs::read_to_string(&wrapper).unwrap();
        assert!(content.starts_with("#!/bin/sh\n"));
        assert!(content.contains("strace"));
        assert!(content.contains("\"$@\""));

        // Check it's executable
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::metadata(&wrapper).unwrap().permissions();
        assert!(perms.mode() & 0o111 != 0);
    }

    #[test]
    fn create_strace_wrappers_unknown_command_errors() {
        let tmp = TempDir::new().unwrap();
        let result = create_strace_wrappers(tmp.path(), &["nonexistent_cmd_xyz".to_string()]);
        assert!(result.is_err());
    }

    #[test]
    fn run_all_tests_serial() {
        let tmp = TempDir::new().unwrap();
        let script_content = "test_a() {\n  true\n}\ntest_b() {\n  true\n}\n";

        // Parse to get real FunctionDefinitions
        let path = tmp.path().join("test.sh");
        fs::write(&path, script_content).unwrap();
        let test_file = crate::parser::parse_test_file(&path).unwrap();

        let config = RunConfig {
            parallel: 1,
            bail: false,
            xtrace: false,
            results: None,
            results_failed: None,
            add_path: vec![],
            strace: vec![],
        };

        let test_refs: Vec<(&str, &[FunctionDefinition], &Path)> = test_file
            .tests
            .iter()
            .map(|t| {
                (
                    t.name.as_str(),
                    test_file.functions.as_slice(),
                    path.as_path(),
                )
            })
            .collect();

        let results = run_all_tests(test_refs, &config).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.passed));
    }

    #[test]
    fn bail_stops_after_first_failure() {
        let tmp = TempDir::new().unwrap();
        // test_fail comes first alphabetically, test_pass second
        let script_content = "test_fail() {\n  false\n}\ntest_pass() {\n  true\n}\n";

        let path = tmp.path().join("test.sh");
        fs::write(&path, script_content).unwrap();
        let test_file = crate::parser::parse_test_file(&path).unwrap();

        let config = RunConfig {
            parallel: 1,
            bail: true,
            xtrace: false,
            results: None,
            results_failed: None,
            add_path: vec![],
            strace: vec![],
        };

        let test_refs: Vec<(&str, &[FunctionDefinition], &Path)> = test_file
            .tests
            .iter()
            .map(|t| {
                (
                    t.name.as_str(),
                    test_file.functions.as_slice(),
                    path.as_path(),
                )
            })
            .collect();

        let results = run_all_tests(test_refs, &config).unwrap();
        // Only the failing test ran; bail stopped execution
        assert_eq!(results.len(), 1);
        assert!(!results[0].passed);
    }

    #[test]
    fn run_all_tests_parallel() {
        let tmp = TempDir::new().unwrap();
        let script_content = "test_x() {\n  true\n}\ntest_y() {\n  false\n}\n";

        let path = tmp.path().join("test.sh");
        fs::write(&path, script_content).unwrap();
        let test_file = crate::parser::parse_test_file(&path).unwrap();

        let config = RunConfig {
            parallel: 0,
            bail: false,
            xtrace: false,
            results: None,
            results_failed: None,
            add_path: vec![],
            strace: vec![],
        };

        let test_refs: Vec<(&str, &[FunctionDefinition], &Path)> = test_file
            .tests
            .iter()
            .map(|t| {
                (
                    t.name.as_str(),
                    test_file.functions.as_slice(),
                    path.as_path(),
                )
            })
            .collect();

        let results = run_all_tests(test_refs, &config).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|r| r.passed));
        assert!(results.iter().any(|r| !r.passed));
    }
}

impl Drop for TestResult {
    fn drop(&mut self) {
        // Clean up tmp dir
        let _ = std::fs::remove_dir_all(&self.tmp_dir);
    }
}
