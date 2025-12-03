use std::{error::Error, path::Path, sync::Arc};

use erudite::{runner::TestResultState, BorrowedFileContent, FileContent, TestContext};

#[tokio::test]
async fn reverse() -> Result<(), Box<dyn Error>> {
    let context = TestContext::builder()
        .compile_command(["rustc", "-o", "runner", "runner.rs"])
        .run_command(["./runner"])
        .test((), "hello", "olleh", ())
        .test((), "world", "dlrow", ())
        .test((), "rust", "tsur", ())
        .test((), "tacocat", "tacocat", ())
        .trim_output(true)
        .file(
            FileContent::string(include_str!("./code/reverse-runner.rs")),
            "runner.rs",
        )
        .build();

    let context = Arc::new(context);

    let mut handle = context
        .default_test_runner()
        .file(
            BorrowedFileContent::string(include_str!("./code/reverse-solution.rs")),
            Path::new("solution.rs"),
        )
        .compile_and_run()
        .await?;

    dbg!(handle.cwd());

    while let Some(result) = handle.wait_next().await.expect("awaiting test") {
        assert_eq!(result.state(), TestResultState::Pass);
    }

    Ok(())
}
