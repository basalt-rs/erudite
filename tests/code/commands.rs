use std::io::Write;

fn main() {
    let line = std::io::stdin().lines().next().unwrap().unwrap();
    let (cmd, rest) = if let Some(space) = line.find(char::is_whitespace) {
        (&line[..space], &line[space + 1..])
    } else {
        (&*line, "")
    };
    match (cmd, rest) {
        ("non-utf8", _) => std::io::stdout()
            .write_all(&[0xf0, 0x28, 0x8c, 0x28])
            .unwrap(),
        ("echo", rest) => std::io::stdout().write_all(rest.as_bytes()).unwrap(),
        _ => panic!("unexpected command '{line}'"),
    }
}
