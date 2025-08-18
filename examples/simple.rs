use std::{error::Error, time::Duration};

use erudite::context::{FileContent, TestContext};
use leucite::Rules;

fn main() -> Result<(), Box<dyn Error>> {
    let rules = Rules::new()
        .add_read_only("/usr")
        .add_read_only("/etc")
        .add_read_only("/dev")
        .add_read_only("/bin")
        .add_read_write("/tmp/foo")
        .add_bind_port(5050)
        .add_connect_port(80)
        .add_connect_port(443);

    let context = TestContext::builder()
        .test("hello", "olleh")
        .test("world", "dlrow")
        .file(FileContent::path("examples/runner.rs"), "./runner.rs")
        .compile_command(["rustc", "-o", "main", "main.rs"])
        .run_command(["./main"])
        .timeout(Duration::from_secs(10))
        .rules(rules)
        .build();

    dbg!(context);

    Ok(())
}
