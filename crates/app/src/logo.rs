//! Application icon, drawn procedurally: rounded square, a curved "reroute"
//! arrow (the chosen route) and a dimmed straight route ending in a stop bar
//! (the ignored default route). Works at any size (anti-aliased SDF).

fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn sd_seg(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (pax, pay) = (px - ax, py - ay);
    let (bax, bay) = (bx - ax, by - ay);
    let h = clamp01((pax * bax + pay * bay) / (bax * bax + bay * bay));
    (pax - bax * h).hypot(pay - bay * h)
}

fn sd_rbox(px: f32, py: f32, r: f32) -> f32 {
    let qx = (px - 0.5).abs() - (0.5 - r);
    let qy = (py - 0.5).abs() - (0.5 - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

type V = (f32, f32);

fn sd_tri(p: V, p0: V, p1: V, p2: V) -> f32 {
    let sub = |a: V, b: V| (a.0 - b.0, a.1 - b.1);
    let dot = |a: V, b: V| a.0 * b.0 + a.1 * b.1;
    let (e0, e1, e2) = (sub(p1, p0), sub(p2, p1), sub(p0, p2));
    let (v0, v1, v2) = (sub(p, p0), sub(p, p1), sub(p, p2));
    let pq = |v: V, e: V| {
        let t = clamp01(dot(v, e) / dot(e, e));
        (v.0 - e.0 * t, v.1 - e.1 * t)
    };
    let (q0, q1, q2) = (pq(v0, e0), pq(v1, e1), pq(v2, e2));
    let s = if e0.0 * e2.1 - e0.1 * e2.0 >= 0.0 { 1.0 } else { -1.0 };
    let c0 = (dot(q0, q0), s * (v0.0 * e0.1 - v0.1 * e0.0));
    let c1 = (dot(q1, q1), s * (v1.0 * e1.1 - v1.1 * e1.0));
    let c2 = (dot(q2, q2), s * (v2.0 * e2.1 - v2.1 * e2.0));
    let dx = c0.0.min(c1.0).min(c2.0);
    let dy = c0.1.min(c1.1).min(c2.1);
    -dx.sqrt() * if dy >= 0.0 { 1.0 } else { -1.0 }
}

/// Coverage of a shape from its signed distance (anti-aliasing).
fn cov(d: f32, aa: f32) -> f32 {
    clamp01(0.5 - d / aa)
}

/// RGBA pixels, `size` x `size`.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size * size * 4) as usize];
    let n = size as f32;
    let aa = 1.0 / n;

    for y in 0..size {
        for x in 0..size {
            let fx = (x as f32 + 0.5) / n;
            let fy = (y as f32 + 0.5) / n;

            let a_bg = cov(sd_rbox(fx, fy, 0.22), aa);
            if a_bg <= 0.0 {
                continue;
            }
            let mut col = mix([46.0, 54.0, 100.0], [20.0, 23.0, 46.0], fy);

            // dimmed default route with a stop bar
            let d = (sd_seg(fx, fy, 0.24, 0.74, 0.76, 0.74) - 0.030)
                .min(sd_seg(fx, fy, 0.82, 0.65, 0.82, 0.83) - 0.030);
            col = mix(col, [84.0, 92.0, 140.0], cov(d, aa) * 0.9);

            // chosen route: up, right, arrow head
            let mut d = sd_seg(fx, fy, 0.24, 0.74, 0.24, 0.36)
                .min(sd_seg(fx, fy, 0.24, 0.36, 0.62, 0.36))
                - 0.042;
            d = d.min(sd_tri((fx, fy), (0.57, 0.19), (0.57, 0.53), (0.85, 0.36)));
            let t = clamp01((fx - 0.24) / 0.61);
            let c_main = mix([112.0, 152.0, 255.0], [96.0, 228.0, 255.0], t);
            col = mix(col, c_main, cov(d, aa));

            // start node
            let d = (fx - 0.24).hypot(fy - 0.74) - 0.068;
            col = mix(col, [238.0, 244.0, 255.0], cov(d, aa));

            let i = ((y * size + x) * 4) as usize;
            out[i] = col[0] as u8;
            out[i + 1] = col[1] as u8;
            out[i + 2] = col[2] as u8;
            out[i + 3] = (a_bg * 255.0) as u8;
        }
    }
    out
}
