use std::{error::Error, path::Path, sync::Arc, time::Duration};

use erudite::{context::TestContext, runner::TestFileContent};
use leucite::Rules;
use regex::Regex;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    let rules = Rules::new()
        .add_read_only("/usr")
        .add_read_only("/etc")
        .add_read_only("/dev")
        .add_read_only("/bin")
        .add_bind_port(5050)
        .add_connect_port(80)
        .add_connect_port(443);

    let context = TestContext::builder()
        .test("hello world", "dlrow olleh", true)
        .test("foo", Regex::new(r"\d+").unwrap(), true)
        .test("hello", "olleh", true)
        // .file(FileContent::path("examples/runner.rs"), "./runner.rs")
        .compile_command(["rustc", "--color=always", "-o", "solution", "solution.rs"])
        .run_command(["./solution"])
        .timeout(Duration::from_secs(10))
        .run_rules(rules.clone())
        .compile_rules(rules.add_read_write("/tmp"))
        .build();

    dbg!(&context);

    let (compile_output, mut tests) = Arc::new(context)
        .test_runner()
        .file(
            TestFileContent::string(include_str!("./solution.rs")),
            Path::new("./solution.rs"),
        )
        .filter_tests(|test| *test.data())
        .compile_and_spawn_runner()
        .await?;

    dbg!(&compile_output);

    if let Some(compile_output) = compile_output {
        eprintln!("STDOUT:\n{}", compile_output.stdout().to_str_lossy());
        eprintln!("STDERR:\n{}", compile_output.stderr().to_str_lossy());
        eprintln!("STATUS: {}", compile_output.exit_status());
    }

    while let Some(test) = tests.wait_next().await? {
        info!("tests[{}] = {:?}", test.index(), test);
    }

    Ok(())
}
