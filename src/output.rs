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
