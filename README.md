> Now that the age of AI programming is undeniably here, guess what just got
> more important... TESTING.

**attest** might be the simplest test framework ever. There is no exotic test
syntax to remember, assertion API, or hidden lifecycle methods to know about.
Tests are just regular shell functions where every line is an assertion.

We already have all of the tools we need in the shell. Need to compare text? `[`
and `[[` have been around for decades. Need to compare JSON? `jq -c` has you
covered.

## Writing tests

Here's an illustrative example of a test for the `md5sum` command:

```sh
## Test the md5sum command with known input/output
testHello() {
	result=$(echo hello | md5sum) # If md5sum exits nonzero, the test fails

	[ "${result}" = "b1946ac92492d2347c6235b4d2611184  -" ] # If the output changes, the test fails
}
```
