//! Spectral background noise reduction (fan noise, AC, hum, room hiss) for 48 kHz audio.
//!
//! Combines an 80 Hz 2nd-order Butterworth high-pass filter, automatic per-segment stationary
//! noise PSD estimation, and a 2048-point 75%-overlap WOLA Ephraim-Malah decision-directed
//! spectral Wiener suppressor with frequency and temporal smoothing.

use std::cmp::Ordering;
use std::f32::consts::LN_2;
use std::f64::consts::PI;

pub const SAMPLE_RATE: usize = 48_000;
pub const FFT_SIZE: usize = 2048;
pub const HOP_SIZE: usize = 512;
pub const NUM_BINS: usize = FFT_SIZE / 2 + 1;
pub const HPF_HZ: f64 = 80.0;

#[derive(Clone, Copy, Default)]
struct Complex {
    re: f32,
    im: f32,
}

impl Complex {
    #[inline]
    fn new(re: f32, im: f32) -> Self {
        Self { re, im }
    }

    #[inline]
    fn mag_sq(self) -> f32 {
        self.re * self.re + self.im * self.im
    }
}

/// In-place radix-2 Cooley-Tukey FFT for `FFT_SIZE` (2048).
#[derive(Clone)]
pub struct Fft2048 {
    bitrev: Vec<u16>,
    twiddles: Vec<Complex>,
}

impl Default for Fft2048 {
    fn default() -> Self {
        Self::new()
    }
}

impl Fft2048 {
    pub fn new() -> Self {
        let bits = FFT_SIZE.trailing_zeros();
        let mut bitrev = Vec::with_capacity(FFT_SIZE);
        for i in 0..FFT_SIZE {
            let r = (i as u32).reverse_bits() >> (32 - bits);
            bitrev.push(r as u16);
        }
        let mut twiddles = Vec::with_capacity(FFT_SIZE - 1);
        let mut len = 2;
        while len <= FFT_SIZE {
            let half = len / 2;
            let ang_step = -2.0 * PI / len as f64;
            for j in 0..half {
                let a = ang_step * j as f64;
                twiddles.push(Complex::new(a.cos() as f32, a.sin() as f32));
            }
            len <<= 1;
        }
        Self { bitrev, twiddles }
    }

    fn transform(&self, buf: &mut [Complex; FFT_SIZE], inverse: bool) {
        for i in 0..FFT_SIZE {
            let j = self.bitrev[i] as usize;
            if i < j {
                buf.swap(i, j);
            }
        }
        let mut tw_off = 0usize;
        let mut len = 2usize;
        while len <= FFT_SIZE {
            let half = len / 2;
            let tw_slice = &self.twiddles[tw_off..tw_off + half];
            for start in (0..FFT_SIZE).step_by(len) {
                for j in 0..half {
                    let w = tw_slice[j];
                    let w_im = if inverse { -w.im } else { w.im };
                    let u = buf[start + j];
                    let v = buf[start + j + half];
                    let tr = v.re * w.re - v.im * w_im;
                    let ti = v.re * w_im + v.im * w.re;
                    buf[start + j] = Complex::new(u.re + tr, u.im + ti);
                    buf[start + j + half] = Complex::new(u.re - tr, u.im - ti);
                }
            }
            tw_off += half;
            len <<= 1;
        }
        if inverse {
            let scale = 1.0 / FFT_SIZE as f32;
            for c in buf.iter_mut() {
                c.re *= scale;
                c.im *= scale;
            }
        }
    }

    fn forward_real(&self, input: &[f32; FFT_SIZE], out: &mut [Complex; FFT_SIZE]) {
        for i in 0..FFT_SIZE {
            out[i] = Complex::new(input[i], 0.0);
        }
        self.transform(out, false);
    }

    fn inverse_to_real(&self, spec: &mut [Complex; FFT_SIZE], out: &mut [f32; FFT_SIZE]) {
        self.transform(spec, true);
        for i in 0..FFT_SIZE {
            out[i] = spec[i].re;
        }
    }
}

