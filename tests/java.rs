use std::time::Duration;

use erudite::{CommandConfig, RunOutput, Runner, SimpleOutput, TestFailReason, TestOutput};
use leucite::MemorySize;

#[tokio::test]
async fn java_success() -> anyhow::Result<()> {
    let mut runner = Runner::new();
    runner
        .compile_command(["javac", "Solution.java"])
        .run_command(["java", "Solution"])
        .test("hello", "olleh")
        .test("hello world", "dlrow olleh")
        .test("foo bar 2", "2 rab oof")
        .max_memory(CommandConfig::both(MemorySize::from_mb(800)))
        .create_file("Solution.java", include_str!("./Solution.java"))
        .timeout(Duration::from_millis(1000));
    dbg!(&runner);

    let results = runner.run().await?;

    dbg!(&results);

    assert_eq!(
        results,
        RunOutput::RunSuccess(vec![
            TestOutput::Pass,
            TestOutput::Fail(TestFailReason::IncorrectOutput(SimpleOutput {
                stdout: "hello world\n".to_string().into(),
                stderr: String::new().into(),
                status: 0
            })),
            TestOutput::Fail(TestFailReason::Timeout)
        ])
    );

    Ok(())
}

#[tokio::test]
async fn java_compile_fail() -> anyhow::Result<()> {
    let mut runner = Runner::new();
    runner
        .compile_command(["javac", "Solution404.java"])
        .run_command(["java", "Solution"])
        .test("hello", "olleh")
        .test("hello world", "dlrow olleh")
        .test("foo bar 2", "2 rab oof")
        .create_file("Solution.java", include_str!("./Solution.java"))
        .timeout(Duration::from_millis(1000));
    dbg!(&runner);

    let results = runner.run().await?;

    dbg!(&results);

    assert!(matches!(results, RunOutput::CompileFail(_)));

    let RunOutput::CompileFail(err) = results else {
        unreachable!()
    };

    assert_eq!(2, err.status, "Incorrect status code (from javac)");

    Ok(())
}
