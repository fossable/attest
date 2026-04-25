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

    /// Path to a .test file or directory of .test files
    path: Option<PathBuf>,

    /// Run tests sequentially instead of in parallel
    #[arg(long)]
    sequential: bool,

    /// Filter tests by name (substring match)
    #[arg(long)]
    filter: Option<String>,

    /// Copy failed test tmp dirs to ./failed
    #[arg(long)]
    failed: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// List test functions without running them
    List {
        /// Path to a .test file or directory of .test files
        path: PathBuf,

        /// Filter tests by name (substring match)
        #[arg(long)]
        filter: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::List { path, filter }) => {
            let files = discovery::discover_test_files(&path)?;
            let mut tests = Vec::new();
            for file in &files {
                let test_file = parser::parse_test_file(file)?;
                for test in test_file.tests {
                    if let Some(ref f) = filter {
                        if !test.name.contains(f.as_str()) {
                            continue;
                        }
                    }
                    tests.push(test);
                }
            }
            output::print_test_list(&tests);
        }
        None => {
            let path = cli.path.unwrap_or_else(|| {
                eprintln!("error: a path to a .test file or directory is required");
                std::process::exit(1);
            });

            let files = discovery::discover_test_files(&path)?;
            let mut all_tests = Vec::new();
            for file in &files {
                let test_file = parser::parse_test_file(file)?;
                let functions = test_file.functions;
                for test in test_file.tests {
                    if let Some(ref f) = cli.filter {
                        if !test.name.contains(f.as_str()) {
                            continue;
                        }
                    }
                    all_tests.push((test.name, functions.clone()));
                }
            }

            let config = runner::RunConfig {
                sequential: cli.sequential,
                failed: cli.failed,
            };

            let test_refs: Vec<(&str, &[brush_parser::ast::FunctionDefinition])> = all_tests
                .iter()
                .map(|(name, funcs)| (name.as_str(), funcs.as_slice()))
                .collect();

            let results = runner::run_all_tests(test_refs, &config).await?;

            if results.iter().any(|r| !r.passed) {
                std::process::exit(1);
            }
        }
    }

    Ok(())
}
