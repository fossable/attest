#[cfg(feature = "cgroup")]
mod cgroup;
mod diagnostics;
mod discovery;
mod output;
mod parser;
mod runner;

use std::path::{Path, PathBuf};

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::engine::ArgValueCompleter;

#[derive(Parser)]
#[command(version, name = "attest", about = "Shell-based test framework")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Test target: a file, directory, or `<file>/<test>` pattern
    #[arg(add = ArgValueCompleter::new(complete_tests))]
    path: Option<String>,

    /// Maximum number of tests to run in parallel (defaults to number of CPU cores)
    #[arg(long, default_value_t = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1))]
    parallel: usize,

    /// Filter tests by pattern: `[<file>/]<name>` where `<name>` supports `*` wildcards and
    /// plain names match as prefixes (e.g. `foo.sh/test_net*`)
    #[arg(long, add = ArgValueCompleter::new(complete_tests))]
    filter: Option<String>,

    /// Copy all test context directories to this directory on exit
    #[arg(long)]
    results: Option<PathBuf>,

    /// Copy failed test context directories to this directory on exit
    #[arg(long)]
    results_failed: Option<PathBuf>,

    /// Stop after first test failure
    #[arg(long)]
    bail: bool,

    /// Print xtrace output from tests as they run (one test at a time)
    #[arg(short = 'x', long)]
    xtrace: bool,

    /// Prepend a directory to $PATH for tests (can be specified multiple times)
    #[arg(long)]
    add_path: Vec<PathBuf>,

    /// Trace a command with strace, saving output to the test context dir (can be specified multiple times)
    #[arg(long)]
    strace: Vec<String>,

    /// Enable debug logging
    #[arg(short = 'd', long)]
    debug: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// List test functions without running them
    List {
        /// Test target: a file, directory, or `<file>/<test>` pattern
        #[arg(add = ArgValueCompleter::new(complete_tests))]
        path: String,

        /// Filter tests by pattern: `[<file>/]<name>` where `<name>` supports `*` wildcards and
        /// plain names match as prefixes (e.g. `foo.sh/test_net*`)
        #[arg(long, add = ArgValueCompleter::new(complete_tests))]
        filter: Option<String>,
    },
    /// Print the AI skill document for writing .test files
    Skill,
}

/// Split a positional argument into a file path and an optional test filter.
///
/// If the argument contains a `/` where the left side is an existing file,
/// it is treated as `file/filter`. Otherwise the whole string is a path.
fn split_path_arg(arg: &str) -> (PathBuf, Option<String>) {
    // Try splitting from the right on `/` — the left side must be an existing file
    if let Some(slash) = arg.rfind('/') {
        let file_part = &arg[..slash];
        let name_part = &arg[slash + 1..];
        let path = Path::new(file_part);
        if path.is_file() {
            let filter = if name_part.is_empty() {
                None
            } else {
                Some(format!("{file_part}/{name_part}"))
            };
            return (path.to_path_buf(), filter);
        }
    }
    (PathBuf::from(arg), None)
}

fn complete_tests(current: &std::ffi::OsStr) -> Vec<clap_complete::engine::CompletionCandidate> {
    use clap_complete::engine::CompletionCandidate;

    let current = current.to_string_lossy();
    let mut candidates = Vec::new();

    // If input contains `/` and left side is a file, complete test names within it
    if let Some(slash) = current.rfind('/') {
        let file_part = &current[..slash];
        let name_prefix = &current[slash + 1..];
        let path = Path::new(file_part);

        if path.is_file() {
            if let Ok(test_file) = parser::parse_test_file(path) {
                for test in &test_file.tests {
                    if test.name.starts_with(name_prefix) {
                        candidates.push(CompletionCandidate::new(format!(
                            "{}/{}",
                            file_part, test.name
                        )));
                    }
                }
            }
            return candidates;
        }
    }

    // No file/test split — complete with discovered files and all file/test patterns
    let cwd = Path::new(".");
    if let Ok(files) = discovery::discover_test_files(cwd) {
        for file in &files {
            let rel = file.strip_prefix(cwd).unwrap_or(file).display().to_string();
            let rel = rel.strip_prefix("./").unwrap_or(&rel).to_string();

            if rel.starts_with(&*current) {
                candidates.push(CompletionCandidate::new(&rel));
            }

            if let Ok(test_file) = parser::parse_test_file(file) {
                for test in &test_file.tests {
                    let full = format!("{}/{}", rel, test.name);
                    if full.starts_with(&*current) {
                        candidates.push(CompletionCandidate::new(full));
                    }
                }
            }
        }
    }

    candidates
}

fn main() -> anyhow::Result<()> {
    clap_complete::CompleteEnv::with_factory(Cli::command).complete();

    let cli = Cli::parse();

    let env_filter = if cli.debug {
        tracing_subscriber::EnvFilter::new("debug")
    } else {
        tracing_subscriber::EnvFilter::from_default_env()
    };
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    match cli.command {
        Some(Commands::Skill) => {
            print!("{}", include_str!("../SKILL.md"));
        }
        Some(Commands::List { path, filter }) => {
            let (path, inline_filter) = split_path_arg(&path);
            let filter = filter.or(inline_filter);
            let pattern = filter.as_deref().map(parser::TestPattern::parse);
            let files = discovery::discover_test_files(&path)?;
            let mut tests = Vec::new();
            for file in &files {
                let test_file = parser::parse_test_file(file)?;
                for test in test_file.tests {
                    if let Some(ref p) = pattern
                        && !p.matches(&test)
                    {
                        continue;
                    }
                    tests.push(test);
                }
            }
            output::print_test_list(&tests);
        }
        None => {
            let path_arg = cli.path.unwrap_or_else(|| {
                eprintln!("error: a path to a test file or directory is required");
                std::process::exit(1);
            });

            let (path, inline_filter) = split_path_arg(&path_arg);
            let filter = cli.filter.or(inline_filter);
            let pattern = filter.as_deref().map(parser::TestPattern::parse);
            let files = discovery::discover_test_files(&path)?;
            let mut all_tests = Vec::new();
            for file in &files {
                let test_file = parser::parse_test_file(file)?;
                let functions = test_file.functions;
                for test in test_file.tests {
                    if let Some(ref p) = pattern
                        && !p.matches(&test)
                    {
                        continue;
                    }
                    all_tests.push((test.name, functions.clone(), file.clone()));
                }
            }

            let config = runner::RunConfig {
                parallel: cli.parallel,
                bail: cli.bail,
                xtrace: cli.xtrace,
                results: cli.results,
                results_failed: cli.results_failed,
                add_path: cli.add_path,
                strace: cli.strace,
            };

            let test_refs: Vec<(
                &str,
                &[brush_parser::ast::FunctionDefinition],
                &std::path::Path,
            )> = all_tests
                .iter()
                .map(|(name, funcs, src)| (name.as_str(), funcs.as_slice(), src.as_path()))
                .collect();

            let results = runner::run_all_tests(test_refs, &config)?;

            if results.iter().any(|r| !r.passed) {
                std::process::exit(1);
            }
        }
    }

    Ok(())
}
