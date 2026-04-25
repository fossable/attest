use std::time::Duration;

use crate::parser::TestCase;
use crate::runner::TestResult;

const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

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

pub fn print_summary(results: &[TestResult]) {
    let passed = results.iter().filter(|r| r.passed).count();
    let failed = results.len() - passed;
    let total_duration: Duration = results.iter().map(|r| r.duration).sum();

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
    println!("Time:   {}", format_duration(total_duration));
}

pub fn print_test_list(tests: &[TestCase]) {
    for test in tests {
        println!("{}:{}", test.file.display(), test.name);
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
