//! Builds the edited soundtrack: source audio mapped through the slices (cuts, speed changes),
//! scaled by the per-slice and project volumes, optionally denoised and loudness-normalized.

use screencraft_audio::denoise::{SpectralDenoiser, FFT_SIZE, HOP_SIZE};
use screencraft_audio::loudness::TruePeakLimiter;
use screencraft_core::project::{Project, Slice};
use screencraft_core::timeline::slice_output_starts;
use std::sync::Arc;

pub const RATE: f64 = 48_000.0;

/// What an audio source must provide: 48 kHz stereo at an arbitrary source time.
pub trait PcmSource {
    fn read(&mut self, start_ms: f64, frames: usize) -> Vec<f32>;
}

impl PcmSource for crate::source::AudioSource {
    fn read(&mut self, start_ms: f64, frames: usize) -> Vec<f32> {
        crate::source::AudioSource::read(self, start_ms, frames).unwrap_or_else(|_| vec![0.0; frames * 2])
    }
}

/// Time-mapped stationary noise floor profiles across project segments.
#[derive(Clone, Debug, Default)]
pub struct SegmentedNoiseProfile {
    pub segments: Vec<(f64, f64, Arc<Vec<f32>>)>,
}

impl SegmentedNoiseProfile {
    pub fn from_single(psd: Arc<Vec<f32>>) -> Self {
        Self { segments: vec![(0.0, f64::INFINITY, psd)] }
    }

    pub fn psd_at(&self, t_ms: f64) -> &[f32] {
        for (start, end, psd) in &self.segments {
            if t_ms >= *start && t_ms < *end {
                return psd;
            }
        }
        if let Some((start, _, psd)) = self.segments.first() {
            if t_ms < *start {
                return psd;
            }
        }
        self.segments.last().map(|(_, _, p)| p.as_slice()).unwrap_or(&[])
    }
}

/// Streaming spectral noise-reduction wrapper around any `PcmSource` (48 kHz stereo).
///
/// Maintains a lookbehind output buffer so small overlapping reads from linear resampling never
/// trigger decoder resets or discontinuities.
pub struct DenoisedSource<S: PcmSource> {
    source: S,
    profile: SegmentedNoiseProfile,
    denoiser: SpectralDenoiser,
    stereo: bool,
    initialized: bool,
    in_start: usize,
    in_raw_next: usize,
    in_l: Vec<f32>,
    in_r: Vec<f32>,
    next_hop: isize,
    out_start: usize,
    out_pcm: Vec<f32>,
}

impl<S: PcmSource> DenoisedSource<S> {
    pub fn new_mono(source: S, profile: SegmentedNoiseProfile, strength: f32) -> Self {
        Self::new_inner(source, profile, strength, false)
    }

    pub fn new_stereo(source: S, profile: SegmentedNoiseProfile, strength: f32) -> Self {
        Self::new_inner(source, profile, strength, true)
    }

    fn new_inner(source: S, profile: SegmentedNoiseProfile, strength: f32, stereo: bool) -> Self {
        Self {
            source,
            profile,
            denoiser: SpectralDenoiser::new(strength),
            stereo,
            initialized: false,
            in_start: 0,
            in_raw_next: 0,
            in_l: Vec::new(),
            in_r: Vec::new(),
            next_hop: -4,
            out_start: 0,
            out_pcm: Vec::new(),
        }
    }

    fn pull_input_until(&mut self, needed_end: usize) {
        const CHUNK_FRAMES: usize = 4_800;
        while self.in_raw_next < needed_end {
            let start_ms = self.in_raw_next as f64 * 1000.0 / RATE;
            let raw = self.source.read(start_ms, CHUNK_FRAMES);
            let n = raw.len() / 2;
            if n == 0 {
                break;
            }
            if self.stereo {
                for f in raw.chunks_exact(2) {
                    let (hl, hr) = self.denoiser.filter_input_stereo(f[0], f[1]);
                    self.in_l.push(hl);
                    self.in_r.push(hr);
                }
            } else {
                for f in raw.chunks_exact(2) {
                    let m = (f[0] + f[1]) * 0.5;
                    self.in_l.push(self.denoiser.filter_input_mono(m));
                }
            }
            self.in_raw_next += n;
        }
    }

