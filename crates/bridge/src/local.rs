//! Whisper inside the program (whisper.cpp, on the GPU of a Mac), for an app that cannot run `whisper-cli`: a sandboxed
//! app. It reads the samples the app decoded (16 kHz, mono, 32-bit float, little-endian, no header) and gives the text,
//! with the same care as the command line: silence and what Whisper makes up on it give nothing, the lexicon corrects.

use std::path::Path;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::Error;
use crate::whisper::{Lexicon, clean, parasite};

/// Below this peak (-50 dB) there is no sound at all: the microphone was off.
const SILENCE: f32 = 0.003_16;

/// What `pcm` says, or `None` for nothing audible. `lexicon`: a Markdown table of terms and their wrong spellings.
pub fn transcribe(
    model: &Path,
    pcm: &Path,
    lang: &str,
    threads: u32,
    lexicon: Option<&Path>,
) -> Result<Option<String>, Error> {
    let bytes = std::fs::read(pcm).map_err(|e| Error::Io(e.to_string()))?;
    let samples: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    if samples.iter().fold(0f32, |m, s| m.max(s.abs())) < SILENCE {
        return Ok(None);
    }
    let model = model
        .to_str()
        .ok_or_else(|| Error::Invalid("the model's path is not text".into()))?;
    let ctx = WhisperContext::new_with_params(model, WhisperContextParameters::default())
        .map_err(|e| Error::Invalid(format!("the speech model cannot be read: {e}")))?;
    let mut state = ctx
        .create_state()
        .map_err(|e| Error::Invalid(format!("whisper: {e}")))?;
    let lexicon = Lexicon::load(lexicon);
    let vocab = lexicon.prompt();
    let mut params = FullParams::new(SamplingStrategy::BeamSearch {
        beam_size: 5,
        patience: -1.0,
    });
    params.set_language(Some(lang));
    params.set_n_threads(i32::try_from(threads).unwrap_or(4));
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_print_special(false);
    if !vocab.is_empty() {
        params.set_initial_prompt(&vocab);
    }
    state
        .full(params, &samples)
        .map_err(|e| Error::Invalid(format!("whisper failed: {e}")))?;
    let mut lines = Vec::new();
    for segment in state.as_iter() {
        let t = segment.to_string();
        let t = t.trim();
        if !t.is_empty() {
            lines.push(t.to_string());
        }
    }
    let text = clean(&lines.join("\n"));
    if text.is_empty() || parasite(&text) {
        return Ok(None);
    }
    Ok(Some(lexicon.correct(&text)))
}
