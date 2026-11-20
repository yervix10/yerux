fn main() {
    let n: i64 = 10000000;
    let mut f: Vec<bool> = Vec::new();
    let mut i: i64 = 0;
    while i <= n { f.push(true); i += 1; }
    f[0] = false; f[1] = false;
    i = 2;
    while i * i <= n {
        if f[i as usize] { let mut j = i * i; while j <= n { f[j as usize] = false; j += i; } }
        i += 1;
    }
    let (mut count, mut last) = (0i64, 0i64);
    i = 0;
    while i <= n { if f[i as usize] { count += 1; last = i; } i += 1; }
    println!("{}\n{}", count, last);
}
