//! Pure scoring / forecasting over a window of values — no I/O, no async,
//! unit-tested on synthetic series. The nodes only add plumbing (series
//! windows, persistence, ports) around these functions.

use augurs::ets::AutoETS;
use augurs::mstl::MSTLModel;
use augurs::prelude::*;

/// MAD → standard deviation for normally distributed data.
const MAD_TO_SIGMA: f64 = 1.4826;

/// Scoring result for one sample.
#[derive(Debug, Clone, PartialEq)]
pub struct Score {
    /// Method-specific score: robust z, or distance to the band edge in
    /// half-widths (|score| ≥ 1 = outside the band).
    pub score: f64,
    pub anomaly: bool,
    pub expected: f64,
    pub lower: f64,
    pub upper: f64,
}

/// One forecast step.
#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    pub mean: f64,
    pub lower: f64,
    pub upper: f64,
}

pub fn median(values: &[f64]) -> f64 {
    let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Robust scale (MAD × 1.4826), falling back to the standard deviation on
/// quantized series where more than half the samples are equal (MAD = 0),
/// then to a tiny relative epsilon on a perfectly flat series.
pub fn robust_scale(values: &[f64], med: f64) -> f64 {
    let dev: Vec<f64> = values.iter().map(|x| (x - med).abs()).collect();
    let mad = median(&dev) * MAD_TO_SIGMA;
    if mad > 0.0 {
        return mad;
    }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let sd = (values.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n).sqrt();
    if sd > 0.0 {
        return sd;
    }
    1e-9 * med.abs().max(1.0)
}

/// Robust z-score of `x` against `history` (median / MAD).
pub fn robust_z(history: &[f64], x: f64, threshold: f64) -> Score {
    let med = median(history);
    let scale = robust_scale(history, med);
    let z = (x - med) / scale;
    Score {
        score: z,
        anomaly: z.abs() >= threshold,
        expected: med,
        lower: med - threshold * scale,
        upper: med + threshold * scale,
    }
}

/// Forecasts `horizon` steps after `history` with prediction intervals at
/// `level`: AutoETS, preceded by an MSTL decomposition when `season` > 0.
pub fn forecast_ets(
    history: &[f64],
    horizon: usize,
    level: f64,
    season: usize,
) -> Result<Vec<Point>, String> {
    let f = if season > 0 {
        let trend = AutoETS::non_seasonal().into_trend_model();
        MSTLModel::new(vec![season], trend)
            .fit(history)
            .map_err(|e| format!("mstl fit: {e}"))?
            .predict(horizon, level)
            .map_err(|e| format!("mstl predict: {e}"))?
    } else {
        AutoETS::non_seasonal()
            .fit(history)
            .map_err(|e| format!("ets fit: {e}"))?
            .predict(horizon, level)
            .map_err(|e| format!("ets predict: {e}"))?
    };
    Ok(points_of(f.point, f.intervals.map(|i| (i.lower, i.upper))))
}

fn points_of(point: Vec<f64>, intervals: Option<(Vec<f64>, Vec<f64>)>) -> Vec<Point> {
    match intervals {
        Some((lo, up)) => point
            .into_iter()
            .zip(lo.into_iter().zip(up))
            .map(|(mean, (lower, upper))| Point { mean, lower, upper })
            .collect(),
        None => point
            .into_iter()
            .map(|mean| Point {
                mean,
                lower: mean,
                upper: mean,
            })
            .collect(),
    }
}

/// Least-squares linear trend over the sample index, with the classic
/// prediction interval `ŷ ± z·s·√(1 + 1/n + (x₀ − x̄)²/Sxx)`.
pub fn forecast_linear(history: &[f64], horizon: usize, level: f64) -> Result<Vec<Point>, String> {
    let n = history.len();
    if n < 3 {
        return Err("linear trend needs at least 3 samples".into());
    }
    let nf = n as f64;
    let x_mean = (nf - 1.0) / 2.0;
    let y_mean = history.iter().sum::<f64>() / nf;
    let (mut sxx, mut sxy) = (0.0, 0.0);
    for (i, y) in history.iter().enumerate() {
        let dx = i as f64 - x_mean;
        sxx += dx * dx;
        sxy += dx * (y - y_mean);
    }
    let slope = sxy / sxx;
    let intercept = y_mean - slope * x_mean;
    let sse: f64 = history
        .iter()
        .enumerate()
        .map(|(i, y)| (y - (intercept + slope * i as f64)).powi(2))
        .sum();
    let s = (sse / (nf - 2.0)).sqrt();
    let z = normal_quantile((1.0 + level) / 2.0);
    Ok((1..=horizon)
        .map(|h| {
            let x0 = (n - 1 + h) as f64;
            let mean = intercept + slope * x0;
            let half = z * s * (1.0 + 1.0 / nf + (x0 - x_mean).powi(2) / sxx).sqrt();
            Point {
                mean,
                lower: mean - half,
                upper: mean + half,
            }
        })
        .collect())
}

/// One-step-ahead band check: `x` is anomalous when it falls outside the
/// forecast interval fitted on `history`.
pub fn forecast_band(history: &[f64], x: f64, level: f64, season: usize) -> Result<Score, String> {
    let p = forecast_ets(history, 1, level, season)?
        .into_iter()
        .next()
        .ok_or("empty forecast")?;
    let half = ((p.upper - p.lower) / 2.0).max(f64::EPSILON);
    let score = (x - p.mean) / half;
    Ok(Score {
        score,
        anomaly: x < p.lower || x > p.upper,
        expected: p.mean,
        lower: p.lower,
        upper: p.upper,
    })
}

/// Changepoint indices of `values`: Bayesian online changepoint detection
/// (Adams & MacKay 2007) with a constant hazard `1/hazard` and a
/// Normal-Gamma prior on the robustly standardized series, then the MAP
/// run-length path walked back from the end. Index 0 (the start of the
/// first run) is dropped. In-house rather than augurs' `changepoint`
/// feature, whose dependency chain (argmin, peroxide, slog, bincode 1) is
/// far heavier than these few lines.
pub fn changepoints(values: &[f64], hazard: f64) -> Vec<usize> {
    let med = median(values);
    let scale = robust_scale(values, med);
    let h = 1.0 / hazard.max(2.0);
    // Posterior parameters and probability per run length (index = run length).
    let (mu0, kappa0, alpha0, beta0) = (0.0, 1.0, 1.0, 1.0);
    let mut mu = vec![mu0];
    let mut kappa = vec![kappa0];
    let mut alpha = vec![alpha0];
    let mut beta = vec![beta0];
    let mut prob = vec![1.0];
    let mut map_run = Vec::with_capacity(values.len());
    for x in values.iter().map(|x| (x - med) / scale) {
        let pred: Vec<f64> = (0..prob.len())
            .map(|r| {
                let var = beta[r] * (kappa[r] + 1.0) / (alpha[r] * kappa[r]);
                student_t_pdf(x, 2.0 * alpha[r], mu[r], var.sqrt())
            })
            .collect();
        let mut next = Vec::with_capacity(prob.len() + 1);
        let cp_mass: f64 = prob.iter().zip(&pred).map(|(p, q)| p * q * h).sum();
        next.push(cp_mass);
        next.extend(prob.iter().zip(&pred).map(|(p, q)| p * q * (1.0 - h)));
        let total: f64 = next.iter().sum();
        if !(total.is_finite() && total > 0.0) {
            // Numerical underflow on an extreme sample: restart the run.
            next = vec![1.0];
        } else {
            next.iter_mut().for_each(|p| *p /= total);
        }
        // Posterior update: run length r+1 extends run r with x; r = 0 is the prior.
        let mut mu2 = vec![mu0];
        let mut kappa2 = vec![kappa0];
        let mut alpha2 = vec![alpha0];
        let mut beta2 = vec![beta0];
        for r in 0..mu.len().min(next.len() - 1) {
            mu2.push((kappa[r] * mu[r] + x) / (kappa[r] + 1.0));
            kappa2.push(kappa[r] + 1.0);
            alpha2.push(alpha[r] + 0.5);
            beta2.push(beta[r] + kappa[r] * (x - mu[r]).powi(2) / (2.0 * (kappa[r] + 1.0)));
        }
        // Truncate negligible long runs (bounded memory and time).
        while next.len() > 2 && next.last().is_some_and(|p| *p < 1e-9) {
            next.pop();
        }
        let keep = next.len();
        mu2.truncate(keep);
        kappa2.truncate(keep);
        alpha2.truncate(keep);
        beta2.truncate(keep);
        (mu, kappa, alpha, beta, prob) = (mu2, kappa2, alpha2, beta2, next);
        let best = prob
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(r, _)| r)
            .unwrap_or(0);
        map_run.push(best);
    }
    // Walk the MAP run lengths back: each run starts at t - run_length(t).
    let mut cps = Vec::new();
    let mut t = values.len();
    while t > 0 {
        let start = (t - 1).saturating_sub(map_run[t - 1]);
        if start > 0 {
            cps.push(start);
        }
        t = start;
    }
    cps.reverse();
    cps
}

