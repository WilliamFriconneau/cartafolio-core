//! Whisper inside the program, on a real model: run by hand, with `MEMCP_WHISPER_MODEL` (a ggml model) and
//! `MEMCP_WHISPER_PCM` (16 kHz mono f32 samples), `cargo test -p memcp-capture --features metal --test local_whisper -- --ignored`.
#![cfg(feature = "local-whisper")]
#![allow(clippy::expect_used)]

#[test]
#[ignore = "needs a model of some 550 MB"]
fn a_model_on_this_machine_reads_a_sentence() {
    let model = std::env::var("MEMCP_WHISPER_MODEL").expect("MEMCP_WHISPER_MODEL");
    let pcm = std::env::var("MEMCP_WHISPER_PCM").expect("MEMCP_WHISPER_PCM");
    let text = memcp_capture::local::transcribe(model.as_ref(), pcm.as_ref(), "fr", 8, None)
        .expect("transcribe")
        .expect("some text");
    println!("{text}");
    assert!(text.to_lowercase().contains("vendredi"), "{text}");
    // Silence gives nothing.
    let silent = std::env::temp_dir().join("memcp-silence.f32");
    std::fs::write(&silent, vec![0u8; 16_000 * 4 * 2]).expect("write");
    assert!(
        memcp_capture::local::transcribe(model.as_ref(), &silent, "fr", 8, None)
            .expect("silence")
            .is_none()
    );
}
