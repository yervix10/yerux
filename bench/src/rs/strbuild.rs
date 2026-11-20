fn push_int(s: &mut Vec<u8>, n: i64) {
    let mut buf = [0u8; 24]; let mut k = 0;
    let mut u = n.unsigned_abs();
    loop { buf[k] = b'0' + (u % 10) as u8; k += 1; u /= 10; if u == 0 { break; } }
    if n < 0 { s.push(b'-'); }
    while k > 0 { k -= 1; s.push(buf[k]); }
}
fn main() {
    let mut s: Vec<u8> = Vec::new();
    let mut i: i64 = 0;
    while i < 8000000 { s.extend_from_slice(b"ab"); push_int(&mut s, i); i += 1; }
    let (mut sum, mut j) = (0i64, 0usize);
    while j < s.len() { sum += s[j] as i64; j += 1; }
    println!("{}\n{}", s.len(), sum);
}