/// Student-t density with `df` degrees of freedom, location `loc`, scale `scale`.
fn student_t_pdf(x: f64, df: f64, loc: f64, scale: f64) -> f64 {
    let z = (x - loc) / scale;
    let ln = ln_gamma((df + 1.0) / 2.0)
        - ln_gamma(df / 2.0)
        - 0.5 * (df * std::f64::consts::PI).ln()
        - scale.ln()
        - (df + 1.0) / 2.0 * (1.0 + z * z / df).ln();
    ln.exp()
}

/// ln Γ(x) for x > 0 (Lanczos, g = 7, n = 9 — ~15 significant digits).
fn ln_gamma(x: f64) -> f64 {
    const G: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        // Reflection formula.
        let pi = std::f64::consts::PI;
        return (pi / (pi * x).sin()).ln() - ln_gamma(1.0 - x);
    }
    let x = x - 1.0;
    let mut a = G[0];
    let t = x + 7.5;
    for (i, g) in G.iter().enumerate().skip(1) {
        a += g / (x + i as f64);
    }
    0.5 * (2.0 * std::f64::consts::PI).ln() + (x + 0.5) * t.ln() - t + a.ln()
}

/// First step (1-based) whose value is on the breach side of `threshold`.
pub fn first_breach(
    values: impl IntoIterator<Item = f64>,
    breaches: impl Fn(f64) -> bool,
) -> Option<usize> {
    values.into_iter().position(breaches).map(|i| i + 1)
}

