fn main() {
    let n: i64 = 300;
    let (mut a, mut b, mut c): (Vec<i64>, Vec<i64>, Vec<i64>) = (Vec::new(), Vec::new(), Vec::new());
    let mut i: i64 = 0;
    while i < n { let mut j = 0; while j < n { a.push((i * j + 1) % 10); b.push((i + j) % 10); c.push(0); j += 1; } i += 1; }
    let (mut grand, mut r) = (0i64, 0i64);
    while r < 5 {
        a[0] = r;
        i = 0;
        while i < n { let mut j = 0; while j < n {
            let (mut acc, mut k) = (0i64, 0i64);
            while k < n { acc += a[(i * n + k) as usize] * b[(k * n + j) as usize]; k += 1; }
            c[(i * n + j) as usize] = acc; j += 1; } i += 1; }
        i = 0;
        while i < n * n { grand += c[i as usize] * (i % 7); i += 1; }
        r += 1;
    }
    println!("{}", grand);
}
