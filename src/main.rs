#[cfg(feature = "cgroup")]
mod cgroup;
mod discovery;
mod output;
mod parser;
mod runner;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "attest", about = "Shell-based test framework")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to a test file or directory to scan for test functions
    path: Option<PathBuf>,

    /// Run tests sequentially instead of in parallel
    #[arg(long)]
    sequential: bool,

    /// Filter tests by name (substring match)
    #[arg(long)]
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

    /// Prepend a directory to $PATH for tests (can be specified multiple times)
    #[arg(long)]
    add_path: Vec<PathBuf>,

    /// Trace a command with strace, saving output to the test context dir (can be specified multiple times)
    #[arg(long)]
    strace: Vec<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// List test functions without running them
    List {
        /// Path to a test file or directory to scan for test functions
        path: PathBuf,

        /// Filter tests by name (substring match)
        #[arg(long)]
        filter: Option<String>,
    },
    /// Print the AI skill document for writing .test files
    Skill,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Skill) => {
            print!("{}", include_str!("../SKILL.md"));
        }
        Some(Commands::List { path, filter }) => {
            let files = discovery::discover_test_files(&path)?;
            let mut tests = Vec::new();
            for file in &files {
                let test_file = parser::parse_test_file(file)?;
                for test in test_file.tests {
                    if let Some(ref f) = filter
                        && !test.name.contains(f.as_str())
                    {
                        continue;
                    }
                    tests.push(test);
                }
            }
            output::print_test_list(&tests);
        }
        None => {
            let path = cli.path.unwrap_or_else(|| {
                eprintln!("error: a path to a test file or directory is required");
                std::process::exit(1);
            });

            let files = discovery::discover_test_files(&path)?;
            let mut all_tests = Vec::new();
            for file in &files {
                let test_file = parser::parse_test_file(file)?;
                let functions = test_file.functions;
                for test in test_file.tests {
                    if let Some(ref f) = cli.filter
                        && !test.name.contains(f.as_str())
                    {
                        continue;
                    }
                    all_tests.push((test.name, functions.clone()));
                }
            }

            let config = runner::RunConfig {
                sequential: cli.sequential,
                bail: cli.bail,
                results: cli.results,
                results_failed: cli.results_failed,
                add_path: cli.add_path,
                strace: cli.strace,
            };

            let test_refs: Vec<(&str, &[brush_parser::ast::FunctionDefinition])> = all_tests
                .iter()
                .map(|(name, funcs)| (name.as_str(), funcs.as_slice()))
                .collect();

            let results = runner::run_all_tests(test_refs, &config)?;

            if results.iter().any(|r| !r.passed) {
                std::process::exit(1);
            }
        }
    }

    Ok(())
}
