//! Speech to text for the meeting note taker: one multipart POST per chunk of
//! audio. Modeled on claude.rs — same transport posture, every failure mapped
//! to one polite error. The OpenAI key never leaves this module.
//!
//! This is the second thing in the project that leaves the machine, and the
//! larger one: the analysis call sends app names, this sends what was said in
//! a room. It runs only between an explicit Start and Stop; nothing here is
//! reachable from a timer.

use crate::error::{AppError, Result};
use serde_json::Value;
use std::time::Duration;

const MODEL: &str = "gpt-4o-transcribe";
const API_URL: &str = "https://api.openai.com/v1/audio/transcriptions";

/// The models this app is allowed to ask for. Both return `{"text": ...}` for
/// a plain request, which is the whole of the contract below.
///
/// Diarization models are deliberately absent even though the endpoint accepts
/// them: their request and response shape differs, and the carry-forward
/// `prompt` this app relies on to stitch chunk boundaries is not part of it. A
/// speaker-labelled transcript is a feature, not an environment variable.
const ALLOWED_MODELS: [&str; 2] = [MODEL, "whisper-1"];

/// What language to tell the model to expect. `en` preserves the behavior
/// this shipped with; the API treats it as guidance, not a filter.
const DEFAULT_LANGUAGE: &str = "en";

/// How much of the previous chunk to hand forward as context. Chunk boundaries
/// fall mid-sentence, and the API's `prompt` field exists precisely to tell the
/// model what it just heard, so a word split across the cut still lands.
const CARRY_CHARS: usize = 200;

fn api_key() -> Result<String> {
    let path = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".config/intentionality/openai_key");
    if let Ok(key) = std::fs::read_to_string(&path) {
        let key = key.trim().to_string();
        if !key.is_empty() {
            return Ok(key);
        }
    }
    std::env::var("OPENAI_API_KEY").map_err(|_| {
        AppError::Other("no OpenAI key (~/.config/intentionality/openai_key)".into())
    })
}

/// Fail before the microphone opens rather than after the first chunk.
/// Without this a missing key costs two minutes of recording and produces an
/// empty transcript, with the reason buried in a log line.
pub fn check_key() -> Result<()> {
    api_key().map(|_| ())
}

/// What to ask the transcription endpoint for.
#[derive(Debug, PartialEq)]
pub struct Config {
    pub model: String,
    pub language: String,
}

/// Read and validate the two overrides. Called once before the microphone
/// opens — the same rule as `check_key`, and for the same reason: a typo in an
/// environment variable should cost a Start error, not a recorded meeting that
/// turns out to have been rejected chunk by chunk.
///
/// A model override is not a language setting. Someone recording in German has
/// to say so with `INTENTIONALITY_TRANSCRIBE_LANGUAGE=de`; picking a different
/// model will not do it, and the default here stays `en` so nothing changes
/// for recordings that worked before.
pub fn config() -> Result<Config> {
    let model = match std::env::var("INTENTIONALITY_TRANSCRIBE_MODEL") {
        Ok(raw) if !raw.trim().is_empty() => {
            let want = raw.trim();
            if !ALLOWED_MODELS.contains(&want) {
                return Err(AppError::Other(format!(
                    "INTENTIONALITY_TRANSCRIBE_MODEL={want} is not supported — use one of {}",
                    ALLOWED_MODELS.join(", ")
                )));
            }
            want.to_string()
        }
        _ => MODEL.to_string(),
    };
    let language = match std::env::var("INTENTIONALITY_TRANSCRIBE_LANGUAGE") {
        Ok(raw) if !raw.trim().is_empty() => {
            let want = raw.trim().to_ascii_lowercase();
            // ISO-639-1: two letters, nothing else. Checked here rather than
            // discovered as an API error two minutes into a recording.
            if want.len() != 2 || !want.chars().all(|c| c.is_ascii_alphabetic()) {
                return Err(AppError::Other(format!(
                    "INTENTIONALITY_TRANSCRIBE_LANGUAGE={want} is not an ISO-639-1 code \
                     (two letters, e.g. en, de, fr)"
                )));
            }
            want
        }
        _ => DEFAULT_LANGUAGE.to_string(),
    };
    Ok(Config { model, language })
}

/// The last `chars` characters, on a character boundary. Slicing a String by
/// bytes would panic mid-codepoint the first time someone says a word with an
/// accent in it.
///
/// Shared with the meeting cleaning pass, which stitches its windows together
/// the same way this stitches audio chunks — the seam problem is identical, so
/// the fix should be too.
pub fn tail(text: &str, chars: usize) -> String {
    let trimmed = text.trim();
    match trimmed.char_indices().nth_back(chars.saturating_sub(1)) {
        // The cut can land mid-word, so trim again: a leading fragment is
        // context the model can use, a leading space is just noise.
        Some((i, _)) => trimmed[i..].trim_start().to_string(),
        None => trimmed.to_string(),
    }
}

/// The tail of what has been transcribed so far, for the next chunk's prompt.
pub fn carry(transcript: &str) -> String {
    tail(transcript, CARRY_CHARS)
}

