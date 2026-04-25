## Architecture

- `src/main.rs` - CLI entry point using clap. Default action is `run`; `list` is
  a subcommand.
- `src/discovery.rs` - Finds test files from a file path or directory
  (recursive). Scans all shell scripts (by extension or shebang) for test
  functions, not just `.test` files.
- `src/parser.rs` - Parses shell scripts with `brush_parser::Parser`, walks the
  AST to extract all `FunctionDefinition` nodes. Test functions start with
  `test`.
- `src/runner.rs` - For each test: writes all extracted functions (test +
  helper) to a temp script, creates a `brush_core::Shell` with `brush_builtins`,
  redirects stdout/stderr to log files, enables xtrace (`set -ex`), sources the
  script, then invokes the test function by name. Parallel by default via
  `tokio::task::JoinSet`.
- `src/output.rs` - ANSI-colored terminal output for PASS/FAIL and summary.

## Key dependencies

- `brush-parser` - Tokenizes and parses shell scripts into an AST
- `brush-core` - Embeddable shell engine (`Shell`, `CreateOptions`,
  `ExecutionParameters`)
- `brush-builtins` - Registers shell builtins (set, test, cd, etc.) via
  `ShellBuilderExt`
- `clap` - CLI argument parsing
- `tokio` - Async runtime for parallel test execution
- `tempfile` - Per-test temporary directories

## Test file format

Standalone test files (`.test`) have `attest` in their shebang and contain
functions prefixed with `test`. Test functions can also be inline in any regular
shell script. When scanning directories, all shell scripts (identified by
extension or shebang) are checked for test functions. Every command in a test
function is an implicit assertion - if it exits nonzero, the test fails.
Non-test functions (helpers/setup) are also extracted and made available to
tests.

## TODO list

- Each test runs in a cgroup and we log the CPU time, IO, memory that the test
  consumed.
