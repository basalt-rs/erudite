use std::{error::Error, path::Path, sync::Arc, time::Duration};

use erudite::{
    context::TestContext,
    runner::{CompileResultState, TestFileContent, TestResultState},
};
use leucite::{MemorySize, Rules};

#[tokio::test]
async fn java_success() -> Result<(), Box<dyn Error>> {
    let rules = Rules::new()
        .add_read_only("/usr")
        .add_read_only("/etc")
        .add_read_only("/bin");

    let context = TestContext::builder()
        .test("hello", "olleh", ())
        .test("hello world", "dlrow olleh", ())
        .test("foo bar 2", "2 rab oof", ())
        .test("", "", ())
        .compile_command(["javac", "Solution.java"])
        .run_command(["java", "Solution"])
        .rules(rules)
        .max_memory(MemorySize::from_mb(800))
        .timeout(Duration::from_secs(1))
        .build();

    dbg!(&context);
    let context = Arc::new(context);

    let (compile, mut tests) = context
        .test_builder()
        .file(
            TestFileContent::string(include_str!("./Solution.java")),
            Path::new("Solution.java"),
        )
        .collect_output(true)
        .compile_and_spawn_runner()
        .await?;

    assert!(compile.is_some());
    let compile = compile.unwrap();
    eprintln!("COMPILE OUTPUT:");
    eprintln!("Status: {}", compile.exit_status());
    eprintln!("STDOUT:");
    for x in compile.stdout().to_str_lossy().lines() {
        eprintln!("    {x}");
    }
    eprintln!("STDERR:");
    for x in compile.stderr().to_str_lossy().lines() {
        eprintln!("    {x}");
    }

    let results = tests.wait_all().await?;

    dbg!(&results);
    assert_eq!(results[0].state(), TestResultState::Pass);
    assert_eq!(results[1].state(), TestResultState::IncorrectOutput);
    assert_eq!(results[1].stdout().to_str_lossy(), "hello world\n");
    assert_eq!(results[2].state(), TestResultState::TimedOut);

    Ok(())
}

#[tokio::test]
async fn java_compile_fail() -> Result<(), Box<dyn Error>> {
    let rules = Rules::new()
        .add_read_only("/usr")
        .add_read_only("/etc")
        .add_read_only("/bin");

    let context = TestContext::builder()
        .test("hello", "olleh", ())
        .test("hello world", "dlrow olleh", ())
        .test("foo bar 2", "2 rab oof", ())
        .test("", "", ())
        .compile_command(["javac", "Solution404.java"])
        .run_command(["java", "Solution"])
        .rules(rules)
        .max_memory(MemorySize::from_mb(800))
        .timeout(Duration::from_secs(1))
        .build();

    dbg!(&context);
    let context = Arc::new(context);

    let (compile, _tests) = context
        .test_builder()
        .file(
            TestFileContent::string(include_str!("./Solution.java")),
            Path::new("Solution.java"),
        )
        .collect_output(true)
        .compile_and_spawn_runner()
        .await?;

    assert!(compile.is_some());
    let compile = compile.unwrap();
    assert_eq!(compile.state(), CompileResultState::Pass);
    eprintln!("COMPILE OUTPUT:");
    eprintln!("Status: {}", compile.exit_status());
    eprintln!("STDOUT:");
    for x in compile.stdout().to_str_lossy().lines() {
        eprintln!("    {x}");
    }
    eprintln!("STDERR:");
    for x in compile.stderr().to_str_lossy().lines() {
        eprintln!("    {x}");
    }

    Ok(())
}