    fn process_until(&mut self, req_end: usize) {
        let mut frame_l = [0.0f32; FFT_SIZE];
        let mut frame_r = [0.0f32; FFT_SIZE];
        let mut hop_l = [0.0f32; HOP_SIZE];
        let mut hop_r = [0.0f32; HOP_SIZE];

        while self.out_start + self.out_pcm.len() / 2 < req_end {
            let h = self.next_hop;
            let base = h * HOP_SIZE as isize;
            let win_end = (base + FFT_SIZE as isize).max(0) as usize;
            self.pull_input_until(win_end);

            let t_ms = (base.max(0) as f64) * 1000.0 / RATE;
            let psd = self.profile.psd_at(t_ms);

            if self.stereo {
                for i in 0..FFT_SIZE {
                    let idx = base + i as isize;
                    if idx >= 0 && (idx as usize) >= self.in_start {
                        let off = (idx as usize) - self.in_start;
                        frame_l[i] = self.in_l.get(off).copied().unwrap_or(0.0);
                        frame_r[i] = self.in_r.get(off).copied().unwrap_or(0.0);
                    } else {
                        frame_l[i] = 0.0;
                        frame_r[i] = 0.0;
                    }
                }
                self.denoiser.process_hop_stereo(&frame_l, &frame_r, psd, &mut hop_l, &mut hop_r);
            } else {
                for i in 0..FFT_SIZE {
                    let idx = base + i as isize;
                    frame_l[i] = if idx >= 0 && (idx as usize) >= self.in_start {
                        self.in_l.get((idx as usize) - self.in_start).copied().unwrap_or(0.0)
                    } else {
                        0.0
                    };
                }
                self.denoiser.process_hop_mono(&frame_l, psd, &mut hop_l);
            }

            if h >= 0 && (h as usize) * HOP_SIZE == self.out_start + self.out_pcm.len() / 2 {
                if self.stereo {
                    for k in 0..HOP_SIZE {
                        self.out_pcm.push(hop_l[k]);
                        self.out_pcm.push(hop_r[k]);
                    }
                } else {
                    for k in 0..HOP_SIZE {
                        let v = hop_l[k];
                        self.out_pcm.push(v);
                        self.out_pcm.push(v);
                    }
                }
            }
            self.next_hop += 1;
        }
    }

    fn prune(&mut self, req_start: usize) {
        const LOOKBEHIND_FRAMES: usize = 2_048;
        let keep_out = req_start.saturating_sub(LOOKBEHIND_FRAMES);
        if keep_out > self.out_start {
            let drop_frames = (keep_out - self.out_start).min(self.out_pcm.len() / 2);
            self.out_pcm.drain(..drop_frames * 2);
            self.out_start += drop_frames;
        }

        let hop_sample = (self.next_hop * HOP_SIZE as isize).max(0) as usize;
        let keep_in = hop_sample.saturating_sub(FFT_SIZE);
        if keep_in > self.in_start {
            let drop_in = (keep_in - self.in_start).min(self.in_l.len());
            self.in_l.drain(..drop_in);
            if self.stereo {
                let drop_r = drop_in.min(self.in_r.len());
                self.in_r.drain(..drop_r);
            }
            self.in_start += drop_in;
        }
    }
}

impl<S: PcmSource> PcmSource for DenoisedSource<S> {
    fn read(&mut self, start_ms: f64, frames: usize) -> Vec<f32> {
        if frames == 0 {
            return Vec::new();
        }
        let mut out = vec![0.0f32; frames * 2];
        let start_frame_signed = (start_ms * RATE / 1000.0).round() as isize;
        let lead_zero = if start_frame_signed < 0 {
            ((-start_frame_signed) as usize).min(frames)
        } else {
            0
        };
        let req_frames = frames - lead_zero;
        if req_frames == 0 {
            return out;
        }
        let req_start = start_frame_signed.max(0) as usize;
        let req_end = req_start + req_frames;

        let out_end = self.out_start + self.out_pcm.len() / 2;
        if !self.initialized || req_start < self.out_start || req_start > out_end + 48_000 {
            self.denoiser.reset();
            self.out_pcm.clear();
            self.in_l.clear();
            self.in_r.clear();
            let hop_target = (req_start / HOP_SIZE) as isize;
            let warmup_start = (hop_target - 4).max(0) as usize * HOP_SIZE;
            self.in_start = warmup_start;
            self.in_raw_next = warmup_start;
            self.next_hop = hop_target - 4;
            self.out_start = (hop_target as usize) * HOP_SIZE;
            self.initialized = true;
        }

        self.process_until(req_end);
        let src_off = (req_start - self.out_start) * 2;
        let avail = self.out_pcm.len().saturating_sub(src_off).min(req_frames * 2);
        out[lead_zero * 2..lead_zero * 2 + avail].copy_from_slice(&self.out_pcm[src_off..src_off + avail]);
        self.prune(req_start);
        out
    }
}

