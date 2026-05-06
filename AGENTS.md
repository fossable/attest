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
  helper) to a temp script, forks a child that execs `/bin/sh -c`, redirects
  stdout to `stdout.log` and stderr to `xtrace.log`, enables `set -ex`, sources
  the script, then invokes the test function by name. Parallel by default via
  `fork(2)` with configurable parallelism (`--parallel`). Supports `--timeout`,
  `--bail`, `--override`, and `--strace`.
- `src/diagnostics.rs` - On failure, parses `xtrace.log` to find the last
  executed command, maps it back to the original source file, and renders an
  annotate-snippets error snippet. Also shows inline character-level diffs for
  failed `[ A = B ]` assertions.
- `src/output.rs` - ANSI-colored terminal output for PASS/FAIL and summary.

## Key dependencies

- `brush-parser` - Tokenizes and parses shell scripts into an AST
- `clap` - CLI argument parsing
- `tempfile` - Per-test temporary directories

## Test file format

Standalone test files (`.test`) have any shell shebang and contain functions
prefixed with `test`. Test functions can also be inline in any regular shell
script. When scanning directories, all shell scripts (identified by extension or
shebang) are checked for test functions. Every command in a test function is an
implicit assertion - if it exits nonzero, the test fails. Non-test functions
(helpers/setup) are also extracted and made available to tests.

## CLI options

- `--parallel N` — max concurrent tests (default: CPU cores)
- `--timeout SECS` — wall-clock timeout per test; timed-out tests show `TIME`
  and count as failures
- `--bail` — stop after first failure
- `--filter [FILE/]PATTERN` — run only matching tests (`*` wildcards, prefix
  match)
- `--override SPEC` — copy a binary into the test context `bin/` dir so tests
  use it exclusively. SPEC is either a path (`/usr/bin/example` or
  `./bin/example`) or a mapping (`example=/usr/bin/override`)
- `--strace CMD` — wrap CMD with strace, output saved to `strace/CMD.log` in the
  test context dir
- `--xtrace` — stream xtrace output live (one test at a time)
- `--save-context DIR` — save test context to DIR for debugging

## Building and running

Use `cargo run --` to execute the project.
