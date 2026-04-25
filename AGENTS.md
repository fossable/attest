## Architecture

- `src/main.rs` - CLI entry point using clap. Default action is `run`; `list` is a subcommand.
- `src/discovery.rs` - Finds `.test` files from a file path or directory (recursive).
- `src/parser.rs` - Parses `.test` files with `brush_parser::Parser`, walks the AST to
  extract all `FunctionDefinition` nodes. Test functions start with `test`.
- `src/runner.rs` - For each test: writes all extracted functions (test + helper) to a
  temp script, creates a `brush_core::Shell` with `brush_builtins`, redirects stdout/stderr
  to log files, enables xtrace (`set -ex`), sources the script, then invokes the test
  function by name. Parallel by default via `tokio::task::JoinSet`.
- `src/output.rs` - ANSI-colored terminal output for PASS/FAIL and summary.

## Key dependencies

- `brush-parser` - Tokenizes and parses shell scripts into an AST
- `brush-core` - Embeddable shell engine (`Shell`, `CreateOptions`, `ExecutionParameters`)
- `brush-builtins` - Registers shell builtins (set, test, cd, etc.) via `ShellBuilderExt`
- `clap` - CLI argument parsing
- `tokio` - Async runtime for parallel test execution
- `tempfile` - Per-test temporary directories

## Test file format

Test files (`.test`) are shell scripts containing functions prefixed with `test`. Every
command in a test function is an implicit assertion - if it exits nonzero, the test fails.
Non-test functions (helpers/setup) are also extracted and made available to tests.

## TODO list

- ~~Initial implementation using the brush family of crates~~
  - ~~brush_parser for parsing and brush_core for executing test functions~~
  - ~~see examples/md5sum.test for how the tests look~~
  - ~~tests run in parallel by default and sequentially with --sequential~~
  - ~~tests run in separate tmp dirs~~
    - ~~stdout captured to stdout.log, xtrace to xtrace.log in each test's tmp dir~~
    - ~~if a test fails, its tmp dir is copied to ./failed with --failed~~
  - ~~either a single test file can be passed on the command line or a directory
    of test files~~
  - ~~the command line allows filtering tests with --filter~~
  - ~~list subcommand that enumerates tests~~
- Inline tests
- Timing
- Coverage
- Manage $PATH
