//! CPU-only English recognition. The caller verifies the pack and supplies decoded 16 kHz mono PCM.
mod features;

use candle_core::{Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::whisper::{self, Config, model::Whisper};
use features::{BANDS, FRAMES, Features};
use std::{fs, path::Path};
use tokenizers::Tokenizer;

// These IDs belong to the pinned English tokenizer. Multilingual models have a different prompt.
const EOT: usize = 50_256;
const SOT: u32 = 50_257;
const NO_SPEECH: usize = 50_361;
const FIRST_TIME: usize = 50_363;
const STEP_MS: u64 = 20;
const MAX_SAMPLES: usize = 480_000;

#[derive(Debug, PartialEq)]
pub(super) struct Segment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

pub(super) struct LocalRecognizer {
    model: Whisper,
    tokenizer: Tokenizer,
    features: Features,
}

impl LocalRecognizer {
    pub fn load(pack_dir: &Path) -> Result<Self, String> {
        let config: Config = serde_json::from_slice(
            &fs::read(pack_dir.join("config.json"))
                .map_err(|e| format!("Cannot read model config: {e}"))?,
        )
        .map_err(|e| format!("Invalid model config: {e}"))?;
        if config.num_mel_bins != BANDS
            || config.vocab_size != FIRST_TIME + 1501
            || config.d_model != 512
            || config.encoder_layers != 6
            || config.decoder_layers != 6
            || config.encoder_attention_heads != 8
            || config.decoder_attention_heads != 8
            || config.max_source_positions != 1_500
            || config.max_target_positions != 448
            || config
                .suppress_tokens
                .iter()
                .any(|&id| id as usize >= config.vocab_size)
        {
            return Err("The local recognizer requires the English base.en model pack.".into());
        }
        let tokenizer = Tokenizer::from_file(pack_dir.join("tokenizer.json"))
            .map_err(|e| format!("Cannot load the English tokenizer: {e}"))?;
        if tokenizer.token_to_id("<|0.00|>") != Some(FIRST_TIME as u32)
            || tokenizer.token_to_id("<|startoftranscript|>") != Some(SOT)
        {
            return Err("Incompatible English tokenizer.".into());
        }
        // The verified weights are copied into model tensors at load. No writable mapped file is retained.
        let weights = fs::read(pack_dir.join("model.safetensors"))
            .map_err(|e| format!("Cannot read model weights: {e}"))?;
        let builder = VarBuilder::from_buffered_safetensors(weights, whisper::DTYPE, &Device::Cpu)
            .map_err(|e| format!("Invalid model weights: {e}"))?;
        let model =
            Whisper::load(&builder, config).map_err(|e| format!("Cannot load base.en: {e}"))?;
        Ok(Self {
            model,
            tokenizer,
            features: Features::new(),
        })
    }

    pub fn recognize(&mut self, pcm: &[i16]) -> Result<Vec<Segment>, String> {
        if pcm.len() > MAX_SAMPLES {
            return Err("A local recognition window exceeds 30 seconds.".into());
        }
        if pcm.iter().all(|sample| *sample == 0) {
            return Ok(Vec::new());
        }
        let duration_ms = (pcm.len() as u64 * 1000).div_ceil(16_000);
        let mel = self.features.compute(pcm);
        let tokens = self
            .decode(mel, duration_ms)
            .map_err(|e| format!("Local speech recognition failed: {e}"))?;
        timed_text(&tokens, duration_ms, |ids| {
            self.tokenizer.decode(ids, false).map_err(|e| e.to_string())
        })
    }

    fn decode(&mut self, mel: Vec<f32>, duration_ms: u64) -> candle_core::Result<Vec<u32>> {
        let spectrogram = Tensor::from_vec(mel, (1, BANDS, FRAMES), &Device::Cpu)?;
        let audio = self.model.encoder.forward(&spectrogram, true)?;
        let mut tokens = vec![SOT];
        let mut no_speech = 0.0;
        let mut log_probability = 0.0;
        let limit = self.model.config.max_target_positions / 2;
        for step in 0..limit {
            let input = Tensor::new(tokens.as_slice(), &Device::Cpu)?.unsqueeze(0)?;
            let hidden = self.model.decoder.forward(&input, &audio, step == 0)?;
            let last = hidden.i((.., tokens.len() - 1.., ..))?;
            let mut logits = self
                .model
                .decoder
                .final_linear(&last)?
                .flatten_all()?
                .to_vec1::<f32>()?;
            if step == 0 {
                no_speech = (logits[NO_SPEECH] - log_sum_exp(&logits)).exp();
            }
            for &token in &self.model.config.suppress_tokens {
                logits[token as usize] = f32::NEG_INFINITY;
            }
            let (token, log_prob) = next_token(&mut logits, &tokens[1..], duration_ms);
            log_probability += log_prob;
            if token == EOT as u32 {
                break;
            }
            tokens.push(token);
        }
        if no_speech > 0.6 && log_probability / (tokens.len() as f32) < -1.0 {
            tokens.clear();
        } else {
            tokens.remove(0);
        }
        Ok(tokens)
    }
}

fn log_sum_exp(scores: &[f32]) -> f32 {
    let peak = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if !peak.is_finite() {
        return peak;
    }
    peak + scores
        .iter()
        .map(|&score| (score - peak).exp())
        .sum::<f32>()
        .ln()
}

// Greedy decoding with Whisper's timestamp grammar. Keep the end token selectable after a closing timestamp.
fn next_token(logits: &mut [f32], sampled: &[u32], duration_ms: u64) -> (u32, f32) {
    let upper = FIRST_TIME + duration_ms.div_ceil(STEP_MS) as usize;
    logits[upper + 1..].fill(f32::NEG_INFINITY);
    logits[EOT + 1..FIRST_TIME].fill(f32::NEG_INFINITY);
    if sampled.is_empty() {
        logits[..FIRST_TIME].fill(f32::NEG_INFINITY);
        logits[FIRST_TIME + 51..].fill(f32::NEG_INFINITY);
    } else {
        let is_time = |token: &u32| *token as usize >= FIRST_TIME;
        let last_time = sampled.last().is_some_and(is_time);
        let previous_time = sampled.len() < 2 || is_time(&sampled[sampled.len() - 2]);
        if last_time {
            if previous_time {
                logits[FIRST_TIME..].fill(f32::NEG_INFINITY);
            } else {
                logits[..EOT].fill(f32::NEG_INFINITY);
            }
        }
        if let Some(&last) = sampled.iter().rev().find(|token| is_time(token)) {
            let minimum = last as usize + usize::from(!last_time || previous_time);
            logits[FIRST_TIME..minimum].fill(f32::NEG_INFINITY);
        }
    }
    let text_max = logits[..FIRST_TIME]
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    if log_sum_exp(&logits[FIRST_TIME..]) > text_max {
        logits[..FIRST_TIME].fill(f32::NEG_INFINITY);
    }
    let (id, &score) = logits
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap();
    if !score.is_finite() {
        return (EOT as u32, 0.0);
    }
    (id as u32, score - log_sum_exp(logits))
}

fn timed_text(
    tokens: &[u32],
    duration_ms: u64,
    decode: impl Fn(&[u32]) -> Result<String, String>,
) -> Result<Vec<Segment>, String> {
    let mut segments = Vec::new();
    let mut words = Vec::new();
    let mut start_ms = 0;
    for &token in tokens {
        if token < EOT as u32 {
            words.push(token);
        } else if token >= FIRST_TIME as u32 {
            let end_ms = (u64::from(token - FIRST_TIME as u32) * STEP_MS).min(duration_ms);
            if !words.is_empty() {
                let text = decode(&words)?.trim().to_owned();
                if !text.is_empty() && end_ms > start_ms {
                    segments.push(Segment {
                        start_ms,
                        end_ms,
                        text,
                    });
                }
                words.clear();
            }
            start_ms = end_ms;
        }
    }
    if !words.is_empty() && duration_ms > start_ms {
        let text = decode(&words)?.trim().to_owned();
        if !text.is_empty() {
            segments.push(Segment {
                start_ms,
                end_ms: duration_ms,
                text,
            });
        }
    }
    Ok(segments)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_track_model_timestamps_and_trim_padding() {
        let t = FIRST_TIME as u32;
        let tokens = [t, 11, t + 50, t + 60, 12, t + 101];
        let parts = timed_text(&tokens, 2_001, |v| Ok(format!(" {}", v[0]))).unwrap();
        assert_eq!(
            parts,
            vec![
                Segment {
                    start_ms: 0,
                    end_ms: 1000,
                    text: "11".into()
                },
                Segment {
                    start_ms: 1200,
                    end_ms: 2001,
                    text: "12".into()
                }
            ]
        );
    }

    #[test]
    fn pair_rules_force_text_after_open_and_allow_eot_after_close() {
        let t = FIRST_TIME as u32;
        let mut logits = vec![-30.0; FIRST_TIME + 1501];
        logits[55] = 1.0;
        logits[FIRST_TIME + 20] = 20.0;
        assert_eq!(next_token(&mut logits.clone(), &[t], 30_000).0, 55);
        logits[EOT] = 25.0;
        assert_eq!(
            next_token(&mut logits, &[t, 55, t + 18], 30_000).0,
            EOT as u32
        );
    }

    #[test]
    fn timestamps_stay_inside_the_input_and_advance_after_text() {
        let t = FIRST_TIME as u32;
        let mut logits = vec![-100.0; FIRST_TIME + 1501];
        logits[FIRST_TIME + 2] = 5.0;
        logits[FIRST_TIME + 4] = 4.0;
        logits[FIRST_TIME + 400] = 99.0;
        assert_eq!(next_token(&mut logits, &[t + 2, 44], 80).0, t + 4);
    }
}