pub struct Lane {
    pub source: Box<dyn PcmSource + Send>,
    pub gain: f32,
}

/// A music clip placed on the edited timeline.
pub struct MusicPlay {
    pub pcm: std::sync::Arc<Vec<f32>>,
    pub offset_ms: f64,
    pub in_ms: f64,
    pub dur_ms: f64,
    pub fade_in_ms: f64,
    pub fade_out_ms: f64,
}

/// Everything besides the recorded tracks: music (with fades and ducking under speech) and the
/// click / typing sound effects.
#[derive(Default)]
pub struct Extras {
    pub music: Vec<MusicPlay>,
    /// Master x music volume.
    pub music_gain: f32,
    /// Output-time ranges (ms) where someone speaks; the music ducks under them.
    pub duck_ranges: Vec<[f64; 2]>,
    pub duck_level: f32,
    pub hits: Vec<crate::sfx::Hit>,
}

const ATTACK_MS: f64 = 150.0;
const RELEASE_MS: f64 = 450.0;

/// Music gain under speech at output time `t` (1 = untouched).
fn duck_gain(ranges: &[[f64; 2]], level: f32, t: f64) -> f32 {
    let lvl = level.clamp(0.02, 1.0);
    let mut g = 1.0f32;
    // ranges are sorted by start; only those near `t` matter
    let hi = ranges.partition_point(|r| r[0] - ATTACK_MS <= t);
    for r in ranges[..hi].iter().rev().take(8) {
        let v = if t < r[0] {
            1.0 - (1.0 - lvl) * ((t - (r[0] - ATTACK_MS)) / ATTACK_MS) as f32
        } else if t <= r[1] {
            lvl
        } else if t < r[1] + RELEASE_MS {
            lvl + (1.0 - lvl) * ((t - r[1]) / RELEASE_MS) as f32
        } else {
            1.0
        };
        g = g.min(v);
    }
    g
}

struct LoudnessMaster {
    gain: f32,
    limiter: TruePeakLimiter,
    last_end_frame: Option<u64>,
}

pub struct Soundtrack {
    slices: Vec<Slice>,
    starts: Vec<f64>,
    lanes: Vec<Lane>,
    extras: Extras,
    master: Option<LoudnessMaster>,
}

impl Soundtrack {
    pub fn new(project: &Project, lanes: Vec<Lane>) -> Self {
        let slices = project.scene.slices.clone();
        let starts = slice_output_starts(&slices);
        Self { slices, starts, lanes, extras: Extras::default(), master: None }
    }

    pub fn with_extras(mut self, e: Extras) -> Self {
        self.extras = e;
        self
    }

    pub fn with_loudness(mut self, gain: f32, ceiling_dbtp: f64) -> Self {
        self.master = Some(LoudnessMaster {
            gain,
            limiter: TruePeakLimiter::new(ceiling_dbtp),
            last_end_frame: None,
        });
        self
    }

