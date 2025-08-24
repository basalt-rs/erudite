mod solution;
use solution::reverse;

fn main() {
    let line = std::io::stdin().lines().next().unwrap().unwrap();

    println!("{}", reverse(&line));
}
