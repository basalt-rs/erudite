fn main() {
    let x = 0;
    println!(
        "{}",
        std::io::stdin()
            .lines()
            .next()
            .unwrap()
            .unwrap()
            .chars()
            .rev()
            .collect::<String>()
    );
}
