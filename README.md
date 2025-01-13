# erudite

A library for running sandboxed tests in parallel in any language.

## Usage

```rust
let results = erudite::Runner::new()
    .compile_command(["rustc", "solution.rs"]);
    .run_command(["./solution"])
    .test("hello", "olleh")
    .test("hello world", "dlrow olleh")
    .timeout(Duration::from_millis(200))
    .create_file("solution.rs", r#"
        use std::io::{self, BufRead, BufReader};
        fn main() {
            println!("{}", BufRead::lines(BufReader::new(io::stdin()))
                    .next()
                    .unwrap()
                    .unwrap()
                    .chars()
                    .rev()
                    .collect::<String>());
        }
    "#)
    .run()
    .await?;
```
