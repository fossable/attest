**attest** is a dead simple test framework. There is no exotic test syntax to
remember, assertion API, or hidden lifecycle methods to know about. Tests are
just regular shell functions where every line is an assertion.

We already have all of the tools we need to write tests in the shell:

- Shell functions neatly organize tests into runnable units
- Need to compare text? `[` and `[[` have been around for decades
- Need to compare JSON? `jq -c` has you covered.
- Need some test setup/cleanup? Easy with helper functions and traps.

By keeping the framework lightweight, tests are easy to write and quick to
understand, leading to more effective tests.

By writing the tests in a shell scripting language, we operate at a high level
of abstraction.

## Writing tests

Here's an illustrative example of a test for the `md5sum` command:

```sh
## Test the md5sum command with known input/output
testHello() {
	result=$(echo hello | md5sum) # If md5sum exits nonzero, the test fails

	[ "${result}" = "b1946ac92492d2347c6235b4d2611184  -" ] # If the output changes, the test fails
}
```

There are only three implicit pieces of knowledge that you need for writing
tests:

- All test functions are named starting with `test`
- If any command in your function exits nonzero, the whole test fails
- Each test runs in a separate temporary directory

### Inline tests

If you're testing something that's itself a shell script, you can also include
your tests inline with the script. For example:

```sh
#!/bin/bash
## Add two numbers.

echo $(( $1 + $2 ))

## No input fails
testNoInput() {
	! $0
}

## Good input
testGoodInput() {
	result=$($0 1 2)

	[ ${result} -eq 3 ]
}

## Bad input
testBadInput() {
	! $0 1 1.2
}
```

This is nice because the closer the tests are to the code that's being tested,
the more likely they are maintained over time.

### Generating tests with AI

**attest** tests are also easy for AIs to write. Use `attest skill` to print the
a skill which is pre-tuned to produce good tests. Here's an example prompt:

> Write attest-style tests for the `tac` command.

<details>
<summary>Here's the output from Haiku 4.5:</summary>

```sh
#!/usr/bin/env attest

## Basic functionality - reverse lines from a file
testBasicFile() {
	echo -e "line1\nline2\nline3" > input.txt
	result=$(tac input.txt)
	[ "$result" = "$(echo -e "line3\nline2\nline1")" ]
}

## Reverse lines from stdin
testStdin() {
	result=$(echo -e "first\nsecond\nthird" | tac)
	[ "$result" = "$(echo -e "third\nsecond\nfirst")" ]
}

## Single line should remain unchanged
testSingleLine() {
	echo "only line" > input.txt
	result=$(tac input.txt)
	[ "$result" = "only line" ]
}

## Empty file produces no output
testEmptyFile() {
	touch empty.txt
	result=$(tac empty.txt)
	[ -z "$result" ]
}

## Multiple files are concatenated and reversed
testMultipleFiles() {
	echo -e "a\nb" > file1.txt
	echo -e "c\nd" > file2.txt
	result=$(tac file1.txt file2.txt)
	[ "$result" = "$(echo -e "d\nc\nb\na")" ]
}

## Non-existent file produces error
testNonExistentFile() {
	! tac nonexistent.txt 2>/dev/null
}

## Trailing newline handling
testTrailingNewline() {
	printf "line1\nline2\nline3" > input.txt
	result=$(tac input.txt)
	[ "$result" = "$(echo -e "line3\nline2\nline1")" ]
}

## Help flag works
testHelp() {
	tac --help || tac -h
}

## Version flag works
testVersion() {
	tac --version || tac -v
}

## Lines with special characters
testSpecialCharacters() {
	echo -e "hello\$world\nfoo@bar\n#comment" > input.txt
	result=$(tac input.txt)
	echo "$result" | grep -q "^#comment"
	echo "$result" | grep -q "foo@bar"
}

## File with only whitespace lines
testWhitespaceLines() {
	printf "line1\n  \nline3\n" > input.txt
	result=$(tac input.txt)
	[ "$(echo "$result" | wc -l)" -eq 3 ]
}

## Very long lines are handled correctly
testLongLines() {
	long_line=$(printf 'a%.0s' {1..1000})
	echo -e "$long_line\nshort" > input.txt
	result=$(tac input.txt)
	echo "$result" | grep -q "^short$"
}

## Reads from multiple sources correctly
testMixedInput() {
	echo "file line" > file.txt
	echo "stdin line" | tac - file.txt | head -1 | grep -q "file line"
}
```

</details>

AI can generate tests all day, so the important thing is how easy it is for a
human to quickly understand and assess the quality of AI-produced tests.

## Running tests

Now that we have some tests, it's time for the good part.

```sh
# Just run the tests in one file
attest example.test

# Run all tests in this directory
attest .

# Tests run in parallel by default, but can be forced to run sequentially
attest --sequential .
```

Every test runs in a temporary _context directory_ that collects logs and
temporary files created by the test.

## Debugging tests

When a test fails, you can obtain the context directory:

```sh
attest . --results-failed output
```
