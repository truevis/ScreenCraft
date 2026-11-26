//! Word-level transcript extraction and transcript-driven timeline jump-cut editing.

use crate::jumpcut::{apply_jump_cuts, heal};
use crate::project::{Slice, Subtitle};

#[derive(Clone, Debug, PartialEq)]
pub struct TranscriptWord {
    pub caption_id: String,
    pub word_index: usize,
    pub text: String,
    pub start_ms: f64,
    pub end_ms: f64,
    pub cut: bool,
}

/// Expand a subtitle into `(word, start_ms, end_ms)` tuples using exact word timestamps when
/// available, otherwise spreading words proportionally across the subtitle's duration.
pub fn word_spans(sub: &Subtitle) -> Vec<(String, f64, f64)> {
    let words: Vec<&str> = sub.text.split_whitespace().collect();
    let n = words.len();
    if n == 0 {
        return Vec::new();
    }
    if sub.word_starts.len() == n && sub.word_ends.len() == n {
        return words
            .into_iter()
            .enumerate()
            .map(|(i, w)| {
                let s = sub.word_starts[i];
                let e = sub.word_ends[i].max(s + 30.0);
                (w.to_string(), s, e)
            })
            .collect();
    }
    if sub.word_starts.len() == n {
        return words
            .into_iter()
            .enumerate()
            .map(|(i, w)| {
                let s = sub.word_starts[i];
                let e = if i + 1 < n { sub.word_starts[i + 1].max(s + 30.0) } else { sub.source_end_ms.max(s + 30.0) };
                (w.to_string(), s, e)
            })
            .collect();
    }
    let dur = (sub.source_end_ms - sub.source_start_ms).max(n as f64 * 30.0);
    let total_chars: usize = words.iter().map(|w| w.chars().count() + 1).sum::<usize>().max(1);
    let mut acc = 0usize;
    words
        .into_iter()
        .map(|w| {
            let len = w.chars().count() + 1;
            let s = sub.source_start_ms + dur * (acc as f64 / total_chars as f64);
            acc += len;
            let e = (sub.source_start_ms + dur * (acc as f64 / total_chars as f64)).max(s + 30.0);
            (w.to_string(), s, e)
        })
        .collect()
}

/// Returns true when the majority of `[start_ms, end_ms]` falls outside enabled slices.
pub fn is_range_cut(slices: &[Slice], start_ms: f64, end_ms: f64) -> bool {
    if slices.is_empty() {
        return false;
    }
    let len = end_ms - start_ms;
    if len <= 0.0 {
        return !slices.iter().any(|s| !s.disabled && start_ms >= s.source_start_ms && start_ms <= s.source_end_ms);
    }
    let covered: f64 = slices
        .iter()
        .filter(|s| !s.disabled)
        .map(|s| (s.source_end_ms.min(end_ms) - s.source_start_ms.max(start_ms)).max(0.0))
        .sum();
    covered < len * 0.5
}

