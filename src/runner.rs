use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use brush_parser::ast::FunctionDefinition;

use crate::output;

pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub duration: Duration,
    pub tmp_dir: PathBuf,
}

pub struct RunConfig {
    pub sequential: bool,
    pub failed: bool,
}

pub async fn run_all_tests(
    tests: Vec<(&str, &[FunctionDefinition])>,
    config: &RunConfig,
) -> anyhow::Result<Vec<TestResult>> {
    let mut results = Vec::new();

    if config.sequential {
        for (test_name, all_functions) in tests {
            let result = run_single_test(test_name, all_functions).await?;
            output::print_test_result(&result);
            results.push(result);
        }
    } else {
        let mut join_set = tokio::task::JoinSet::new();
        for (test_name, all_functions) in tests {
            let test_name = test_name.to_string();
            let all_functions: Vec<FunctionDefinition> = all_functions.to_vec();
            join_set.spawn(async move { run_single_test(&test_name, &all_functions).await });
        }
        while let Some(result) = join_set.join_next().await {
            let result = result??;
            output::print_test_result(&result);
            results.push(result);
        }
    }

    if config.failed {
        copy_failed_dirs(&results)?;
    }

    output::print_summary(&results);
    Ok(results)
}

async fn run_single_test(
    test_name: &str,
    all_functions: &[FunctionDefinition],
) -> anyhow::Result<TestResult> {
    let tmp_dir = tempfile::TempDir::new()?;
    let tmp_path = tmp_dir.path().to_path_buf();

    // Write all functions to a temporary script
    let script_path = tmp_path.join("functions.sh");
    let mut script = String::new();
    for func in all_functions {
        script.push_str(&func.to_string());
        script.push('\n');
    }
    std::fs::write(&script_path, &script)?;

    let start = Instant::now();
    let passed = execute_test(test_name, &script_path, &tmp_path).await?;
    let duration = start.elapsed();

    // Persist the tmp dir so it's available for --failed
    let tmp_path = tmp_dir.keep();

    Ok(TestResult {
        name: test_name.to_string(),
        passed,
        duration,
        tmp_dir: tmp_path,
    })
}

async fn execute_test(
    test_name: &str,
    script_path: &Path,
    working_dir: &Path,
) -> anyhow::Result<bool> {
    use brush_builtins::ShellBuilderExt;
    let mut shell = brush_core::Shell::builder()
        .no_profile(true)
        .no_rc(true)
        .default_builtins(brush_builtins::BuiltinSet::BashMode)
        .build()
        .await
        .map_err(|e| anyhow::anyhow!("failed to create shell: {e}"))?;

    shell
        .set_working_dir(working_dir)
        .map_err(|e| anyhow::anyhow!("failed to set working dir: {e}"))?;

    let params = brush_core::ExecutionParameters::default();

    // Redirect stdout and stderr to log files, then enable xtrace
    let stdout_path = working_dir.join("stdout.log");
    let xtrace_path = working_dir.join("xtrace.log");
    let setup = format!(
        "exec 1>{} 2>{} ; set -ex",
        stdout_path.display(),
        xtrace_path.display()
    );
    shell
        .run_string(setup, &params)
        .await
        .map_err(|e| anyhow::anyhow!("failed to set up xtrace: {e}"))?;

    // Source the functions script
    let empty: &[&str] = &[];
    let source_result = shell
        .source_script(script_path, empty.iter(), &params)
        .await
        .map_err(|e| anyhow::anyhow!("failed to source functions: {e}"))?;

    if !source_result.is_success() {
        return Ok(false);
    }

    // Run the test function
    let result = shell
        .run_string(test_name.to_string(), &params)
        .await
        .map_err(|e| anyhow::anyhow!("failed to run test {test_name}: {e}"))?;

    Ok(result.is_success())
}

fn copy_failed_dirs(results: &[TestResult]) -> anyhow::Result<()> {
    let failed_dir = PathBuf::from("failed");
    let has_failures = results.iter().any(|r| !r.passed);
    if !has_failures {
        return Ok(());
    }

    if failed_dir.exists() {
        std::fs::remove_dir_all(&failed_dir)?;
    }
    std::fs::create_dir_all(&failed_dir)?;

    for result in results.iter().filter(|r| !r.passed) {
        let dest = failed_dir.join(&result.name);
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

impl Drop for TestResult {
    fn drop(&mut self) {
        // Clean up tmp dir
        let _ = std::fs::remove_dir_all(&self.tmp_dir);
    }
}