/// 2nd-order Butterworth high-pass filter at 80 Hz (48 kHz) in `f64` Direct Form II Transposed.
#[derive(Clone, Copy)]
struct HighPass80 {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl HighPass80 {
    fn new() -> Self {
        let w0 = 2.0 * PI * HPF_HZ / SAMPLE_RATE as f64;
        let cos_w0 = w0.cos();
        let alpha = w0.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let b0 = ((1.0 + cos_w0) * 0.5) / a0;
        let b1 = -(1.0 + cos_w0) / a0;
        let b2 = b0;
        let a1 = (-2.0 * cos_w0) / a0;
        let a2 = (1.0 - alpha) / a0;
        Self { b0, b1, b2, a1, a2, z1: 0.0, z2: 0.0 }
    }

    fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let xd = x as f64;
        let y = self.b0 * xd + self.z1;
        self.z1 = self.b1 * xd - self.a1 * y + self.z2;
        self.z2 = self.b2 * xd - self.a2 * y;
        y as f32
    }
}

fn sine_window() -> Vec<f32> {
    (0..FFT_SIZE)
        .map(|i| (std::f32::consts::PI * (i as f32 + 0.5) / FFT_SIZE as f32).sin())
        .collect()
}

/// Estimated stationary noise power spectral density (`NUM_BINS` bins at 48 kHz / 2048 FFT).
#[derive(Clone, Debug)]
pub struct NoiseProfile {
    pub psd: Vec<f32>,
}

impl Default for NoiseProfile {
    fn default() -> Self {
        Self { psd: vec![0.0; NUM_BINS] }
    }
}

/// Estimates the stationary noise floor PSD from mono 48 kHz audio without requiring user selection.
pub struct NoiseProfileEstimator {
    fft: Fft2048,
    window: Vec<f32>,
    hpf: HighPass80,
    buf: Vec<f32>,
    frames: Vec<Vec<f32>>,
}

const ESTIMATOR_HOP: usize = 1024;
const ESTIMATOR_MAX_FRAMES: usize = 4096;

impl Default for NoiseProfileEstimator {
    fn default() -> Self {
        Self::new()
    }
}

impl NoiseProfileEstimator {
    pub fn new() -> Self {
        Self {
            fft: Fft2048::new(),
            window: sine_window(),
            hpf: HighPass80::new(),
            buf: Vec::with_capacity(FFT_SIZE * 2),
            frames: Vec::new(),
        }
    }

    pub fn push_mono(&mut self, samples: &[f32]) {
        if self.frames.len() >= ESTIMATOR_MAX_FRAMES {
            return;
        }
        for &s in samples {
            self.buf.push(self.hpf.process(s));
        }
        let mut windowed = [0.0f32; FFT_SIZE];
        let mut spec = [Complex::default(); FFT_SIZE];
        while self.buf.len() >= FFT_SIZE && self.frames.len() < ESTIMATOR_MAX_FRAMES {
            for i in 0..FFT_SIZE {
                windowed[i] = self.buf[i] * self.window[i];
            }
            self.fft.forward_real(&windowed, &mut spec);
            let mut pow = Vec::with_capacity(NUM_BINS);
            for k in 0..NUM_BINS {
                pow.push(spec[k].mag_sq());
            }
            self.frames.push(pow);
            self.buf.drain(..ESTIMATOR_HOP);
        }
    }

    pub fn finish(self) -> NoiseProfile {
        let active: Vec<&Vec<f32>> = self
            .frames
            .iter()
            .filter(|f| f.iter().sum::<f32>() >= 1e-11)
            .collect();
        let n = active.len();
        if n < 4 {
            return NoiseProfile::default();
        }

        // Rank active frames by mid-band speech energy (~160 Hz - 4 kHz).
        let mut ranked: Vec<(usize, f32)> = active
            .iter()
            .enumerate()
            .map(|(i, f)| (i, f[7..170].iter().sum::<f32>()))
            .collect();
        ranked.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal));

        let drop_lo = if n >= 50 { n / 50 } else { 0 };
        let take_n = (n * 15 / 100).max(4).min(n - drop_lo);
        let quiet_indices: Vec<usize> = ranked[drop_lo..drop_lo + take_n].iter().map(|&(i, _)| i).collect();

        let mut psd = vec![0.0f32; NUM_BINS];
        let mut col_quiet = Vec::with_capacity(quiet_indices.len());
        let mut col_all = Vec::with_capacity(n);
        let p15_scale = -(0.85_f32.ln());

        for k in 0..NUM_BINS {
            col_quiet.clear();
            for &fi in &quiet_indices {
                col_quiet.push(active[fi][k]);
            }
            col_quiet.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
            let n_frame = col_quiet[col_quiet.len() / 2] / LN_2;

            col_all.clear();
            for f in &active {
                col_all.push(f[k]);
            }
            col_all.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
            let idx15 = ((n as f32 * 0.15) as usize).min(n - 1);
            let n_bin = col_all[idx15] / p15_scale;

            psd[k] = n_frame.min(n_bin);
        }

        NoiseProfile { psd }
    }
}

