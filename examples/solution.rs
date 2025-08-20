fn main() {
    let line = std::io::stdin().lines().next().unwrap().unwrap();
    eprintln!("length of line: {}", line.len());
    println!("{}", line.chars().rev().collect::<String>());
}
