use std::time::Duration;

use erudite::{RunOutput, Runner, SimpleOutput, TestFailReason, TestOutput};
use leucite::Rules;

#[tokio::test]
async fn malicious_fail() -> anyhow::Result<()> {
    let mut runner = Runner::new();
    let rules = Rules::new()
        .add_read_only("/usr")
        .add_read_only("/etc")
        .add_read_only("/bin");
    runner
        .run_command(["node", "malicious.js"])
        .test("hello", "olleh")
        .create_file("malicious.js", include_str!("./malicious.js"))
        .run_rules(rules)
        .timeout(Duration::from_secs(10));
    dbg!(&runner);

    let results = runner.run().await?;

    dbg!(&results);

    assert!(matches!(results, RunOutput::RunSuccess(_)));
    let RunOutput::RunSuccess(results) = results else {
        unreachable!()
    };

    assert_eq!(1, results.len());

    match &results[0] {
        TestOutput::Fail(TestFailReason::Crash(SimpleOutput {
            stdout,
            stderr,
            status: 1,
        })) if stdout.len() == 0
            && stderr
                .str()
                .is_some_and(|s| s.contains("permission denied")) => {}
        _ => {
            panic!("Incorrect output from command")
        }
    }

    Ok(())
}
