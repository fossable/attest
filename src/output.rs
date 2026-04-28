use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};

use crate::parser::TestCase;
use crate::runner::TestResult;

const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

pub struct StatusDisplay {
    bar: Option<ProgressBar>,
}

impl StatusDisplay {
    pub fn new(total: usize, json: bool) -> Self {
        if !json && !indicatif::ProgressDrawTarget::stderr().is_hidden() {
            let bar = ProgressBar::new(total as u64);
            bar.set_style(
                ProgressStyle::default_bar()
                    .template("\x1b[1;32mTesting\x1b[0m {pos}/{len}: [{msg}]")
                    .unwrap(),
            );
            bar.enable_steady_tick(Duration::from_millis(250));
            Self { bar: Some(bar) }
        } else {
            Self { bar: None }
        }
    }

    /// Update the status line with currently running tests and their elapsed times.
    pub fn update(&self, running: &[(&str, Duration)], completed: usize) {
        if let Some(ref bar) = self.bar {
            bar.set_position(completed as u64);
            let msg: String = running
                .iter()
                .map(|(name, elapsed)| format!("{}({})", name, format_duration(*elapsed)))
                .collect::<Vec<_>>()
                .join(", ");
            bar.set_message(msg);
        }
    }

    /// Run a closure with the status line temporarily hidden, so printed output
    /// doesn't collide with it.
    pub fn suspend<F: FnOnce()>(&self, f: F) {
        if let Some(ref bar) = self.bar {
            bar.suspend(f);
        } else {
            f();
        }
    }

    pub fn finish(&self) {
        if let Some(ref bar) = self.bar {
            bar.finish_and_clear();
        }
    }
}

/// Escape a string for inclusion as a JSON string value (without surrounding quotes).
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

pub fn print_test_result_json(result: &TestResult) {
    let status = if result.passed {
        "pass"
    } else if result.timed_out {
        "timeout"
    } else {
        "fail"
    };

    let read_log = |name: &str| -> String {
        std::fs::read_to_string(result.tmp_dir.join(name)).unwrap_or_default()
    };

    let stdout = json_escape(&read_log("stdout.log"));
    let xtrace = json_escape(&read_log("xtrace.log"));

    // Collect strace logs: strace/<cmd>.log → key is <cmd>
    let strace_dir = result.tmp_dir.join("strace");
    let mut strace_pairs: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&strace_dir) {
        let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let fname = entry.file_name();
            let fname = fname.to_string_lossy();
            let key = fname.strip_suffix(".log").unwrap_or(&fname);
            let content = std::fs::read_to_string(entry.path()).unwrap_or_default();
            strace_pairs.push(format!(
                "\"{}\":\"{}\"",
                json_escape(key),
                json_escape(&content)
            ));
        }
    }
    let strace_obj = format!("{{{}}}", strace_pairs.join(","));

    #[cfg(feature = "cgroup")]
    let resources_json = match result.resources {
        Some(ref r) => {
            let mut fields: Vec<String> = Vec::new();
            if let Some(v) = r.cpu_user_usec {
                fields.push(format!("\"cpu_user_usec\":{v}"));
            }
            if let Some(v) = r.cpu_system_usec {
                fields.push(format!("\"cpu_system_usec\":{v}"));
            }
            if let Some(v) = r.memory_peak {
                fields.push(format!("\"memory_peak\":{v}"));
            }
            if let Some(v) = r.io_read_bytes {
                fields.push(format!("\"io_read_bytes\":{v}"));
            }
            if let Some(v) = r.io_write_bytes {
                fields.push(format!("\"io_write_bytes\":{v}"));
            }
            if let Some(v) = r.pids_peak {
                fields.push(format!("\"pids_peak\":{v}"));
            }
            format!("{{{}}}", fields.join(","))
        }
        None => "null".to_string(),
    };
    #[cfg(not(feature = "cgroup"))]
    let resources_json = "null";

    let name = json_escape(&result.name);
    let file = json_escape(&result.source_path.display().to_string());
    let duration_ms = result.duration.as_millis();

    println!(
        r#"{{"name":"{name}","file":"{file}","status":"{status}","duration_ms":{duration_ms},"stdout":"{stdout}","xtrace":"{xtrace}","strace":{strace_obj},"resources":{resources_json}}}"#
    );
}

pub fn print_test_result(result: &TestResult) {
    let (label, color) = if result.passed {
        ("PASS", GREEN)
    } else if result.timed_out {
        ("TIME", RED)
    } else {
        ("FAIL", RED)
    };
    let duration = format_duration(result.duration);
    println!("{color}{label}{RESET}  {:<40} ({duration})", result.name);
    #[cfg(feature = "cgroup")]
    if let Some(ref r) = result.resources {
        print_resource_stats(r);
    }
    if !result.passed {
        crate::diagnostics::print_failure_snippet(result);
    }
}

#[cfg(feature = "cgroup")]
fn print_resource_stats(r: &crate::cgroup::ResourceStats) {
    let mut parts: Vec<String> = Vec::new();

    match (r.cpu_user_usec, r.cpu_system_usec) {
        (Some(u), Some(s)) => parts.push(format!(
            "cpu={:.1}ms+{:.1}ms",
            u as f64 / 1000.0,
            s as f64 / 1000.0
        )),
        (Some(u), None) => parts.push(format!("cpu={:.1}ms", u as f64 / 1000.0)),
        (None, Some(s)) => parts.push(format!("cpu=sys:{:.1}ms", s as f64 / 1000.0)),
        (None, None) => {}
    }

    if let Some(m) = r.memory_peak {
        parts.push(format!("mem={}", format_bytes(m)));
    }

    match (r.io_read_bytes, r.io_write_bytes) {
        (Some(rb), Some(wb)) => parts.push(format!("io={}/{}", format_bytes(rb), format_bytes(wb))),
        (Some(rb), None) => parts.push(format!("io={}r", format_bytes(rb))),
        (None, Some(wb)) => parts.push(format!("io={}w", format_bytes(wb))),
        (None, None) => {}
    }

    if let Some(p) = r.pids_peak {
        parts.push(format!("pids={p}"));
    }

    if !parts.is_empty() {
        println!("      {}", parts.join("  "));
    }
}

#[cfg(feature = "cgroup")]
fn format_bytes(b: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    if b >= GIB {
        format!("{:.2}GiB", b as f64 / GIB as f64)
    } else if b >= MIB {
        format!("{:.1}MiB", b as f64 / MIB as f64)
    } else if b >= KIB {
        format!("{:.1}KiB", b as f64 / KIB as f64)
    } else {
        format!("{b}B")
    }
}

pub fn print_summary(results: &[TestResult], wall_duration: Duration) {
    let passed = results.iter().filter(|r| r.passed).count();
    let failed = results.len() - passed;

    println!();
    if failed > 0 {
        println!(
            "Results: {GREEN}{passed} passed{RESET}, {RED}{failed} failed{RESET}, {} total",
            results.len()
        );
    } else {
        println!(
            "Results: {GREEN}{passed} passed{RESET}, {} total",
            results.len()
        );
    }
    println!("Time:   {}", format_duration(wall_duration));
}

pub fn print_test_list(tests: &[TestCase]) {
    for test in tests {
        let filename = test
            .file
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_else(|| test.file.to_string_lossy());
        println!("{}/{}", filename, test.name);
    }
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 1.0 {
        format!("{:.0}ms", d.as_millis())
    } else {
        format!("{secs:.2}s")
    }
}
