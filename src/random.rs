//! The arithmetic of random draws. A draw depends only on the seed, on where
//! it's written and on the period it's drawn for, so a seed reproduces a run
//! however often, and in whatever order, the model is evaluated.

/// SplitMix64's finalizer: spreads every bit of `z` over the result.
pub fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// `a` and `b` mixed into one number.
pub fn mix(a: u64, b: u64) -> u64 {
    splitmix64(a ^ splitmix64(b))
}

/// FNV-1a, which, unlike std's hasher, gives the same hash in every Rust
/// release.
pub fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// A number strictly between 0 and 1, from the top 52 bits of `bits`: the
/// middle of one of 2^52 equal slices. (With 53 bits, the top slice's middle
/// would round to 1.)
pub fn unit_interval(bits: u64) -> f64 {
    ((bits >> 12) as f64 + 0.5) / (1u64 << 52) as f64
}

/// The number of standard deviations from the mean below which a normal
/// distribution has `p` of its mass, for `p` strictly between 0 and 1. This is
/// Acklam's approximation, whose relative error is under 1.2e-9.
pub fn normal_quantile(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_69e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    const LOW: f64 = 0.024_25;
    // The tails, below `LOW` and above `1 - LOW`, are mirror images.
    let tail = |q: f64| {
        let q = (-2.0 * q.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    };
    if p < LOW {
        tail(p)
    } else if p > 1.0 - LOW {
        -tail(1.0 - p)
    } else {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    }
}

/// The number of a lognormal distribution with `mean` and standard deviation
/// `sd` that's `z` standard deviations from the mean of its log. `mean` is
/// positive.
pub fn lognormal(mean: f64, sd: f64, z: f64) -> f64 {
    let variance = (1.0 + (sd / mean).powi(2)).ln();
    (mean.ln() - variance / 2.0 + variance.sqrt() * z).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_quantiles_match_known_values() {
        // From Python's `statistics.NormalDist().inv_cdf`.
        for (p, z) in [
            (0.5, 0.0),
            (0.841_344_746_068_542_9, 1.0),
            (0.975, 1.959_963_984_540_053_6),
            (0.01, -2.326_347_874_040_840_8),
            (0.001, -3.090_232_306_167_813),
            (0.999_999, 4.753_424_308_817_089),
            (1e-12, -7.034_483_825_301_132),
        ] {
            let found = normal_quantile(p);
            assert!(
                (found - z).abs() <= 1.2e-9 * z.abs().max(1.0),
                "quantile of {p}: {found}, not {z}"
            );
        }
    }

    #[test]
    fn unit_interval_stays_inside() {
        assert!(unit_interval(0) > 0.0);
        assert!(unit_interval(u64::MAX) < 1.0);
    }

    #[test]
    fn hashes_dont_change() {
        // FNV-1a's published test vectors.
        assert_eq!(hash(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(hash(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(splitmix64(0), 0xe220_a839_7b1d_cdaf);
    }
}
