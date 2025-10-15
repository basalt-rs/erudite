<!-- Readme generated with `cargo-readme`: https://github.com/webern/cargo-readme -->

# erudite

[![Crates.io](https://img.shields.io/crates/v/erudite.svg)](https://crates.io/crates/erudite)
[![Documentation](https://docs.rs/erudite/badge.svg)](https://docs.rs/erudite/)
[![Dependency status](https://deps.rs/repo/github/basalt-rs/erudite/status.svg)](https://deps.rs/repo/github/basalt-rs/erudite)

Erudite is an asynchronous test runner that can run a suites of tests in concurrently.

There are a couple of key structures which make up Erudite:

## `TestContext`

A test context holds information about how a test suite should be run.  Information such as
the commands needed to run the tests, the restrictions to apply to said commands, and the
tests themselves are included in a test context.

A test context is designed with the intention that it can be placed into an `Arc` and be used
throughout the program, creating test runners, which can be used to run tests.

In a test context, individual tests are associated via groups.  A group can be anything (with
some restrictions, see `TestContextBuilder::test` for details).

## `TestRunner`

A test runner is created from a test context by selecting a single group and is used to run a
test suite.  There are some additional configurations that can be added to the test runner for
run-specific settings.  Once a runner has been created, it can compile and then run the tests,
giving a handle to those tests.

## `TestHandle`

A test handle is a handle to an actively running suite of tests.  A test handle can wait for
the tests to finish one at a time or for all of them to finish.  Test handles return
`TestResult`s which contain information about the output of a test, the time it took, and
whether that test passed.

## Usage

```rust
let context = TestContext::builder()
    .compile_command(["rustc", "-o", "runner", "runner.rs"])
    .run_command(["./runner"])
    .test("group", "hello", "olleh", ())
    .test("group", "world", "dlrow", ())
    .test("group", "rust", "tsur", ())
    .test("group", "tacocat", "tacocat", ())
    .trim_output(true)
    .file(FileContent::string(runner_code), "runner.rs")
    .build();

let context = Arc::new(context);

let mut handle = context
    .test_runner(&"group")
    .expect("This group was added above")
    .file(BorrowedFileContent::string(solution_code), Path::new("solution.rs"))
    .compile_and_run()
    .await?;

let results = handle.wait_all().await?;

assert_eq!(results[0].state(), TestResultState::Pass);
```