/// Ordered transcript words across all subtitles, annotated with whether each word is currently cut.
pub fn extract_words(subtitles: &[Subtitle], slices: &[Slice]) -> Vec<TranscriptWord> {
    let mut sorted: Vec<&Subtitle> = subtitles.iter().collect();
    sorted.sort_by(|a, b| a.source_start_ms.partial_cmp(&b.source_start_ms).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = Vec::new();
    for sub in sorted {
        for (idx, (text, start_ms, end_ms)) in word_spans(sub).into_iter().enumerate() {
            let cut = is_range_cut(slices, start_ms, end_ms);
            out.push(TranscriptWord {
                caption_id: sub.id.clone(),
                word_index: idx,
                text,
                start_ms,
                end_ms,
                cut,
            });
        }
    }
    out
}

pub fn is_filler_word(word: &str) -> bool {
    let clean = word
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_ascii_lowercase();
    matches!(
        clean.as_str(),
        "um" | "uh" | "uhh" | "umm" | "er" | "erm" | "ah" | "ahh" | "hmm" | "hm"
    )
}

/// Indices into `words` of uncut filler words.
pub fn detect_fillers(words: &[TranscriptWord]) -> Vec<usize> {
    words
        .iter()
        .enumerate()
        .filter(|(_, w)| !w.cut && is_filler_word(&w.text))
        .map(|(i, _)| i)
        .collect()
}

/// Gaps between consecutive transcript words of at least `min_gap_ms`:
/// `(after_word_index, cut_start_ms, cut_end_ms, is_cut)`.
pub fn pause_gaps(words: &[TranscriptWord], slices: &[Slice], min_gap_ms: f64) -> Vec<(usize, f64, f64, bool)> {
    let mut out = Vec::new();
    for i in 0..words.len().saturating_sub(1) {
        let gap = words[i + 1].start_ms - words[i].end_ms;
        if gap >= min_gap_ms {
            let a = words[i].end_ms + 30.0;
            let b = words[i + 1].start_ms - 30.0;
            if b - a >= 60.0 {
                out.push((i, a, b, is_range_cut(slices, a, b)));
            }
        }
    }
    out
}

fn merge_ranges(ranges: &[[f64; 2]], merge_gap_ms: f64) -> Vec<[f64; 2]> {
    let mut sorted: Vec<[f64; 2]> = ranges
        .iter()
        .copied()
        .filter(|r| r[1] > r[0])
        .collect();
    sorted.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
    let mut merged: Vec<[f64; 2]> = Vec::new();
    for r in sorted {
        match merged.last_mut() {
            Some(last) if r[0] <= last[1] + merge_gap_ms => {
                last[1] = last[1].max(r[1]);
            }
            _ => merged.push(r),
        }
    }
    merged
}

/// Mark source ranges as jump cuts (disabled slices), merging nearby ranges first and healing
/// adjacent disabled pieces afterwards.
pub fn cut_ranges(slices: &mut Vec<Slice>, ranges: &[[f64; 2]], new_id: impl FnMut() -> String) -> usize {
    let merged = merge_ranges(ranges, 45.0);
    if merged.is_empty() {
        return 0;
    }
    let made = apply_jump_cuts(slices, &merged, new_id);
    heal(slices);
    made
}

/// Restore previously cut source ranges back to active playback, splitting partially covered
/// disabled slices and healing adjacent matching slices afterwards.
pub fn restore_ranges(slices: &mut Vec<Slice>, ranges: &[[f64; 2]], mut new_id: impl FnMut() -> String) -> usize {
    let merged = merge_ranges(ranges, 45.0);
    if merged.is_empty() {
        return 0;
    }
    let mut restored = 0usize;
    let mut out: Vec<Slice> = Vec::with_capacity(slices.len() + merged.len() * 2);
    for s in slices.drain(..) {
        if !s.disabled || s.freeze || s.ramp {
            out.push(s);
            continue;
        }
        let mut hits: Vec<[f64; 2]> = merged
            .iter()
            .map(|r| [r[0].max(s.source_start_ms), r[1].min(s.source_end_ms)])
            .filter(|r| r[1] - r[0] >= 1.0)
            .collect();
        if hits.is_empty() {
            out.push(s);
            continue;
        }
        hits.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
        let mut cursor = s.source_start_ms;
        let mut first = true;
        let mut push = |out: &mut Vec<Slice>, a: f64, b: f64, disabled: bool, first: &mut bool| {
            if b - a < 1.0 {
                return;
            }
            let mut piece = s.clone();
            piece.source_start_ms = a;
            piece.source_end_ms = b;
            piece.disabled = disabled;
            if *first {
                *first = false;
            } else {
                piece.id = new_id();
                piece.transition = None;
            }
            out.push(piece);
        };
        for h in hits {
            if h[0] < cursor {
                continue;
            }
            push(&mut out, cursor, h[0], true, &mut first);
            push(&mut out, h[0], h[1], false, &mut first);
            restored += 1;
            cursor = h[1];
        }
        push(&mut out, cursor, s.source_end_ms, true, &mut first);
    }
    *slices = out;
    heal(slices);
    restored
}

/// Active (non-cut) words and their start times for rendering and SRT export.
pub fn active_caption_words(sub: &Subtitle, slices: &[Slice]) -> Vec<(String, f64)> {
    word_spans(sub)
        .into_iter()
        .filter(|(_, s, e)| !is_range_cut(slices, *s, *e))
        .map(|(w, s, _)| (w, s))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(id: &str, text: &str, starts: Vec<f64>, ends: Vec<f64>, a: f64, b: f64) -> Subtitle {
        Subtitle {
            id: id.into(),
            source_start_ms: a,
            source_end_ms: b,
            text: text.into(),
            word_starts: starts,
            word_ends: ends,
        }
    }

    #[test]
    fn extracts_words_detects_fillers_and_pauses() {
        let s1 = sub("c1", "Hello um world", vec![100.0, 400.0, 700.0], vec![350.0, 620.0, 1000.0], 100.0, 1100.0);
        let s2 = sub("c2", "uh next", vec![2200.0, 2500.0], vec![2400.0, 2900.0], 2200.0, 2900.0);
        let sl = vec![Slice { id: "s0".into(), source_start_ms: 0.0, source_end_ms: 4000.0, ..Default::default() }];

        let words = extract_words(&[s2, s1], &sl);
        assert_eq!(words.len(), 5);
        assert_eq!(words[0].text, "Hello");
        assert_eq!(words[4].text, "next");
        assert_eq!(detect_fillers(&words), vec![1, 3]);

        let pauses = pause_gaps(&words, &sl, 500.0);
        assert_eq!(pauses.len(), 1);
        assert_eq!(pauses[0].0, 2); // gap between "world" (1000ms) and "uh" (2200ms)
        assert!(!pauses[0].3);
    }

    #[test]
    fn cut_heal_restore_roundtrip_and_active_caption_words() {
        let s = sub("c1", "Hello um world", vec![100.0, 400.0, 700.0], vec![350.0, 620.0, 1000.0], 100.0, 1100.0);
        let mut sl = vec![Slice { id: "s0".into(), source_start_ms: 0.0, source_end_ms: 2000.0, ..Default::default() }];
        let mut n = 0;
        let mut next_id = || {
            n += 1;
            format!("s{n}")
        };

        // Cut "um" (400..620)
        assert_eq!(cut_ranges(&mut sl, &[[400.0, 620.0]], &mut next_id), 1);
        assert_eq!(sl.len(), 3);
        let words = extract_words(&[s.clone()], &sl);
        assert!(!words[0].cut);
        assert!(words[1].cut);
        assert!(!words[2].cut);

        let active = active_caption_words(&s, &sl);
        assert_eq!(active, vec![("Hello".into(), 100.0), ("world".into(), 700.0)]);

        // Restore "um" -> should merge back into 1 slice
        assert_eq!(restore_ranges(&mut sl, &[[400.0, 620.0]], &mut next_id), 1);
        assert_eq!(sl.len(), 1);
        assert!(!sl[0].disabled);
        assert_eq!(sl[0].source_start_ms, 0.0);
        assert_eq!(sl[0].source_end_ms, 2000.0);
    }
}
