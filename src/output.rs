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
    pub fn new(total: usize) -> Self {
        if !indicatif::ProgressDrawTarget::stderr().is_hidden() {
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

pub fn print_test_result(result: &TestResult) {
    let (label, color) = if result.passed {
        ("PASS", GREEN)
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
