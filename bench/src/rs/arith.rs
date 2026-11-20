fn main() {
    let (mut i, mut acc): (i64, i64) = (0, 0);
    while i < 100000000 { acc = (acc + i * 3) % 1000003; i += 1; }
    println!("{}", acc);
}
