fn energy(x: &[f64], y: &[f64], z: &[f64], vx: &[f64], vy: &[f64], vz: &[f64], m: &[f64]) -> f64 {
    let mut e = 0.0;
    for i in 0..5 {
        e = e + 0.5 * m[i] * (vx[i] * vx[i] + vy[i] * vy[i] + vz[i] * vz[i]);
        for j in i + 1..5 {
            let (dx, dy, dz) = (x[i] - x[j], y[i] - y[j], z[i] - z[j]);
            e = e - m[i] * m[j] / (dx * dx + dy * dy + dz * dz).sqrt();
        }
    }
    e
}
fn main() {
    let (mut x, mut y, mut z, mut vx, mut vy, mut vz, mut m) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for k in 0..5 {
        let f = k as f64;
        x.push(f * 1.5 - 3.0); y.push(f * f * 0.25 - 1.0); z.push(2.0 - f * 0.75);
        vx.push(0.01 * (f - 2.0)); vy.push(0.02 * (1.0 - f)); vz.push(0.005 * f); m.push(1.0 + f * 0.5);
    }
    println!("{}", (energy(&x, &y, &z, &vx, &vy, &vz, &m) * 1000000000.0) as i64);
    let dt = 0.001;
    let mut s = 0i64;
    while s < 2000000 {
        let mut i = 0;
        while i < 5 {
            let mut j = i + 1;
            while j < 5 {
                let (dx, dy, dz) = (x[i] - x[j], y[i] - y[j], z[i] - z[j]);
                let d2 = dx * dx + dy * dy + dz * dz;
                let mag = dt / (d2 * d2.sqrt());
                let (mi, mj) = (m[i] * mag, m[j] * mag);
                vx[i] = vx[i] - dx * mj; vy[i] = vy[i] - dy * mj; vz[i] = vz[i] - dz * mj;
                vx[j] = vx[j] + dx * mi; vy[j] = vy[j] + dy * mi; vz[j] = vz[j] + dz * mi;
                j += 1;
            }
            i += 1;
        }
        i = 0;
        while i < 5 { x[i] = x[i] + dt * vx[i]; y[i] = y[i] + dt * vy[i]; z[i] = z[i] + dt * vz[i]; i += 1; }
        s += 1;
    }
    println!("{}", (energy(&x, &y, &z, &vx, &vy, &vz, &m) * 1000000000.0) as i64);
}
