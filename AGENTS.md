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
  `fork(2)` with configurable parallelism (`--parallel`).
- `src/output.rs` - ANSI-colored terminal output for PASS/FAIL and summary.

## Key dependencies

- `brush-parser` - Tokenizes and parses shell scripts into an AST
- `clap` - CLI argument parsing
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

- Refine the runner UI
  - With a tty attached, the last line continuously updates 4 times per second
    for example: "Testing 2/30: [testHelp(10ms), testVersion(52ms)]" where the
    currently executing test is shown with the total CPU time reported by the
    test's cgroup.
  - When a test fails, render a snippet of the test's source code where it
    failed in the terminal. Find a dependency similar to Python's rich library
    to do this.
    - If the line that failed is a `[`, then parse the operands to render rich
      inforation about the failure. For example for the failed command:
      `[ "AABB" = "BBBB" ]`, show a diff of the two operands.
  - With xtrace mode (-x), print each test's xtrace output to the terminal as
    it's produced. Create a lock to prevent interleaving output from multiple
    tests. Only one test is allowed to incrementally print xtrace output at a
    time.
  - With debug mode (-d), increase the log level to DEBUG.
