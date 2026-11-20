#[derive(Clone, Copy)]
struct P { x: i64, y: i64, vx: i64, vy: i64 }
fn step(p: P) -> P {
    let mut q = p;
    q.x += q.vx; q.y += q.vy; q.vy += 1;
    if q.x > 1000000 { q.vx = 0 - q.vx; }
    if q.x < 0 { q.vx = 0 - q.vx; }
    if q.y > 1000000 { q.vy = 0 - q.vy; }
    if q.y < 0 { q.vy = 0 - q.vy; }
    q
}
fn main() {
    let n: i64 = 1000; let mut ps: Vec<P> = Vec::new(); let mut i: i64 = 0;
    while i < n { ps.push(P { x: i * 997 % 1000000, y: i * 331 % 1000000, vx: i % 17 + 1, vy: i % 13 + 1 }); i += 1; }
    let mut t = 0;
    while t < 100000 { i = 0; while i < n { ps[i as usize] = step(ps[i as usize]); i += 1; } t += 1; }
    let mut sum = 0i64; i = 0;
    while i < n { let p = ps[i as usize]; sum += p.x + p.y * 3 + p.vx * 5 + p.vy * 7; i += 1; }
    println!("{}", sum);
}
