use std::hint::black_box;
fn main() {
    let (mut total, mut i) = (0i64, 0i64);
    while i < 6000000 {
        let mut l: Vec<i64> = Vec::new(); let mut k = 0;
        while k < 8 { l.push(i + k); k += 1; }
        let mut s: Vec<u8> = Vec::new();
        s.extend_from_slice(b"item-");
        let mut n = i; let mut buf = [0u8; 24]; let mut c = 0;
        loop { buf[c] = b'0' + (n % 10) as u8; c += 1; n /= 10; if n == 0 { break; } }
        while c > 0 { c -= 1; s.push(buf[c]); }
        let (l, s) = (black_box(l), black_box(s));
        total = (total + l[7] + l.len() as i64 + s.len() as i64 + s[5] as i64) % 1000000007;
        i += 1;
    }
    println!("{}", total);
}
