fn pass(src: &Vec<i64>, dst: &mut Vec<i64>, n: i64, w: i64) {
    let mut lo = 0i64;
    while lo < n {
        let mut mid = lo + w; if mid > n { mid = n; }
        let mut hi = mid + w; if hi > n { hi = n; }
        let (mut i, mut j, mut k) = (lo, mid, lo);
        while i < mid && j < hi {
            let a = src[i as usize]; let b = src[j as usize];
            if a <= b { dst[k as usize] = a; i += 1; } else { dst[k as usize] = b; j += 1; }
            k += 1;
        }
        while i < mid { dst[k as usize] = src[i as usize]; i += 1; k += 1; }
        while j < hi { dst[k as usize] = src[j as usize]; j += 1; k += 1; }
        lo = lo + w + w;
    }
}
fn checksum(l: &Vec<i64>) -> i64 {
    let (mut s, mut i) = (0i64, 0i64);
    while i < l.len() as i64 { s += l[i as usize] * (i % 10); i += 1; }
    s
}
fn main() {
    let n: i64 = 200000;
    let (mut a, mut b): (Vec<i64>, Vec<i64>) = (Vec::new(), Vec::new());
    let (mut x, mut grand, mut round) = (12345i64, 0i64, 0);
    while round < 10 {
        a.clear(); b.clear();
        let mut i = 0i64;
        while i < n { x = (x * 1103515245 + 12345) % 2147483648; a.push(x); b.push(0); i += 1; }
        let (mut w, mut flip) = (1i64, 0);
        while w < n {
            if flip == 0 { pass(&a, &mut b, n, w); flip = 1; } else { pass(&b, &mut a, n, w); flip = 0; }
            w *= 2;
        }
        let r = if flip == 0 { &a } else { &b };
        grand = (grand + r[0] + r[(n - 1) as usize] + checksum(r)) % 1000000007;
        round += 1;
    }
    println!("{}", grand);
}
