use anyhow::{Context, Result, bail};
use membrie_core::{
    AttachmentProcessingJob, AudioTranscription, attachment_blob_path, speech_model_path,
    transcription_spool_dir,
};
use serde::Deserialize;
use std::fs;
use std::process::{Command, Stdio};
use uuid::Uuid;

const FFMPEG: &str = "/usr/bin/ffmpeg";
const FFPROBE: &str = "/usr/bin/ffprobe";
const TIMEOUT: &str = "/usr/bin/timeout";
const WHISPER_CLI: &str = "/usr/bin/whisper-cli";
const MODEL_NAME: &str = "whisper.cpp/base.en-q5_1";
const MAX_AUDIO_SECONDS: f64 = 30.0 * 60.0;
const MAX_JSON_BYTES: u64 = 16 * 1024 * 1024;
const MAX_TRANSCRIPT_CHARACTERS: usize = 100_000;

pub fn is_ready() -> bool {
    [FFMPEG, FFPROBE, TIMEOUT, WHISPER_CLI]
        .iter()
        .all(installed_executable)
        && fs::symlink_metadata(speech_model_path())
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 20 * 1024 * 1024)
}

pub fn transcribe(job: &AttachmentProcessingJob) -> Result<AudioTranscription> {
    if job.kind != "attachment_audio" {
        bail!("the attachment job is not an audio transcription");
    }
    if !is_ready() {
        bail!("local audio transcription is not installed");
    }
    let input = validated_attachment_path(job)?;
    let duration = audio_duration(&input)?;
    if !(0.05..=MAX_AUDIO_SECONDS).contains(&duration) {
        bail!("voice notes must be between a moment and 30 minutes long");
    }

    let work = TranscriptionWork::create()?;
    let wav_path = work.path.join("voice-note.wav");
    let ffmpeg_status = Command::new(TIMEOUT)
        .args(["--kill-after=10s", "5m", FFMPEG, "-nostdin", "-hide_banner"])
        .args(["-loglevel", "error", "-threads", "1", "-i"])
        .arg(&input)
        .args(["-vn", "-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le", "-y"])
        .arg(&wav_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("could not start the local audio decoder")?;
    if !ffmpeg_status.success() || !regular_file(&wav_path) {
        bail!("the attached audio could not be decoded locally");
    }

    let output_prefix = work.path.join("transcript");
    let threads = std::thread::available_parallelism()
        .map(|count| count.get().min(4))
        .unwrap_or(2)
        .to_string();
    let whisper_status = Command::new(TIMEOUT)
        .args(["--kill-after=15s", "45m", WHISPER_CLI, "-m"])
        .arg(speech_model_path())
        .args(["-l", "en", "-t", &threads, "-sns", "-ojf", "-np", "-of"])
        .arg(&output_prefix)
        .args(["-f"])
        .arg(&wav_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("could not start the local speech model")?;
    let json_path = output_prefix.with_extension("json");
    if !whisper_status.success() || !regular_file(&json_path) {
        bail!("the local speech model could not transcribe this recording");
    }
    let metadata = fs::metadata(&json_path)?;
    if metadata.len() == 0 || metadata.len() > MAX_JSON_BYTES {
        bail!("the local speech model returned an invalid result");
    }
    let json_bytes =
        fs::read(&json_path).context("the local speech transcript could not be read")?;
    parse_whisper_bytes(&json_bytes)
}

fn audio_duration(path: &std::path::Path) -> Result<f64> {
    let output = Command::new(TIMEOUT)
        .args(["--kill-after=2s", "15s", FFPROBE, "-v", "error"])
        .args(["-show_entries", "format=duration"])
        .args(["-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .context("could not inspect the local audio attachment")?;
    if !output.status.success() || output.stdout.len() > 128 {
        bail!("the attached audio duration could not be read");
    }
    let value = std::str::from_utf8(&output.stdout)
        .context("the attached audio duration was invalid")?
        .trim()
        .parse::<f64>()
        .context("the attached audio duration was invalid")?;
    if !value.is_finite() {
        bail!("the attached audio duration was invalid");
    }
    Ok(value)
}

fn validated_attachment_path(job: &AttachmentProcessingJob) -> Result<std::path::PathBuf> {
    let hash = &job.attachment.blob_hash;
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("the stored attachment hash was invalid");
    }
    let path = attachment_blob_path(hash);
    let metadata =
        fs::symlink_metadata(&path).context("the stored audio attachment was unavailable")?;
    if !metadata.file_type().is_file()
        || metadata.len() != job.attachment.byte_size
        || metadata.len() > 100 * 1024 * 1024
    {
        bail!("the stored audio attachment failed its size or type check");
    }
    Ok(path)
}

fn regular_file(path: impl AsRef<std::path::Path>) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

fn installed_executable(path: impl AsRef<std::path::Path>) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
}

struct TranscriptionWork {
    path: std::path::PathBuf,
}

impl TranscriptionWork {
    fn create() -> Result<Self> {
        let parent = transcription_spool_dir();
        fs::create_dir_all(&parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
        }
        let path = parent.join(Uuid::now_v7().to_string());
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(&path)?;
        }
        #[cfg(not(unix))]
        fs::create_dir(&path)?;
        Ok(Self { path })
    }
}

impl Drop for TranscriptionWork {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Debug, Deserialize)]
struct WhisperOutput {
    #[serde(default)]
    result: WhisperResult,
    #[serde(default)]
    transcription: Vec<WhisperSegment>,
}