/// Transcribe one WAV. `context` is the tail of the previous chunk.
///
/// The timeout is generous compared with claude.rs's: this uploads several
/// megabytes on a home connection, and a chunk that times out is two minutes
/// of a meeting lost.
pub async fn transcribe(wav: Vec<u8>, context: &str) -> Result<String> {
    let config = config()?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|e| AppError::Other(e.to_string()))?;

    let part = reqwest::multipart::Part::bytes(wav)
        .file_name("chunk.wav")
        .mime_str("audio/wav")
        .map_err(|e| AppError::Other(e.to_string()))?;
    let mut form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", config.model)
        .text("language", config.language);
    if !context.is_empty() {
        form = form.text("prompt", context.to_string());
    }

    let resp = client
        .post(API_URL)
        .header("Authorization", format!("Bearer {}", api_key()?))
        .multipart(form)
        .send()
        .await
        .map_err(|e| AppError::Other(format!("transcription API unreachable: {e}")))?;

    let status = resp.status();
    let payload: Value = resp
        .json()
        .await
        .map_err(|e| AppError::Other(format!("bad transcription response: {e}")))?;
    if !status.is_success() {
        let msg = payload["error"]["message"].as_str().unwrap_or("unknown error");
        return Err(AppError::Other(format!("transcription API {status}: {msg}")));
    }
    Ok(payload["text"].as_str().unwrap_or("").trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carry_takes_the_tail_not_the_head() {
        let long = "a".repeat(500) + "the end";
        let got = carry(&long);
        assert!(got.ends_with("the end"));
        assert_eq!(got.chars().count(), CARRY_CHARS);
    }

    #[test]
    fn carry_returns_short_transcripts_whole() {
        assert_eq!(carry("  so we agreed on Thursday.  "), "so we agreed on Thursday.");
        assert_eq!(carry(""), "");
    }

    /// Byte-slicing a transcript would panic the first time anyone used a word
    /// with an accent in it, and a meeting is exactly where that happens.
    #[test]
    fn carry_does_not_split_a_multibyte_character() {
        let text = "é".repeat(400);
        let got = carry(&text);
        assert_eq!(got.chars().count(), CARRY_CHARS);
        assert!(got.chars().all(|c| c == 'é'));
    }

    /// The environment is process-wide, so these run under one lock and put
    /// every variable back. Without it a `cargo test` thread reading the
    /// config would see another test's override.
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_env(model: Option<&str>, language: Option<&str>) -> Result<Config> {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let before = (
            std::env::var("INTENTIONALITY_TRANSCRIBE_MODEL").ok(),
            std::env::var("INTENTIONALITY_TRANSCRIBE_LANGUAGE").ok(),
        );
        let set = |k: &str, v: Option<&str>| match v {
            Some(v) => std::env::set_var(k, v),
            None => std::env::remove_var(k),
        };
        set("INTENTIONALITY_TRANSCRIBE_MODEL", model);
        set("INTENTIONALITY_TRANSCRIBE_LANGUAGE", language);
        let got = config();
        set("INTENTIONALITY_TRANSCRIBE_MODEL", before.0.as_deref());
        set("INTENTIONALITY_TRANSCRIBE_LANGUAGE", before.1.as_deref());
        got
    }

    #[test]
    fn the_default_configuration_is_what_shipped() {
        let got = with_env(None, None).unwrap();
        assert_eq!(got.model, "gpt-4o-transcribe");
        assert_eq!(got.language, "en");
    }

    #[test]
    fn an_allowed_model_is_accepted() {
        assert_eq!(with_env(Some("whisper-1"), None).unwrap().model, "whisper-1");
        assert_eq!(with_env(Some(" whisper-1 "), None).unwrap().model, "whisper-1");
        // Empty is not a choice; it is an unset variable with extra steps.
        assert_eq!(with_env(Some(""), None).unwrap().model, MODEL);
    }

    /// Diarization is the specific thing this rejects: the endpoint would take
    /// it and hand back a shape this app does not read.
    #[test]
    fn an_unknown_model_is_refused_with_the_allowed_list() {
        let err = with_env(Some("gpt-4o-transcribe-diarize"), None).unwrap_err().to_string();
        assert!(err.contains("gpt-4o-transcribe-diarize"), "{err}");
        assert!(err.contains("whisper-1"), "{err}");
    }

    #[test]
    fn a_language_override_is_validated_as_iso_639_1() {
        assert_eq!(with_env(None, Some("de")).unwrap().language, "de");
        assert_eq!(with_env(None, Some("DE")).unwrap().language, "de");
        for bad in ["deu", "e", "e1", "german"] {
            assert!(with_env(None, Some(bad)).is_err(), "{bad} was accepted");
        }
    }

    /// A model override is not a language setting — the plan's words, and the
    /// mistake worth a test.
    #[test]
    fn changing_the_model_does_not_change_the_language() {
        assert_eq!(with_env(Some("whisper-1"), None).unwrap().language, "en");
    }
}