/// Real-time/streaming 2048-point WOLA Ephraim-Malah decision-directed spectral denoiser.
#[derive(Clone)]
pub struct SpectralDenoiser {
    fft: Fft2048,
    window: Vec<f32>,
    strength: f32,
    prev_gain: Vec<f32>,
    prev_clean_pow: Vec<f32>,
    hpf_l: HighPass80,
    hpf_r: HighPass80,
    ola_l: [f32; FFT_SIZE],
    ola_r: [f32; FFT_SIZE],
}

impl SpectralDenoiser {
    pub fn new(strength: f32) -> Self {
        Self {
            fft: Fft2048::new(),
            window: sine_window(),
            strength: strength.clamp(0.0, 1.0),
            prev_gain: vec![1.0; NUM_BINS],
            prev_clean_pow: vec![0.0; NUM_BINS],
            hpf_l: HighPass80::new(),
            hpf_r: HighPass80::new(),
            ola_l: [0.0; FFT_SIZE],
            ola_r: [0.0; FFT_SIZE],
        }
    }

    pub fn set_strength(&mut self, strength: f32) {
        self.strength = strength.clamp(0.0, 1.0);
    }

    pub fn reset(&mut self) {
        self.hpf_l.reset();
        self.hpf_r.reset();
        self.ola_l.fill(0.0);
        self.ola_r.fill(0.0);
        self.prev_gain.fill(1.0);
        self.prev_clean_pow.fill(0.0);
    }

    #[inline]
    pub fn filter_input_mono(&mut self, x: f32) -> f32 {
        let hp = self.hpf_l.process(x);
        let mix = (self.strength * 2.5).min(1.0);
        x * (1.0 - mix) + hp * mix
    }

    #[inline]
    pub fn filter_input_stereo(&mut self, l: f32, r: f32) -> (f32, f32) {
        let hl = self.hpf_l.process(l);
        let hr = self.hpf_r.process(r);
        let mix = (self.strength * 2.5).min(1.0);
        (l * (1.0 - mix) + hl * mix, r * (1.0 - mix) + hr * mix)
    }

    fn compute_gains(&mut self, power: &[f32; NUM_BINS], psd: &[f32], gains: &mut [f32; NUM_BINS]) {
        if self.strength <= 1e-4 {
            gains.fill(1.0);
            self.prev_gain.fill(1.0);
            self.prev_clean_pow.copy_from_slice(power);
            return;
        }
        let beta = 1.0 + 2.0 * self.strength;
        let max_atten_db = 30.0 * self.strength;
        let g_min = 10f32.powf(-max_atten_db / 20.0);
        const ALPHA: f32 = 0.96;

        let mut g_raw = [1.0f32; NUM_BINS];
        for k in 0..NUM_BINS {
            let p_noise = psd.get(k).copied().unwrap_or(0.0);
            if p_noise <= 1e-12 {
                g_raw[k] = 1.0;
            } else {
                let noise = (p_noise * beta).max(1e-12);
                let gamma = (power[k] / noise).min(1000.0);
                let xi = ALPHA * (self.prev_clean_pow[k] / noise).min(1000.0)
                    + (1.0 - ALPHA) * (gamma - 1.0).max(0.0);
                g_raw[k] = (xi / (xi + 1.0)).clamp(g_min, 1.0);
            }
        }

        // 3-bin spectral smoothing suppresses isolated musical-noise sparkles.
        for k in 0..NUM_BINS {
            let lo = g_raw[k.saturating_sub(1)];
            let mid = g_raw[k];
            let hi = g_raw[(k + 1).min(NUM_BINS - 1)];
            let g_freq = 0.2 * lo + 0.6 * mid + 0.2 * hi;

            let prev = self.prev_gain[k];
            let coeff = if g_freq > prev { 0.75 } else { 0.25 };
            let g = (prev + coeff * (g_freq - prev)).clamp(g_min, 1.0);
            gains[k] = g;
            self.prev_gain[k] = g;
            self.prev_clean_pow[k] = g * g * power[k];
        }
    }

