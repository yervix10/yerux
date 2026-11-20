fn main() {
    let (w, h, maxit) = (800i64, 600i64, 200i64);
    let (mut inside, mut total) = (0i64, 0i64);
    let mut py = 0i64;
    while py < h {
        let ci = py as f64 * (2.4 / 600.0) - 1.2;
        let mut px = 0i64;
        while px < w {
            let cr = px as f64 * (3.2 / 800.0) - 2.2;
            let (mut zr, mut zi, mut it) = (0.0f64, 0.0f64, 0i64);
            while it < maxit && zr * zr + zi * zi <= 4.0 {
                let t = zr * zr - zi * zi + cr;
                zi = 2.0 * zr * zi + ci;
                zr = t;
                it += 1;
            }
            if it == maxit { inside += 1; }
            total += it;
            px += 1;
        }
        py += 1;
    }
    println!("{}\n{}", inside, total);
}
