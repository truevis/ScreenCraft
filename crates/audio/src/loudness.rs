//! ITU-R BS.1770-4 / EBU R128 integrated loudness measurement and stereo-linked true-peak
//! limiting for 48 kHz stereo audio.

pub const SAMPLE_RATE: usize = 48_000;
pub const YOUTUBE_LUFS: f64 = -14.0;
pub const PODCAST_LUFS: f64 = -16.0;
pub const BROADCAST_LUFS: f64 = -23.0;
pub const TRUE_PEAK_CEILING_DBTP: f64 = -1.0;
pub const TRUE_PEAK_CEILING_LINEAR: f32 = 0.891_250_94; // 10^(-1.0 / 20.0)
pub const MAX_BOOST_DB: f64 = 24.0;
pub const MAX_CUT_DB: f64 = -36.0;

const SUB_BLOCK_FRAMES: usize = 4_800; // 100 ms at 48 kHz
const GATING_SUB_BLOCKS: usize = 4; // 400 ms = 19,200 frames, 75% overlap
const GATING_FRAMES: usize = SUB_BLOCK_FRAMES * GATING_SUB_BLOCKS;
const ABSOLUTE_GATE_LUFS: f64 = -70.0;

/// Integrated loudness and true-peak statistics of a 48 kHz stereo soundtrack.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoudnessStats {
    pub integrated_lufs: Option<f64>,
    pub true_peak_dbtp: f64,
}

/// Makeup gain in dB required to shift `integrated_lufs` to `target_lufs`, clamped to
/// `[MAX_CUT_DB, MAX_BOOST_DB]`.
pub fn gain_db(integrated_lufs: f64, target_lufs: f64) -> f64 {
    (target_lufs - integrated_lufs).clamp(MAX_CUT_DB, MAX_BOOST_DB)
}

/// Linear makeup gain factor for `integrated_lufs` -> `target_lufs` (`1.0` when silent / `None`).
pub fn makeup_gain(integrated_lufs: Option<f64>, target_lufs: f64) -> f32 {
    match integrated_lufs {
        Some(lufs) if lufs.is_finite() => 10f64.powf(gain_db(lufs, target_lufs) / 20.0) as f32,
        _ => 1.0,
    }
}

#[derive(Clone, Copy)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    fn new(b0: f64, b1: f64, b2: f64, a1: f64, a2: f64) -> Self {
        Self { b0, b1, b2, a1, a2, z1: 0.0, z2: 0.0 }
    }

    #[inline]
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// ITU-R BS.1770-4 K-weighting filter pair at 48 kHz for one channel.
#[derive(Clone, Copy)]
struct KWeightChannel {
    pre: Biquad,
    rlb: Biquad,
}

impl KWeightChannel {
    fn new() -> Self {
        Self {
            // Stage 1: high-frequency shelf pre-filter (exact 48 kHz BS.1770-4 coefficients)
            pre: Biquad::new(
                1.53512485958697,
                -2.69169618940638,
                1.19839281085285,
                -1.69065929318241,
                0.73248077421585,
            ),
            // Stage 2: Revised Low-frequency B-weighting (RLB) high-pass filter
            rlb: Biquad::new(1.0, -2.0, 1.0, -1.99004745483398, 0.99007225036621),
        }
    }

    #[inline]
    fn process(&mut self, sample: f32) -> f64 {
        let s1 = self.pre.process(sample as f64);
        self.rlb.process(s1)
    }
}