    /// Interleaved stereo before final mastering/limiting, for loudness measurement.
    pub fn render_unmastered(&mut self, start_frame: u64, len: usize) -> Vec<f32> {
        let mut out = vec![0f32; len * 2];
        let mut done = 0usize;
        while done < len {
            let t_ms = (start_frame + done as u64) as f64 * 1000.0 / RATE;
            // Slice containing t_ms and how many output frames remain in it.
            let idx = self
                .starts
                .iter()
                .rposition(|&s| s <= t_ms + 1e-6)
                .filter(|&i| {
                    let s = &self.slices[i];
                    t_ms < self.starts[i] + (s.source_end_ms - s.source_start_ms) / s.time_scale.max(1e-3) - 1e-6
                });
            let Some(i) = idx else {
                break; // past the end of the edit
            };
            let s = &self.slices[i];
            let scale = s.time_scale.max(1e-3);
            let slice_out_end_ms = self.starts[i] + (s.source_end_ms - s.source_start_ms) / scale;
            let remain = (((slice_out_end_ms - t_ms) * RATE / 1000.0).ceil() as usize).clamp(1, len - done);
            let silent = s.freeze || scale < 0.05;
            if !silent {
                let src_start = s.source_start_ms + (t_ms - self.starts[i]) * scale;
                let src_frames = ((remain as f64 * scale).ceil() as usize + 2).max(2);
                for lane in &mut self.lanes {
                    let g = lane.gain * s.volume as f32;
                    if g <= 0.0 {
                        continue;
                    }
                    let pcm = lane.source.read(src_start, src_frames);
                    let last = pcm.len() / 2 - 1;
                    for k in 0..remain {
                        // Linear resample to the output grid (scale == 1 hits exact samples).
                        let pos = k as f64 * scale;
                        let a = (pos.floor() as usize).min(last);
                        let b = (a + 1).min(last);
                        let f = (pos - a as f64) as f32;
                        let o = (done + k) * 2;
                        out[o] += (pcm[a * 2] * (1.0 - f) + pcm[b * 2] * f) * g;
                        out[o + 1] += (pcm[a * 2 + 1] * (1.0 - f) + pcm[b * 2 + 1] * f) * g;
                    }
                }
            }
            done += remain;
        }
        self.mix_extras(&mut out, start_frame);
        out
    }

    /// Interleaved stereo for output frames `[start, start + len)` at 48 kHz.
    pub fn render(&mut self, start_frame: u64, len: usize) -> Vec<f32> {
        let mut out = self.render_unmastered(start_frame, len);
        if let Some(m) = self.master.as_mut() {
            if m.last_end_frame != Some(start_frame) {
                m.limiter.reset();
            }
            m.limiter.process(&mut out, m.gain);
            m.last_end_frame = Some(start_frame + len as u64);
        } else {
            screencraft_audio::mix::limit(&mut out);
        }
        out
    }