#[derive(Debug, Default, Deserialize)]
struct WhisperResult {
    #[serde(default)]
    language: String,
}

#[derive(Debug, Deserialize)]
struct WhisperSegment {
    #[serde(default)]
    offsets: WhisperOffsets,
    #[serde(default)]
    text: String,
    #[serde(default)]
    tokens: Vec<WhisperToken>,
}

#[derive(Debug, Default, Deserialize)]
struct WhisperOffsets {
    #[serde(default)]
    from: u64,
}

#[derive(Debug, Deserialize)]
struct WhisperToken {
    #[serde(default)]
    text: String,
    #[serde(default)]
    p: f64,
}

fn parse_whisper_output(json: &str) -> Result<AudioTranscription> {
    let output: WhisperOutput =
        serde_json::from_str(json).context("the local speech model returned invalid JSON")?;
    let speech_segments = output
        .transcription
        .iter()
        .filter(|segment| contains_speech(&segment.text))
        .collect::<Vec<_>>();
    let speech_detected = !speech_segments.is_empty();
    let transcript = if speech_detected {
        let mut text = String::new();
        for segment in speech_segments {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&format!(
                "[{}] {}",
                timestamp(segment.offsets.from),
                segment.text.trim()
            ));
            if text.chars().count() >= MAX_TRANSCRIPT_CHARACTERS {
                break;
            }
        }
        truncate_chars(&text, MAX_TRANSCRIPT_CHARACTERS)
    } else {
        String::new()
    };
    let probabilities = output
        .transcription
        .iter()
        .flat_map(|segment| &segment.tokens)
        .filter(|token| {
            !token.text.starts_with("<|")
                && token.text.chars().any(char::is_alphanumeric)
                && token.p.is_finite()
        })
        .map(|token| token.p.clamp(0.0, 1.0))
        .collect::<Vec<_>>();
    let average = if probabilities.is_empty() {
        0.0
    } else {
        probabilities.iter().sum::<f64>() / probabilities.len() as f64
    };
    let confidence = if !speech_detected || average < 0.55 {
        "low"
    } else if average < 0.80 {
        "medium"
    } else {
        "high"
    };
    Ok(AudioTranscription {
        transcript,
        language: output.result.language,
        speech_detected,
        confidence: confidence.to_owned(),
        model: MODEL_NAME.to_owned(),
    })
}

fn parse_whisper_bytes(json: &[u8]) -> Result<AudioTranscription> {
    let json = String::from_utf8_lossy(json);
    parse_whisper_output(&json)
}

fn contains_speech(text: &str) -> bool {
    let compact = text
        .trim()
        .trim_matches(|character: char| {
            character.is_whitespace()
                || character.is_ascii_punctuation()
                || matches!(character, '♪' | '♫' | '♬')
        })
        .to_ascii_lowercase();
    if compact.is_empty() {
        return false;
    }
    !matches!(
        compact.as_str(),
        "music"
            | "instrumental"
            | "silence"
            | "noise"
            | "background noise"
            | "applause"
            | "laughter"
            | "singing"
            | "inaudible"
            | "blank audio"
    )
}

fn timestamp(milliseconds: u64) -> String {
    let total_seconds = milliseconds / 1000;
    format!("{:02}:{:02}", total_seconds / 60, total_seconds % 60)
}

fn truncate_chars(value: &str, maximum: usize) -> String {
    if value.chars().count() <= maximum {
        return value.to_owned();
    }
    value.chars().take(maximum).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_timestamped_speech_and_confidence() {
        let result = parse_whisper_output(
            r#"{
                "result":{"language":"en"},
                "transcription":[
                    {"offsets":{"from":1200,"to":3100},"text":" Remember to call Duke.",
                     "tokens":[{"text":" Remember","p":0.92},{"text":" call","p":0.86}]}
                ]
            }"#,
        )
        .unwrap();
        assert!(result.speech_detected);
        assert_eq!(result.transcript, "[00:01] Remember to call Duke.");
        assert_eq!(result.confidence, "high");
        assert_eq!(result.language, "en");
    }

    #[test]
    fn labels_music_only_audio_without_inventing_a_transcript() {
        let result = parse_whisper_output(
            r#"{
                "result":{"language":"en"},
                "transcription":[
                    {"offsets":{"from":0},"text":" [Music]","tokens":[{"text":" music","p":0.97}]},
                    {"offsets":{"from":9000},"text":" ♪","tokens":[]}
                ]
            }"#,
        )
        .unwrap();
        assert!(!result.speech_detected);
        assert!(result.transcript.is_empty());
        assert_eq!(result.confidence, "low");
    }

    #[test]
    fn replaces_invalid_model_bytes_without_losing_the_result() {
        let mut json = br#"{
            "result":{"language":"en"},
            "transcription":[{"offsets":{"from":0},"text":" Voice note "#
            .to_vec();
        json.push(0xff);
        json.extend_from_slice(br#" text","tokens":[{"text":" Voice","p":0.8}]}]}"#);
        let result = parse_whisper_bytes(&json).unwrap();
        assert!(result.speech_detected);
        assert!(result.transcript.contains("Voice note"));
    }
}
