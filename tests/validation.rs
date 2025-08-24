use std::{error::Error, sync::Arc};

use erudite::{error::CompileError, runner::TestResultState, TestContext};
use regex::Regex;

const COMMANDS_SOLUTION: &str = include_str!("./code/commands.rs");

#[tokio::test]
async fn all_invalid() -> Result<(), Box<dyn Error>> {
    let ctx: Arc<_> = TestContext::builder()
        .test("non-utf8", "", ())
        .compile_command(["rustc", "--color=always", "-o", "solution", "solution.rs"])
        .run_command(["./solution"])
        .file(COMMANDS_SOLUTION.as_bytes(), "solution.rs")
        .build()
        .into();

    let handle_res = ctx.test_runner().compile_and_run().await;

    if let Err(CompileError::CompileFail(ref c)) = handle_res {
        eprintln!("COMPILE FAIL");
        eprintln!("Status: {}", c.exit_status());
        eprintln!("STDOUT:\n{}", c.stdout().to_str_lossy());
        eprintln!("STDERR:\n{}", c.stderr().to_str_lossy());
        panic!("compile fail")
    }

    let results = handle_res?.wait_all().await?;

    for r in results {
        assert_eq!(dbg!(r).state(), TestResultState::IncorrectOutput);
    }

    Ok(())
}

#[tokio::test]
async fn all_valid() -> Result<(), Box<dyn Error>> {
    let ctx: Arc<_> = TestContext::builder()
        .test("echo foo bar", "foo bar", ())
        .test("echo 123", Regex::new(r"^\d+$").unwrap(), ())
        .test("echo a2b", Regex::new(r"\d").unwrap(), ())
        .test("echo ab2", Regex::new(r"\d$").unwrap(), ())
        .compile_command(["rustc", "--color=always", "-o", "solution", "solution.rs"])
        .run_command(["./solution"])
        .file(COMMANDS_SOLUTION.as_bytes(), "solution.rs")
        .build()
        .into();

    let handle_res = ctx.test_runner().compile_and_run().await;

    if let Err(CompileError::CompileFail(ref c)) = handle_res {
        eprintln!("COMPILE FAIL");
        eprintln!("Status: {}", c.exit_status());
        eprintln!("STDOUT:\n{}", c.stdout().to_str_lossy());
        eprintln!("STDERR:\n{}", c.stderr().to_str_lossy());
        panic!("compile fail")
    }

    let results = handle_res?.wait_all().await?;

    for r in results {
        assert_eq!(dbg!(r).state(), TestResultState::Pass);
    }

    Ok(())
}
