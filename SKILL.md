# Writing .test files for attest

attest is a shell-based test framework. Test files are plain shell scripts with
a `.test` extension. There is no exotic syntax, assertion API, or lifecycle
methods. Do NOT use syntax from other test frameworks (no `assert_eq`, no
`@test`, no `shunit2` functions, etc.).

## Test functions

- Test functions MUST be prefixed with `test` (e.g., `testFoo`, `testVersion`)
- Every command in a test function is an implicit assertion: if it exits
  nonzero, the test fails immediately (`set -e` semantics)
- Each test runs in its own temporary directory (`$PWD` is a fresh tmpdir)

## Helper functions

- Functions without a `test` prefix are helpers
- Helpers are available to all test functions in the same file
- Use helpers for shared setup, cleanup, or utility logic

```sh
# Assertion helper
who_sanity_check() {
	grep '.'
}

testBasic() {
	who | who_sanity_check
}

testHeading() {
	who --heading | who_sanity_check
}
```

## Common assertion idioms

```sh
# String equality
[ "$output" = "expected" ]

# String inequality
[ "$output" != "unexpected" ]

# String match
[[ "$output" ~= "^[0-9]$" ]]

# File exists
[ -f path/to/file ]

# Directory exists
[ -d path/to/dir ]

# Output contains a pattern
echo "$output" | grep "pattern"

# Output does NOT contain a pattern
! echo "$output" | grep "pattern"

# Numeric comparison
[ "$count" -eq 5 ]

# JSON equality
[ "$(echo "$json" | jq -c)" = '["value"]' ]

# Compare command output to expected
diff <(some_command) <(echo "expected output")

# Exit code checking (when you need to assert failure)
! some_command_that_should_fail

# When something can fail and that's OK
might_fail || true
```

## Setup and cleanup

Use `trap` for cleanup that must run even if the test fails:

```sh
testWithCleanup() {
	background_server &
	trap "kill $!" EXIT

	# ... test logic ...
}
```

Use a helper function for shared setup:

```sh
setup() {
	echo "hello" > input.txt
}

testReadInput() {
	setup
	result=$(cat input.txt)
	[ "$result" = "hello" ]
}
```

## Example: testing a CLI command

```sh
#!/usr/bin/env bash

testHelp() {
	md5sum --help
}

testVersion() {
	md5sum --version
}

## Test known input/output
testHello() {
	result=$(echo hello | md5sum)

	[ "${result}" = "b1946ac92492d2347c6235b4d2611184  -" ]
}
```

## Style guidelines

- Prefer `camelCase` for function names (`testMyFeature`), but underscores are
  OK too
- Use tabs for indentation
- Group related tests in the same `.test` file
- Name files after the command or feature under test (e.g., `md5sum.test`)
- Keep tests focused: one logical check per test function when practical
- Use comments to explain non-obvious assertions, not obvious ones
- Comments on test functions themselves should be "documentation style" with
  `##`.
- Avoid redirecting stderr or stdout to /dev/null because it might be useful for
  debugging.

## General test writing practice

- Don't test trivial behavior
