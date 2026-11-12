//! Cutoff register -> centre frequency, from measured points, interpolated to
//! 2048 entries.

pub struct CutoffTable {
    pub f0: [f64; 2048],
}

/// Cubic Hermite tangents restricted to the monotone region (Fritsch-Carlson).
///
/// The measured points are unevenly spaced and the 6581 curve steps down where
/// the high half of the cutoff register rolls over. Tangents that ignore the
/// spacing let the interpolant overshoot the neighbouring values, which turns a
/// measured step into a bump and breaks monotonicity on both sides of it.
fn tangents(points: &[(u32, f64)]) -> Vec<f64> {
    let n = points.len();
    let mut d = vec![0.0; n];
    if n < 2 {
        return d;
    }
    let secant: Vec<f64> = (0..n - 1)
        .map(|k| {
            let (x1, y1) = points[k];
            let (x2, y2) = points[k + 1];
            (y2 - y1) / f64::from(x2 - x1)
        })
        .collect();

    d[0] = secant[0];
    d[n - 1] = secant[n - 2];
    for k in 1..n - 1 {
        let (m0, m1) = (secant[k - 1], secant[k]);
        d[k] = if m0 * m1 <= 0.0 { 0.0 } else { (m0 + m1) * 0.5 };
    }

    for k in 0..n - 1 {
        let m = secant[k];
        if m == 0.0 {
            d[k] = 0.0;
            d[k + 1] = 0.0;
            continue;
        }
        let a = d[k] / m;
        let b = d[k + 1] / m;
        let s = a * a + b * b;
        if s > 9.0 {
            let scale = 3.0 / s.sqrt();
            d[k] = scale * a * m;
            d[k + 1] = scale * b * m;
        }
    }
    d
}

fn build(points: &[(u32, f64)]) -> CutoffTable {
    let d = tangents(points);
    let mut f0 = [points[0].1; 2048];
    for (i, slot) in f0.iter_mut().enumerate() {
        let x = f64::from(i as u32);
        // Index of the last point at or before x.
        let mut k = 0usize;
        for (j, &(fx, _)) in points.iter().enumerate() {
            if f64::from(fx) <= x {
                k = j;
            } else {
                break;
            }
        }
        let (x1, y1) = points[k];
        let (x2, y2) = points[(k + 1).min(points.len() - 1)];
        if x2 == x1 {
            *slot = y2;
            continue;
        }
        let h = f64::from(x2 - x1);
        let t = ((x - f64::from(x1)) / h).clamp(0.0, 1.0);
        let t2 = t * t;
        let t3 = t2 * t;
        *slot = (2.0 * t3 - 3.0 * t2 + 1.0) * y1
            + (t3 - 2.0 * t2 + t) * h * d[k]
            + (-2.0 * t3 + 3.0 * t2) * y2
            + (t3 - t2) * h * d[k + 1];
    }
    CutoffTable { f0 }
}

#[cfg(feature = "gpl-data")]
const POINTS_6581: &[(u32, f64)] = sid_gpl_data::CUTOFF_POINTS_6581;
#[cfg(feature = "gpl-data")]
const POINTS_8580: &[(u32, f64)] = sid_gpl_data::CUTOFF_POINTS_8580;
#[cfg(not(feature = "gpl-data"))]
const POINTS_6581: &[(u32, f64)] = super::default::CUTOFF_POINTS;
#[cfg(not(feature = "gpl-data"))]
const POINTS_8580: &[(u32, f64)] = super::default::CUTOFF_POINTS;

static CUTOFF_6581: std::sync::OnceLock<CutoffTable> = std::sync::OnceLock::new();
static CUTOFF_8580: std::sync::OnceLock<CutoffTable> = std::sync::OnceLock::new();

/// The 6581 is the modelled chip, so both clock variants share its curve.
pub fn cutoff_table(model: crate::Model) -> &'static CutoffTable {
    match model {
        crate::Model::Pal | crate::Model::Ntsc => CUTOFF_6581.get_or_init(|| build(POINTS_6581)),
    }
}

/// The 8580 curve: the measured 8580 points with `gpl-data`, the built-in
/// linear curve (the same one the 6581 uses) without it. Kept as a reference;
/// nothing in the chip path selects it yet.
pub fn cutoff_table_8580() -> &'static CutoffTable {
    CUTOFF_8580.get_or_init(|| build(POINTS_8580))
}

// The 8580 curve assertions describe the measured data.
#[cfg(all(test, feature = "gpl-data"))]
mod tests {
    use super::cutoff_table_8580;

    #[test]
    fn the_8580_curve_is_the_linear_one() {
        let t = cutoff_table_8580();
        let points = sid_gpl_data::CUTOFF_POINTS_8580;
        let (first, last) = (points[0].1, points[points.len() - 1].1);
        assert!(
            (t.f0[0] - first).abs() < 1e-6,
            "fc=0 gave {}, data says {first}",
            t.f0[0]
        );
        assert!(
            (t.f0[2047] - last).abs() < 1e-6,
            "fc=2047 gave {}, data says {last}",
            t.f0[2047]
        );
    }

    #[test]
    fn the_8580_curve_has_no_step() {
        let t = cutoff_table_8580();
        let mut prev = t.f0[0];
        for (fc, &now) in t.f0.iter().enumerate().skip(1) {
            assert!(now >= prev, "not monotone at {fc}");
            prev = now;
        }
    }
}