#[inline]
fn catmull_rom(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

#[inline]
fn intersample_peak(p0: f32, p1: f32, p2: f32, p3: f32) -> f32 {
    catmull_rom(p0, p1, p2, p3, 0.25)
        .abs()
        .max(catmull_rom(p0, p1, p2, p3, 0.50).abs())
        .max(catmull_rom(p0, p1, p2, p3, 0.75).abs())
}

/// Streaming ITU-R BS.1770-4 / EBU R128 integrated loudness and 4x true-peak meter (48 kHz stereo).
pub struct LoudnessMeter {
    kw_l: KWeightChannel,
    kw_r: KWeightChannel,
    cur_sum_l: f64,
    cur_sum_r: f64,
    cur_frames: usize,
    total_sum_l: f64,
    total_sum_r: f64,
    total_frames: usize,
    ring_l: [f64; GATING_SUB_BLOCKS],
    ring_r: [f64; GATING_SUB_BLOCKS],
    sub_blocks_done: usize,
    blocks: Vec<f64>,
    hist_l: [f32; 3],
    hist_r: [f32; 3],
    max_true_peak: f32,
}

impl Default for LoudnessMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl LoudnessMeter {
    pub fn new() -> Self {
        Self {
            kw_l: KWeightChannel::new(),
            kw_r: KWeightChannel::new(),
            cur_sum_l: 0.0,
            cur_sum_r: 0.0,
            cur_frames: 0,
            total_sum_l: 0.0,
            total_sum_r: 0.0,
            total_frames: 0,
            ring_l: [0.0; GATING_SUB_BLOCKS],
            ring_r: [0.0; GATING_SUB_BLOCKS],
            sub_blocks_done: 0,
            blocks: Vec::new(),
            hist_l: [0.0; 3],
            hist_r: [0.0; 3],
            max_true_peak: 0.0,
        }
    }

    /// Push interleaved 48 kHz stereo samples (`[L, R, L, R, ...]`).
    pub fn push(&mut self, interleaved: &[f32]) {
        for frame in interleaved.chunks_exact(2) {
            let l = frame[0];
            let r = frame[1];
            let kl = self.kw_l.process(l);
            let kr = self.kw_r.process(r);
            let sq_l = kl * kl;
            let sq_r = kr * kr;
            self.cur_sum_l += sq_l;
            self.cur_sum_r += sq_r;
            self.total_sum_l += sq_l;
            self.total_sum_r += sq_r;
            self.cur_frames += 1;
            self.total_frames += 1;

            if self.cur_frames == SUB_BLOCK_FRAMES {
                let slot = self.sub_blocks_done % GATING_SUB_BLOCKS;
                self.ring_l[slot] = self.cur_sum_l;
                self.ring_r[slot] = self.cur_sum_r;
                self.cur_sum_l = 0.0;
                self.cur_sum_r = 0.0;
                self.cur_frames = 0;
                self.sub_blocks_done += 1;
                if self.sub_blocks_done >= GATING_SUB_BLOCKS {
                    let sl: f64 = self.ring_l.iter().sum();
                    let sr: f64 = self.ring_r.iter().sum();
                    let z = (sl + sr) / GATING_FRAMES as f64;
                    self.blocks.push(z);
                }
            }

            let tp_l = l.abs().max(intersample_peak(self.hist_l[0], self.hist_l[1], self.hist_l[2], l));
            let tp_r = r.abs().max(intersample_peak(self.hist_r[0], self.hist_r[1], self.hist_r[2], r));
            self.max_true_peak = self.max_true_peak.max(tp_l).max(tp_r);
            self.hist_l = [self.hist_l[1], self.hist_l[2], l];
            self.hist_r = [self.hist_r[1], self.hist_r[2], r];
        }
    }

    /// Finalize measurement after all audio has been pushed.
    pub fn finish(mut self) -> LoudnessStats {
        // Flush trailing inter-sample intervals with zero padding.
        for _ in 0..2 {
            let tp_l = intersample_peak(self.hist_l[0], self.hist_l[1], self.hist_l[2], 0.0);
            let tp_r = intersample_peak(self.hist_r[0], self.hist_r[1], self.hist_r[2], 0.0);
            self.max_true_peak = self.max_true_peak.max(tp_l).max(tp_r);
            self.hist_l = [self.hist_l[1], self.hist_l[2], 0.0];
            self.hist_r = [self.hist_r[1], self.hist_r[2], 0.0];
        }

        let true_peak_dbtp = if self.max_true_peak > 0.0 {
            20.0 * (self.max_true_peak as f64).log10()
        } else {
            f64::NEG_INFINITY
        };

        // Short-clip fallback when total duration is under one 400 ms gating block.
        if self.blocks.is_empty() && self.total_frames > 0 {
            let z = (self.total_sum_l + self.total_sum_r) / self.total_frames as f64;
            if z > 0.0 {
                let lufs = -0.691 + 10.0 * z.log10();
                if lufs > ABSOLUTE_GATE_LUFS {
                    return LoudnessStats { integrated_lufs: Some(lufs), true_peak_dbtp };
                }
            }
            return LoudnessStats { integrated_lufs: None, true_peak_dbtp };
        }

        // Stage 1: absolute gate at -70 LUFS.
        let abs_gated: Vec<f64> = self
            .blocks
            .iter()
            .copied()
            .filter(|&z| z > 0.0 && (-0.691 + 10.0 * z.log10()) > ABSOLUTE_GATE_LUFS)
            .collect();
        if abs_gated.is_empty() {
            return LoudnessStats { integrated_lufs: None, true_peak_dbtp };
        }

        let abs_mean = abs_gated.iter().sum::<f64>() / abs_gated.len() as f64;
        let rel_gate = -0.691 + 10.0 * abs_mean.log10() - 10.0;

        // Stage 2: relative gate at Gamma_r = L_K_abs - 10 LU.
        let rel_gated: Vec<f64> = abs_gated
            .iter()
            .copied()
            .filter(|&z| (-0.691 + 10.0 * z.log10()) > rel_gate)
            .collect();
        let chosen = if rel_gated.is_empty() { &abs_gated } else { &rel_gated };
        let mean_z = chosen.iter().sum::<f64>() / chosen.len() as f64;
        let integrated_lufs = if mean_z > 0.0 { Some(-0.691 + 10.0 * mean_z.log10()) } else { None };
        LoudnessStats { integrated_lufs, true_peak_dbtp }
    }
}

/// Stereo-linked zero-latency soft-knee true-peak ceiling limiter (48 kHz).
pub struct TruePeakLimiter {
    ceiling: f32,
    knee_start: f32,
    release_coeff: f32,
    gain: f32,
    hist_l: [f32; 3],
    hist_r: [f32; 3],
}

impl TruePeakLimiter {
    pub fn new(ceiling_dbtp: f64) -> Self {
        let ceiling = 10f64.powf(ceiling_dbtp / 20.0).clamp(0.01, 1.0) as f32;
        Self {
            ceiling,
            knee_start: ceiling * 0.82,
            release_coeff: (-1.0f32 / (0.060 * SAMPLE_RATE as f32)).exp(),
            gain: 1.0,
            hist_l: [0.0; 3],
            hist_r: [0.0; 3],
        }
    }

    pub fn reset(&mut self) {
        self.gain = 1.0;
        self.hist_l = [0.0; 3];
        self.hist_r = [0.0; 3];
    }

    /// Apply `makeup_gain` and limit `buf` (interleaved stereo) so true peaks never exceed `ceiling`.
    pub fn process(&mut self, buf: &mut [f32], makeup_gain: f32) {
        let n_frames = buf.len() / 2;
        for i in 0..n_frames {
            let l = buf[i * 2] * makeup_gain;
            let r = buf[i * 2 + 1] * makeup_gain;
            let nl1 = if i + 1 < n_frames { buf[(i + 1) * 2] * makeup_gain } else { 0.0 };
            let nr1 = if i + 1 < n_frames { buf[(i + 1) * 2 + 1] * makeup_gain } else { 0.0 };
            let nl2 = if i + 2 < n_frames { buf[(i + 2) * 2] * makeup_gain } else { 0.0 };
            let nr2 = if i + 2 < n_frames { buf[(i + 2) * 2 + 1] * makeup_gain } else { 0.0 };

            let pk_l = l
                .abs()
                .max(intersample_peak(self.hist_l[0], self.hist_l[1], self.hist_l[2], l))
                .max(intersample_peak(self.hist_l[1], self.hist_l[2], l, nl1))
                .max(if i + 2 < n_frames { intersample_peak(self.hist_l[2], l, nl1, nl2) } else { 0.0 });
            let pk_r = r
                .abs()
                .max(intersample_peak(self.hist_r[0], self.hist_r[1], self.hist_r[2], r))
                .max(intersample_peak(self.hist_r[1], self.hist_r[2], r, nr1))
                .max(if i + 2 < n_frames { intersample_peak(self.hist_r[2], r, nr1, nr2) } else { 0.0 });
            let p = pk_l.max(pk_r);

            let target = if p <= self.knee_start || p <= 1e-12 {
                1.0
            } else {
                let span = (self.ceiling - self.knee_start).max(1e-6);
                let y = self.knee_start + span * ((p - self.knee_start) / span).tanh();
                (y / p).min(self.ceiling / p)
            };

            if target < self.gain {
                self.gain = target;
            } else {
                self.gain = self.gain * self.release_coeff + target * (1.0 - self.release_coeff);
            }

            buf[i * 2] = (l * self.gain).clamp(-self.ceiling, self.ceiling);
            buf[i * 2 + 1] = (r * self.gain).clamp(-self.ceiling, self.ceiling);

            self.hist_l = [self.hist_l[1], self.hist_l[2], l];
            self.hist_r = [self.hist_r[1], self.hist_r[2], r];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    fn sine_stereo(freq_hz: f64, peak_dbfs: f64, frames: usize) -> Vec<f32> {
        let amp = 10f64.powf(peak_dbfs / 20.0);
        let mut out = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let s = (amp * (TAU * freq_hz * i as f64 / SAMPLE_RATE as f64).sin()) as f32;
            out.push(s);
            out.push(s);
        }
        out
    }

    #[test]
    fn itu_1khz_stereo_sine_reference() {
        for target_db in [-14.0, -23.0] {
            let pcm = sine_stereo(1000.0, target_db, SAMPLE_RATE * 2);
            let mut m = LoudnessMeter::new();
            m.push(&pcm);
            let stats = m.finish();
            let lufs = stats.integrated_lufs.expect("sine must be above gate");
            assert!(
                (lufs - target_db).abs() <= 0.15,
                "expected ~{target_db} LUFS for 1 kHz stereo sine, got {lufs}"
            );
        }
    }

    #[test]
    fn gating_ignores_long_silences() {
        // ITU-R BS.1770-4 uses 400 ms / 75% overlap windows; for 1 s tone + silence, the 3
        // overlapping boundary windows keep the gated reading at -14.70 LUFS regardless of how
        // much silence follows, and a 5 s tone + 10 s silence stays within -14.0 +/- 0.2 LUFS.
        let mut short_3s = sine_stereo(1000.0, -14.0, SAMPLE_RATE);
        short_3s.extend(std::iter::repeat(0.0).take(SAMPLE_RATE * 3 * 2));
        let mut m1 = LoudnessMeter::new();
        m1.push(&short_3s);
        let lufs1 = m1.finish().integrated_lufs.expect("speech + silence must measure");

        let mut short_10s = sine_stereo(1000.0, -14.0, SAMPLE_RATE);
        short_10s.extend(std::iter::repeat(0.0).take(SAMPLE_RATE * 10 * 2));
        let mut m2 = LoudnessMeter::new();
        m2.push(&short_10s);
        let lufs2 = m2.finish().integrated_lufs.expect("speech + 10s silence must measure");

        assert!((lufs1 - lufs2).abs() < 0.01, "trailing silence leaked into gated LUFS: {lufs1} vs {lufs2}");
        assert!((lufs1 - (-14.70)).abs() <= 0.1, "expected ITU-R BS.1770-4 gated ~-14.70 LUFS, got {lufs1}");

        let mut long_clip = sine_stereo(1000.0, -14.0, SAMPLE_RATE * 5);
        long_clip.extend(std::iter::repeat(0.0).take(SAMPLE_RATE * 10 * 2));
        let mut m3 = LoudnessMeter::new();
        m3.push(&long_clip);
        let lufs3 = m3.finish().integrated_lufs.expect("5s tone + silence must measure");
        assert!(
            (lufs3 - (-14.0)).abs() <= 0.2,
            "5s tone + 10s silence should measure within -14.0 +/- 0.2 LUFS, got {lufs3}"
        );
    }

    #[test]
    fn silence_returns_none_and_unity_gain() {
        let pcm = vec![0.0f32; SAMPLE_RATE * 2 * 2];
        let mut m = LoudnessMeter::new();
        m.push(&pcm);
        let stats = m.finish();
        assert_eq!(stats.integrated_lufs, None);
        assert_eq!(makeup_gain(stats.integrated_lufs, YOUTUBE_LUFS), 1.0);
    }

    #[test]
    fn true_peak_limiter_enforces_minus_1_dbtp_ceiling() {
        // Test 1: Boosted 1 kHz sine
        let mut pcm_1k = sine_stereo(1000.0, -6.0, SAMPLE_RATE);
        let mut lim = TruePeakLimiter::new(TRUE_PEAK_CEILING_DBTP);
        lim.process(&mut pcm_1k, 4.0);
        assert!(pcm_1k.iter().all(|s| s.is_finite()));
        let mut m1 = LoudnessMeter::new();
        m1.push(&pcm_1k);
        let tp1 = m1.finish().true_peak_dbtp;
        assert!(tp1 <= -0.95, "1 kHz true peak exceeded ceiling: {tp1} dBTP");

        // Test 2: 12 kHz sine shifted by pi/4 produces inter-sample peaks ~3 dB higher than sample peaks
        let amp = 1.25f64;
        let mut pcm_12k = Vec::with_capacity(SAMPLE_RATE * 2);
        for i in 0..SAMPLE_RATE {
            let s = (amp * (TAU * 12_000.0 * i as f64 / SAMPLE_RATE as f64 + std::f64::consts::FRAC_PI_4).sin()) as f32;
            pcm_12k.push(s);
            pcm_12k.push(s);
        }
        lim.reset();
        lim.process(&mut pcm_12k, 1.0);
        assert!(pcm_12k.iter().all(|s| s.is_finite()));
        let mut m2 = LoudnessMeter::new();
        m2.push(&pcm_12k);
        let tp2 = m2.finish().true_peak_dbtp;
        assert!(tp2 <= -0.95, "12 kHz inter-sample true peak exceeded ceiling: {tp2} dBTP");
    }
}