    fn apply_gains(spec: &mut [Complex; FFT_SIZE], gains: &[f32; NUM_BINS]) {
        spec[0].re *= gains[0];
        spec[0].im *= gains[0];
        let nyq = FFT_SIZE / 2;
        spec[nyq].re *= gains[nyq];
        spec[nyq].im *= gains[nyq];
        for k in 1..nyq {
            let g = gains[k];
            spec[k].re *= g;
            spec[k].im *= g;
            let ck = FFT_SIZE - k;
            spec[ck].re *= g;
            spec[ck].im *= g;
        }
    }

    /// Process one `FFT_SIZE` mono frame (already high-pass filtered), returning the next
    /// `HOP_SIZE` overlap-added samples into `out`.
    pub fn process_hop_mono(&mut self, frame: &[f32; FFT_SIZE], psd: &[f32], out: &mut [f32; HOP_SIZE]) {
        let mut windowed = [0.0f32; FFT_SIZE];
        for i in 0..FFT_SIZE {
            windowed[i] = frame[i] * self.window[i];
        }
        let mut spec = [Complex::default(); FFT_SIZE];
        self.fft.forward_real(&windowed, &mut spec);
        let mut power = [0.0f32; NUM_BINS];
        for k in 0..NUM_BINS {
            power[k] = spec[k].mag_sq();
        }
        let mut gains = [1.0f32; NUM_BINS];
        self.compute_gains(&power, psd, &mut gains);
        Self::apply_gains(&mut spec, &gains);

        let mut time = [0.0f32; FFT_SIZE];
        self.fft.inverse_to_real(&mut spec, &mut time);
        for i in 0..FFT_SIZE {
            self.ola_l[i] += time[i] * self.window[i] * 0.5;
        }
        out.copy_from_slice(&self.ola_l[..HOP_SIZE]);
        self.ola_l.copy_within(HOP_SIZE.., 0);
        self.ola_l[FFT_SIZE - HOP_SIZE..].fill(0.0);
    }

    /// Process one `FFT_SIZE` stereo pair (already high-pass filtered) using linked L+R gain.
    pub fn process_hop_stereo(
        &mut self,
        frame_l: &[f32; FFT_SIZE],
        frame_r: &[f32; FFT_SIZE],
        psd: &[f32],
        out_l: &mut [f32; HOP_SIZE],
        out_r: &mut [f32; HOP_SIZE],
    ) {
        let mut wl = [0.0f32; FFT_SIZE];
        let mut wr = [0.0f32; FFT_SIZE];
        for i in 0..FFT_SIZE {
            let w = self.window[i];
            wl[i] = frame_l[i] * w;
            wr[i] = frame_r[i] * w;
        }
        let mut spec_l = [Complex::default(); FFT_SIZE];
        let mut spec_r = [Complex::default(); FFT_SIZE];
        self.fft.forward_real(&wl, &mut spec_l);
        self.fft.forward_real(&wr, &mut spec_r);

        let mut power = [0.0f32; NUM_BINS];
        for k in 0..NUM_BINS {
            let mid_re = (spec_l[k].re + spec_r[k].re) * 0.5;
            let mid_im = (spec_l[k].im + spec_r[k].im) * 0.5;
            power[k] = (mid_re * mid_re + mid_im * mid_im).max((spec_l[k].mag_sq() + spec_r[k].mag_sq()) * 0.5);
        }
        let mut gains = [1.0f32; NUM_BINS];
        self.compute_gains(&power, psd, &mut gains);
        Self::apply_gains(&mut spec_l, &gains);
        Self::apply_gains(&mut spec_r, &gains);

        let mut tl = [0.0f32; FFT_SIZE];
        let mut tr = [0.0f32; FFT_SIZE];
        self.fft.inverse_to_real(&mut spec_l, &mut tl);
        self.fft.inverse_to_real(&mut spec_r, &mut tr);
        for i in 0..FFT_SIZE {
            let ws = self.window[i] * 0.5;
            self.ola_l[i] += tl[i] * ws;
            self.ola_r[i] += tr[i] * ws;
        }
        out_l.copy_from_slice(&self.ola_l[..HOP_SIZE]);
        out_r.copy_from_slice(&self.ola_r[..HOP_SIZE]);
        self.ola_l.copy_within(HOP_SIZE.., 0);
        self.ola_r.copy_within(HOP_SIZE.., 0);
        self.ola_l[FFT_SIZE - HOP_SIZE..].fill(0.0);
        self.ola_r[FFT_SIZE - HOP_SIZE..].fill(0.0);
    }
}