/// Median spacing between consecutive timestamps (seconds), `None` with
/// fewer than two samples or a degenerate clock.
pub fn median_step(times: &[f64]) -> Option<f64> {
    if times.len() < 2 {
        return None;
    }
    let diffs: Vec<f64> = times
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|d| *d > 0.0)
        .collect();
    let m = median(&diffs);
    (m.is_finite() && m > 0.0).then_some(m)
}

/// Inverse of the standard normal CDF (Acklam's rational approximation,
/// relative error < 1.2e-9) — enough for prediction-interval widths.
pub fn normal_quantile(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969683028665376e1,
        2.209460984245205e2,
        -2.759285104469687e2,
        1.38357751867269e2,
        -3.066479806614716e1,
        2.506628277459239,
    ];
    const B: [f64; 5] = [
        -5.447609879822406e1,
        1.615858368580409e2,
        -1.556989798598866e2,
        6.680131188771972e1,
        -1.328068155288572e1,
    ];
    const C: [f64; 6] = [
        -7.784894002430293e-3,
        -3.223964580411365e-1,
        -2.400758277161838,
        -2.549732539343734,
        4.374664141464968,
        2.938163982698783,
    ];
    const D: [f64; 4] = [
        7.784695709041462e-3,
        3.224671290700398e-1,
        2.445134137142996,
        3.754408661907416,
    ];
    let p = p.clamp(1e-12, 1.0 - 1e-12);
    let tail = |q: f64| {
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    };
    if p < 0.02425 {
        tail((-2.0 * p.ln()).sqrt())
    } else if p > 1.0 - 0.02425 {
        -tail((-2.0 * (1.0 - p).ln()).sqrt())
    } else {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-noise in [-0.5, 0.5).
    fn noise(i: usize) -> f64 {
        (i * 7919 % 97) as f64 / 97.0 - 0.5
    }

    #[test]
    fn quantile_matches_known_values() {
        assert!((normal_quantile(0.975) - 1.959964).abs() < 1e-5);
        assert!((normal_quantile(0.5)).abs() < 1e-9);
        assert!((normal_quantile(0.005) + 2.575829).abs() < 1e-5);
    }

    #[test]
    fn robust_z_flags_spike_not_noise() {
        let h: Vec<f64> = (0..200).map(|i| 20.0 + noise(i)).collect();
        assert!(!robust_z(&h, 20.3, 3.5).anomaly);
        let s = robust_z(&h, 25.0, 3.5);
        assert!(s.anomaly && s.score > 3.5, "{s:?}");
        assert!(s.lower < 20.0 && s.upper > 20.0);
    }

    #[test]
    fn robust_z_survives_flat_and_quantized_series() {
        let flat = vec![5.0; 50];
        assert!(!robust_z(&flat, 5.0, 3.5).anomaly);
        assert!(robust_z(&flat, 6.0, 3.5).anomaly);
        // Mostly zeros (MAD = 0) with occasional ones: std fallback.
        let q: Vec<f64> = (0..60)
            .map(|i| if i % 5 == 0 { 1.0 } else { 0.0 })
            .collect();
        assert!(!robust_z(&q, 1.0, 3.5).anomaly);
    }

    #[test]
    fn band_flags_level_jump() {
        let h: Vec<f64> = (0..150)
            .map(|i| 50.0 + 0.01 * i as f64 + noise(i))
            .collect();
        assert!(!forecast_band(&h, 51.6, 0.99, 0).unwrap().anomaly);
        assert!(forecast_band(&h, 60.0, 0.99, 0).unwrap().anomaly);
    }

    #[test]
    fn band_with_season() {
        let h: Vec<f64> = (0..96)
            .map(|i| 10.0 + 3.0 * (i as f64 * std::f64::consts::TAU / 24.0).sin() + 0.2 * noise(i))
            .collect();
        // Next expected value follows the season (≈ sin(96·2π/24) = 0 → 10).
        let s = forecast_band(&h, 10.0, 0.99, 24).unwrap();
        assert!(!s.anomaly, "{s:?}");
        assert!(forecast_band(&h, 20.0, 0.99, 24).unwrap().anomaly);
    }

    #[test]
    fn linear_forecast_follows_slow_drift() {
        let h: Vec<f64> = (0..500)
            .map(|i| 40.0 + 0.005 * i as f64 + 0.3 * noise(i))
            .collect();
        let f = forecast_linear(&h, 100, 0.95).unwrap();
        let truth = 40.0 + 0.005 * 599.0;
        assert!((f[99].mean - truth).abs() < 0.1, "{:?}", f[99]);
        assert!(f[99].lower < truth && truth < f[99].upper);
        // Intervals widen with the horizon.
        assert!(f[99].upper - f[99].lower > f[0].upper - f[0].lower);
    }

    #[test]
    fn ets_forecast_follows_drift() {
        let h: Vec<f64> = (0..300)
            .map(|i| 40.0 + 0.02 * i as f64 + noise(i))
            .collect();
        let f = forecast_ets(&h, 50, 0.95, 0).unwrap();
        assert_eq!(f.len(), 50);
        let truth = 40.0 + 0.02 * 349.0;
        assert!((f[49].mean - truth).abs() < 1.0, "{:?}", f[49]);
    }

    #[test]
    fn changepoint_detects_level_shift() {
        let mut z: Vec<f64> = (0..200).map(noise).collect();
        for v in z.iter_mut().skip(120) {
            *v += 5.0;
        }
        let cps = changepoints(&z, 250.0);
        assert!(cps.iter().any(|&i| (118..=123).contains(&i)), "{cps:?}");
        let quiet: Vec<f64> = (0..200).map(noise).collect();
        assert!(changepoints(&quiet, 250.0).is_empty());
    }

    #[test]
    fn ln_gamma_matches_factorials() {
        assert!((ln_gamma(5.0) - 24f64.ln()).abs() < 1e-10);
        assert!((ln_gamma(0.5) - std::f64::consts::PI.sqrt().ln()).abs() < 1e-10);
    }

    #[test]
    fn changepoint_detects_variance_and_multiple_shifts() {
        let mut z: Vec<f64> = (0..300).map(noise).collect();
        for v in z.iter_mut().skip(100) {
            *v += 4.0;
        }
        for v in z.iter_mut().skip(200) {
            *v -= 8.0;
        }
        let cps = changepoints(&z, 250.0);
        assert!(cps.iter().any(|&i| (98..=103).contains(&i)), "{cps:?}");
        assert!(cps.iter().any(|&i| (198..=203).contains(&i)), "{cps:?}");
        assert!(cps.len() <= 3, "{cps:?}");
    }

    #[test]
    fn breach_and_step_helpers() {
        assert_eq!(first_breach([1.0, 2.0, 3.0], |v| v >= 2.5), Some(3));
        assert_eq!(first_breach([1.0, 2.0], |v| v >= 2.5), None);
        assert_eq!(median_step(&[0.0, 10.0, 20.0, 31.0]), Some(10.0));
        assert_eq!(median_step(&[5.0]), None);
    }
}
