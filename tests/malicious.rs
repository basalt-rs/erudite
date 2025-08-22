use std::{error::Error, path::Path, sync::Arc, time::Duration};

use erudite::{
    context::TestContext,
    runner::{TestFileContent, TestResultState},
};
use leucite::Rules;

#[tokio::test]
async fn malicious_fail() -> Result<(), Box<dyn Error>> {
    let rules = Rules::new()
        .add_read_only("/usr")
        .add_read_only("/etc")
        .add_read_only("/bin");

    let context = TestContext::builder()
        .test("foo", "bar", ())
        .run_command(["node", "malicious.js"])
        .rules(rules)
        .timeout(Duration::from_secs(5))
        .build();

    dbg!(&context);
    let context = Arc::new(context);

    let compiled = context
        .test_runner()
        .file(
            TestFileContent::string(include_str!("./malicious.js")),
            Path::new("malicious.js"),
        )
        .collect_output(true)
        .compile()
        .await?;

    // compile == None because there is no compile step
    assert!(compiled.compile_result().is_none());

    let out = compiled.run().wait_all().await?;

    for x in out {
        eprintln!("State: {:?}\n", x.state());
        eprintln!("STDOUT:\n{}\n", x.stdout().to_str_lossy());
        eprintln!("STDERR:\n{}\n", x.stderr().to_str_lossy());
        assert_eq!(x.state(), TestResultState::RuntimeFail);
        assert_eq!(x.exit_status(), 1);
        assert!(x.stdout().is_empty());
        assert!(x.stderr().as_str().unwrap().contains("permission denied"));
    }

    Ok(())
}
