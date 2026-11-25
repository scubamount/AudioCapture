//! Whisper's centered STFT and area-normalized Slaney mel filters for 16 kHz PCM.
use rustfft::{Fft, FftPlanner, num_complex::Complex32};
use std::{f64::consts::TAU, sync::Arc};

pub(super) const FRAMES: usize = 3_000;
pub(super) const BANDS: usize = 80;
const FFT: usize = 400;
const HOP: usize = 160;

pub(super) struct Features {
    fft: Arc<dyn Fft<f32>>,
    window: [f32; FFT],
    filters: Vec<Vec<(usize, f32)>>,
    scratch: Vec<Complex32>,
}

impl Features {
    pub(super) fn new() -> Self {
        let fft = FftPlanner::new().plan_fft_forward(FFT);
        let window =
            std::array::from_fn(|i| (0.5 - 0.5 * (TAU * i as f64 / FFT as f64).cos()) as f32);
        // Slaney's linear scale below 1 kHz and logarithmic scale above it.
        let log_step = (6.4_f64).ln() / 27.0;
        let top_mel = 15.0 + 8.0_f64.ln() / log_step;
        let edges: Vec<f64> = (0..BANDS + 2)
            .map(|i| {
                let mel = top_mel * i as f64 / (BANDS + 1) as f64;
                if mel < 15.0 {
                    mel * (200.0 / 3.0)
                } else {
                    1000.0 * ((mel - 15.0) * log_step).exp()
                }
            })
            .collect();
        let filters = edges
            .windows(3)
            .map(|limits| {
                let (left, center, right) = (limits[0], limits[1], limits[2]);
                (0..=FFT / 2)
                    .filter_map(|bin| {
                        let hz = bin as f64 * 16000.0 / FFT as f64;
                        let triangle =
                            ((hz - left) / (center - left)).min((right - hz) / (right - center));
                        (triangle > 0.0).then_some((bin, (2.0 * triangle / (right - left)) as f32))
                    })
                    .collect()
            })
            .collect();
        let scratch = vec![Complex32::default(); fft.get_inplace_scratch_len()];
        Self {
            fft,
            window,
            filters,
            scratch,
        }
    }

    pub(super) fn compute(&mut self, pcm: &[i16]) -> Vec<f32> {
        // All-zero padding has log10 power -10. Skip its FFTs for partial windows.
        let mut mel = vec![-10.0_f32; BANDS * FRAMES];
        let mut spectrum = [Complex32::default(); FFT];
        let mut powers = [0.0_f32; FFT / 2 + 1];
        let active_frames = (pcm.len() + FFT / 2).div_ceil(HOP).min(FRAMES);
        let mut peak = -10.0_f32;
        for frame in 0..active_frames {
            for (n, value) in spectrum.iter_mut().enumerate() {
                let sample = (frame * HOP + n).abs_diff(FFT / 2);
                let level = pcm.get(sample).copied().unwrap_or(0) as f32 / 32768.0;
                *value = Complex32::new(self.window[n] * level, 0.0);
            }
            self.fft
                .process_with_scratch(&mut spectrum, &mut self.scratch);
            for (power, bin) in powers.iter_mut().zip(&spectrum) {
                *power = bin.norm_sqr();
            }
            for (band, filter) in self.filters.iter().enumerate() {
                let power: f32 = filter
                    .iter()
                    .map(|&(bin, weight)| powers[bin] * weight)
                    .sum();
                let log = power.max(1.0e-10).log10();
                peak = peak.max(log);
                mel[band * FRAMES + frame] = log;
            }
        }
        let floor = peak - 8.0;
        for sample in &mut mel {
            *sample = (sample.max(floor) + 4.0) / 4.0;
        }
        mel
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_pcm_uses_a_fixed_window_and_preserves_the_silence_floor() {
        let mut features = Features::new();
        let mel = features.compute(&[0; 160]);
        assert_eq!(mel.len(), BANDS * FRAMES);
        assert!(mel.iter().all(|value| *value == -1.5));
    }

    #[test]
    fn one_khz_tone_excites_its_mel_band() {
        let pcm: Vec<i16> = (0..3_200)
            .map(|i| ((i as f64 * TAU / 16.0).sin() * 16_384.0) as i16)
            .collect();
        let mel = Features::new().compute(&pcm);
        let loudest = (0..BANDS)
            .max_by(|&a, &b| mel[a * FRAMES + 5].total_cmp(&mel[b * FRAMES + 5]))
            .unwrap();
        assert_eq!(loudest, 26);
        // Independent periodic-Hann FFT and Slaney filter reference for this tone.
        assert!((mel[loudest * FRAMES + 5] - 1.43965).abs() < 0.0001);
        assert!(mel[loudest * FRAMES + 5] - mel[loudest * FRAMES + 100] > 1.9);
    }
}