/// Denoise a whole mono buffer from start to finish (zero-latency aligned).
pub fn denoise_mono_buffer(samples: &[f32], profile: &NoiseProfile, strength: f32) -> Vec<f32> {
    if samples.is_empty() || strength <= 1e-4 {
        return samples.to_vec();
    }
    let mut dn = SpectralDenoiser::new(strength);
    let filtered: Vec<f32> = samples.iter().map(|&s| dn.filter_input_mono(s)).collect();
    let total_hops = samples.len().div_ceil(HOP_SIZE);
    let mut out = Vec::with_capacity(samples.len());
    let mut frame = [0.0f32; FFT_SIZE];
    let mut hop_out = [0.0f32; HOP_SIZE];

    for h in -3isize..(total_hops as isize) {
        let base = h * HOP_SIZE as isize;
        for i in 0..FFT_SIZE {
            let idx = base + i as isize;
            frame[i] = if idx >= 0 && (idx as usize) < filtered.len() {
                filtered[idx as usize]
            } else {
                0.0
            };
        }
        dn.process_hop_mono(&frame, &profile.psd, &mut hop_out);
        if h >= 0 {
            let rem = samples.len() - out.len();
            out.extend_from_slice(&hop_out[..rem.min(HOP_SIZE)]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    #[test]
    fn fft_2048_roundtrip() {
        let fft = Fft2048::new();
        let mut input = [0.0f32; FFT_SIZE];
        for (i, v) in input.iter_mut().enumerate() {
            *v = (TAU * 440.0 * i as f64 / SAMPLE_RATE as f64).sin() as f32 * 0.7
                + (TAU * 1760.0 * i as f64 / SAMPLE_RATE as f64).cos() as f32 * 0.25;
        }
        let mut spec = [Complex::default(); FFT_SIZE];
        fft.forward_real(&input, &mut spec);
        let mut back = [0.0f32; FFT_SIZE];
        fft.inverse_to_real(&mut spec, &mut back);
        let max_err = input.iter().zip(&back).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(max_err < 1e-4, "FFT roundtrip error too high: {max_err}");
    }

    #[test]
    fn wola_unity_gain_reconstructs_signal() {
        let len = SAMPLE_RATE;
        let input: Vec<f32> = (0..len)
            .map(|i| {
                0.5 * (TAU * 330.0 * i as f64 / SAMPLE_RATE as f64).sin() as f32
                    + 0.3 * (TAU * 1200.0 * i as f64 / SAMPLE_RATE as f64).cos() as f32
            })
            .collect();
        let profile = NoiseProfile::default();
        // Zero PSD -> unity gains across all bins (with HPF at 0.85 strength, 330Hz/1200Hz pass near unity;
        // also test exact bypass with strength = 0.0 -> zero reconstruction error).
        let bypass = denoise_mono_buffer(&input, &profile, 0.0);
        let max_bypass_err = input.iter().zip(&bypass).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(max_bypass_err < 1e-6);

        let mut dn = SpectralDenoiser::new(0.8);
        let mut frame = [0.0f32; FFT_SIZE];
        let mut hop_out = [0.0f32; HOP_SIZE];
        let mut out = Vec::with_capacity(len);
        for h in -3isize..(len.div_ceil(HOP_SIZE) as isize) {
            let base = h * HOP_SIZE as isize;
            for i in 0..FFT_SIZE {
                let idx = base + i as isize;
                frame[i] = if idx >= 0 && (idx as usize) < len { input[idx as usize] } else { 0.0 };
            }
            dn.process_hop_mono(&frame, &profile.psd, &mut hop_out);
            if h >= 0 {
                let rem = len - out.len();
                out.extend_from_slice(&hop_out[..rem.min(HOP_SIZE)]);
            }
        }
        // After the 3-hop startup ramp ends at sample 1536 and before trailing fade at len-1536,
        // check exact sample alignment and unity reconstruction.
        let max_err = input[2048..len - 2048]
            .iter()
            .zip(&out[2048..len - 2048])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(max_err < 1e-4, "WOLA reconstruction error too high: {max_err}");
    }

    #[test]
    fn reduces_stationary_fan_noise_while_preserving_speech_bursts() {
        let len = SAMPLE_RATE * 4;
        // Deterministic PRNG white/pink-ish fan noise + 60 Hz & 120 Hz hum + 3 speech bursts.
        let mut rng = 0x1234_5678_9abc_def0u64;
        let mut lp = 0.0f32;
        let mut noise = vec![0.0f32; len];
        for i in 0..len {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let white = ((rng >> 33) as f32 / (u32::MAX >> 1) as f32) - 1.0;
            lp = lp * 0.85 + white * 0.15;
            let hum = 0.025 * (TAU * 60.0 * i as f64 / SAMPLE_RATE as f64).sin() as f32
                + 0.015 * (TAU * 120.0 * i as f64 / SAMPLE_RATE as f64).sin() as f32;
            noise[i] = lp * 0.10 + hum;
        }

        let speech_windows = [
            (SAMPLE_RATE / 2, SAMPLE_RATE),
            (SAMPLE_RATE * 3 / 2, SAMPLE_RATE * 2),
            (SAMPLE_RATE * 5 / 2, SAMPLE_RATE * 3),
        ];
        let mut clean_speech = vec![0.0f32; len];
        for &(a, b) in &speech_windows {
            for i in a..b {
                let t = i as f64 / SAMPLE_RATE as f64;
                clean_speech[i] = 0.25 * (TAU * 180.0 * t).sin() as f32
                    + 0.18 * (TAU * 360.0 * t).sin() as f32
                    + 0.12 * (TAU * 720.0 * t).sin() as f32
                    + 0.06 * (TAU * 1440.0 * t).sin() as f32;
            }
        }

        let noisy: Vec<f32> = noise.iter().zip(&clean_speech).map(|(n, s)| n + s).collect();
        let mut est = NoiseProfileEstimator::new();
        est.push_mono(&noisy);
        let profile = est.finish();

        let denoised = denoise_mono_buffer(&noisy, &profile, 0.85);
        assert!(denoised.iter().all(|s| s.is_finite()));

        let rms = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();

        // Check noise reduction in a quiet pause (e.g., 3.2s..3.8s).
        let pause_in = rms(&noisy[SAMPLE_RATE * 32 / 10..SAMPLE_RATE * 38 / 10]);
        let pause_out = rms(&denoised[SAMPLE_RATE * 32 / 10..SAMPLE_RATE * 38 / 10]);
        let drop_db = 20.0 * (pause_in / pause_out.max(1e-9)).log10();
        assert!(drop_db >= 14.0, "expected >= 14 dB fan noise drop, got {drop_db:.2} dB");

        // Check speech preservation across the 3 bursts.
        for (idx, &(a, b)) in speech_windows.iter().enumerate() {
            let speech_ref = rms(&clean_speech[a + 2400..b - 2400]);
            let speech_out = rms(&denoised[a + 2400..b - 2400]);
            let diff_db = 20.0 * (speech_out / speech_ref).log10();
            assert!(
                diff_db.abs() <= 2.5,
                "speech burst {idx} distorted by {diff_db:.2} dB"
            );
        }
    }
}
