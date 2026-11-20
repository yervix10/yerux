fn main() {
    let n: i64 = 1000000;
    let (mut a, mut b): (Vec<f64>, Vec<f64>) = (Vec::new(), Vec::new());
    let mut i = 0i64;
    while i < n { a.push((i % 100) as f64 * 0.5); b.push(((i * 7) % 13) as f64 * 0.25); i += 1; }
    let mut grand = 0.0f64;
    let mut r = 0i64;
    while r < 200 {
        a[0] = r as f64;
        let mut acc = 0.0f64;
        i = 0;
        while i < n { acc = acc + a[i as usize] * b[i as usize]; i += 1; }
        grand = grand + acc * 0.001;
        r += 1;
    }
    println!("{}", grand as i64);
}