    fn mix_extras(&self, out: &mut [f32], start_frame: u64) {
        let len = out.len() / 2;
        let (t0, t1) = (start_frame as f64 * 1000.0 / RATE, (start_frame + len as u64) as f64 * 1000.0 / RATE);
        let e = &self.extras;
        for m in &e.music {
            let end = m.offset_ms + m.dur_ms;
            if end <= t0 || m.offset_ms >= t1 {
                continue;
            }
            for k in 0..len {
                let t = (start_frame + k as u64) as f64 * 1000.0 / RATE;
                if t < m.offset_ms || t >= end {
                    continue;
                }
                let idx = (((m.in_ms + (t - m.offset_ms)) * RATE / 1000.0) as usize) * 2;
                if idx + 1 >= m.pcm.len() {
                    continue;
                }
                let fi = if m.fade_in_ms > 0.0 { ((t - m.offset_ms) / m.fade_in_ms).min(1.0) } else { 1.0 };
                let fo = if m.fade_out_ms > 0.0 { ((end - t) / m.fade_out_ms).min(1.0) } else { 1.0 };
                let g = e.music_gain * fi.min(fo) as f32 * duck_gain(&e.duck_ranges, e.duck_level, t);
                out[k * 2] += m.pcm[idx] * g;
                out[k * 2 + 1] += m.pcm[idx + 1] * g;
            }
        }
        if !e.hits.is_empty() {
            let (a, b) = (start_frame as i64, (start_frame + len as u64) as i64);
            let first = e.hits.partition_point(|h| h.start_frame < a - 96_000);
            for h in &e.hits[first..] {
                if h.start_frame >= b {
                    break;
                }
                let frames = (h.pcm.len() / 2) as f64 / h.rate;
                if (h.start_frame as f64 + frames) < a as f64 {
                    continue;
                }
                let from = (a - h.start_frame).max(0);
                let last = h.pcm.len() / 2 - 1;
                for k in from..(b - h.start_frame) {
                    let pos = k as f64 * h.rate;
                    if pos >= last as f64 {
                        break;
                    }
                    let (i0, f) = (pos as usize, (pos - pos.floor()) as f32);
                    let i1 = (i0 + 1).min(last);
                    let o = ((h.start_frame + k - a) * 2) as usize;
                    out[o] += (h.pcm[i0 * 2] * (1.0 - f) + h.pcm[i1 * 2] * f) * h.gain;
                    out[o + 1] += (h.pcm[i0 * 2 + 1] * (1.0 - f) + h.pcm[i1 * 2 + 1] * f) * h.gain;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screencraft_core::project::{Media, Project};

    struct Ramp;
    impl PcmSource for Ramp {
        /// Value = source time in seconds, so mapping errors are visible.
        fn read(&mut self, start_ms: f64, frames: usize) -> Vec<f32> {
            (0..frames)
                .flat_map(|i| {
                    let v = (start_ms / 1000.0 + i as f64 / RATE) as f32 * 0.1;
                    [v, v]
                })
                .collect()
        }
    }

    fn project(slices: Vec<Slice>) -> Project {
        let mut p = Project::from_recording("t", Media { duration_ms: 10_000.0, ..Default::default() });
        p.scene.slices = slices;
        p
    }

    #[test]
    fn cuts_skip_source_time() {
        // keep 0-1 s and 3-4 s of the source
        let p = project(vec![
            Slice { id: "a".into(), source_start_ms: 0.0, source_end_ms: 1000.0, ..Default::default() },
            Slice { id: "b".into(), source_start_ms: 3000.0, source_end_ms: 4000.0, ..Default::default() },
        ]);
        let mut s = Soundtrack::new(&p, vec![Lane { source: Box::new(Ramp), gain: 1.0 }]);
        let pcm = s.render(48_000 + 100, 4);
        // output 1.002 s is source 3.002 s
        assert!((pcm[0] - 0.3002).abs() < 0.01, "{}", pcm[0]);
    }

    #[test]
    fn speed_change_scales_time() {
        let p = project(vec![Slice { id: "a".into(), source_start_ms: 0.0, source_end_ms: 4000.0, time_scale: 2.0, ..Default::default() }]);
        let mut s = Soundtrack::new(&p, vec![Lane { source: Box::new(Ramp), gain: 1.0 }]);
        // output 1 s -> source 2 s
        let pcm = s.render(48_000, 2);
        assert!((pcm[0] - 0.2).abs() < 0.01, "{}", pcm[0]);
        // the edit is 2 s long: beyond it, silence
        let tail = s.render(48_000 * 2 + 10, 4);
        assert!(tail.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn volume_and_mute_apply() {
        let p = project(vec![Slice { id: "a".into(), source_start_ms: 1000.0, source_end_ms: 2000.0, volume: 0.5, ..Default::default() }]);
        let mut s = Soundtrack::new(
            &p,
            vec![Lane { source: Box::new(Ramp), gain: 1.0 }, Lane { source: Box::new(Ramp), gain: 0.0 }],
        );
        let pcm = s.render(0, 2);
        assert!((pcm[0] - 0.05).abs() < 0.005, "{}", pcm[0]);
    }

    struct BufSrc(Vec<f32>);
    impl PcmSource for BufSrc {
        fn read(&mut self, start_ms: f64, frames: usize) -> Vec<f32> {
            let start = (start_ms.max(0.0) * RATE / 1000.0).round() as usize;
            let mut out = vec![0.0f32; frames * 2];
            for k in 0..frames {
                let i = start + k;
                if i * 2 + 1 < self.0.len() {
                    out[k * 2] = self.0[i * 2];
                    out[k * 2 + 1] = self.0[i * 2 + 1];
                }
            }
            out
        }
    }

    fn make_noisy_voice_pcm(secs: usize) -> (Vec<f32>, Arc<Vec<f32>>) {
        use std::f64::consts::TAU;
        let frames = secs * 48_000;
        let mut rng = 0x9e37_79b9_7f4a_7c15u64;
        let mut mono = Vec::with_capacity(frames);
        let mut stereo = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let n = (((rng >> 33) as f32 / (u32::MAX >> 1) as f32) - 1.0) * 0.04;
            let tone = if (i / 24_000) % 2 == 0 {
                0.22 * (TAU * 320.0 * i as f64 / RATE).sin() as f32
            } else {
                0.0
            };
            let s = n + tone;
            mono.push(s);
            stereo.push(s);
            stereo.push(s);
        }
        let mut est = screencraft_audio::denoise::NoiseProfileEstimator::new();
        est.push_mono(&mono);
        (stereo, Arc::new(est.finish().psd))
    }

    #[test]
    fn denoised_source_matches_contiguous_across_overlapping_chunks() {
        let (pcm, psd) = make_noisy_voice_pcm(2);
        let prof = SegmentedNoiseProfile::from_single(psd);

        let mut whole = DenoisedSource::new_mono(BufSrc(pcm.clone()), prof.clone(), 0.75);
        let expected = whole.read(0.0, 24_000);

        let mut chunked = DenoisedSource::new_mono(BufSrc(pcm), prof, 0.75);
        let mut got = Vec::with_capacity(24_000 * 2);
        let mut pos = 0usize;
        while pos < 24_000 {
            let need = 800usize.min(24_000 - pos);
            // +2 extra frames mirrors Soundtrack::render linear-resampling overlap.
            let piece = chunked.read(pos as f64 * 1000.0 / RATE, need + 2);
            got.extend_from_slice(&piece[..need * 2]);
            pos += need;
        }

        let max_err = expected
            .iter()
            .zip(&got)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(max_err < 1e-5, "chunked DenoisedSource diverged by {max_err}");
    }

    #[test]
    fn soundtrack_loudness_target_hits_youtube_minus_14_lufs() {
        use screencraft_audio::loudness::{makeup_gain, LoudnessMeter, TRUE_PEAK_CEILING_DBTP, YOUTUBE_LUFS};
        use std::f64::consts::TAU;

        for input_db in [-26.0f64, -8.0f64] {
            let amp = 10f64.powf(input_db / 20.0) as f32;
            let frames = 48_000 * 2;
            let mut src_pcm = Vec::with_capacity(frames * 2);
            for i in 0..frames {
                let s = amp * (TAU * 1000.0 * i as f64 / RATE).sin() as f32;
                src_pcm.push(s);
                src_pcm.push(s);
            }

            let p = project(vec![Slice { id: "a".into(), source_start_ms: 0.0, source_end_ms: 2000.0, ..Default::default() }]);
            let mut raw = Soundtrack::new(&p, vec![Lane { source: Box::new(BufSrc(src_pcm.clone())), gain: 1.0 }]);
            let mut m_raw = LoudnessMeter::new();
            for chunk in 0..2 {
                let pcm = raw.render_unmastered(chunk * 48_000, 48_000);
                m_raw.push(&pcm);
            }
            let raw_stats = m_raw.finish();
            let gain = makeup_gain(raw_stats.integrated_lufs, YOUTUBE_LUFS);

            let mut mastered = Soundtrack::new(&p, vec![Lane { source: Box::new(BufSrc(src_pcm)), gain: 1.0 }])
                .with_loudness(gain, TRUE_PEAK_CEILING_DBTP);
            let mut m_out = LoudnessMeter::new();
            for chunk in 0..4 {
                let pcm = mastered.render(chunk * 24_000, 24_000);
                m_out.push(&pcm);
            }
            let out_stats = m_out.finish();
            let lufs = out_stats.integrated_lufs.expect("output must be audible");
            assert!(
                (lufs - YOUTUBE_LUFS).abs() <= 0.3,
                "from {input_db} dBFS expected ~{YOUTUBE_LUFS} LUFS, got {lufs}"
            );
            assert!(
                out_stats.true_peak_dbtp <= -0.95,
                "true peak exceeded ceiling: {} dBTP",
                out_stats.true_peak_dbtp
            );
        }
    }
}
