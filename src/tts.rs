//! Text-to-speech — bring-your-own voice. When enabled, a finished assistant reply is split into
//! sentences and synthesized in order on a worker thread that owns the audio output; the worker
//! also emits an RMS amplitude envelope per sentence so the avatar's mouth tracks the spoken voice
//! (replacing the text heuristic while TTS is on).
//!
//! Two BYO presets, both normalizing to raw PCM s16le / mono / 24 kHz so there is ONE decode +
//! playback path:
//!   - raw-PCM-stream: `POST {base}/v1/tts/stream` `{text,voice}` → raw headerless PCM.
//!   - OpenAI-compatible: `POST {base}/audio/speech` `{model,voice,input,response_format:"pcm"}`.
//!
//! ⚠️ The bytes are headerless raw `i16` LE — decoded directly, NOT through a WAV/format sniffer
//! (a WAV decoder would choke on / misread the first samples). A `RIFF` guard skips an accidental
//! 44-byte header just in case a server ignores `response_format`.

use crate::brain::{ChatState, Role};
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, unbounded};
use rodio::buffer::SamplesBuffer;
use rodio::{OutputStream, Sink};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use vrm_stage_core::LipSync;

/// Output sample rate of both providers.
const SAMPLE_RATE: u32 = 24_000;
/// Envelope frame size (~40 ms at 24 kHz) and the matching lipsync frame duration.
const ENV_FRAME_SAMPLES: usize = 960;
const ENV_FRAME_SECS: f32 = 0.04;

pub struct TtsPlugin;

impl Plugin for TtsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TtsConfig>()
            .add_systems(Startup, spawn_worker)
            .add_systems(Update, (tts_on_finish, pump_envelopes));
    }
}

/// Which BYO backend to call.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TtsProviderKind {
    /// `POST {base}/v1/tts/stream` `{text,voice}` → raw PCM s16le/24k/mono (e.g. raw-PCM).
    RawPcmStream,
    /// `POST {base}/audio/speech` `{model,voice,input,response_format:"pcm"}` (OpenAI-compatible).
    OpenAiSpeech,
}

/// Persisted voice settings (BYO endpoint — nothing is hardcoded).
#[derive(Resource, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TtsConfig {
    pub enabled: bool,
    pub provider: TtsProviderKind,
    pub base_url: String,
    pub voice: String,
    pub model: String,
    pub api_key: String,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: TtsProviderKind::OpenAiSpeech,
            base_url: String::new(),
            voice: "alloy".into(),
            model: "tts-1".into(),
            api_key: String::new(),
        }
    }
}

/// A unit of speech for the worker: the sentence plus a snapshot of the config to synth it with.
struct TtsJob {
    text: String,
    cfg: TtsConfig,
}

/// An RMS amplitude envelope for one synthesized sentence, sent back to drive lipsync.
struct Envelope {
    amps: Vec<f32>,
    frame_secs: f32,
}

/// Channels to the audio worker (jobs out, envelopes back).
#[derive(Resource)]
struct TtsChannels {
    jobs: Sender<TtsJob>,
    env_rx: Receiver<Envelope>,
}

fn spawn_worker(mut commands: Commands) {
    let (job_tx, job_rx) = unbounded::<TtsJob>();
    let (env_tx, env_rx) = unbounded::<Envelope>();
    std::thread::spawn(move || worker_loop(job_rx, env_tx));
    commands.insert_resource(TtsChannels {
        jobs: job_tx,
        env_rx,
    });
}

/// When a reply finishes streaming, split it into sentences and queue them for the worker (in order).
fn tts_on_finish(
    chat: Res<ChatState>,
    cfg: Res<TtsConfig>,
    channels: Option<Res<TtsChannels>>,
    mut was_streaming: Local<bool>,
) {
    let now = chat.streaming;
    let just_finished = *was_streaming && !now;
    *was_streaming = now;

    if !just_finished || !cfg.enabled {
        return;
    }
    let Some(channels) = channels else { return };
    let Some(last) = chat.messages.last() else {
        return;
    };
    if last.role != Role::Assistant {
        return;
    }
    for sentence in split_sentences(&last.text) {
        let _ = channels.jobs.send(TtsJob {
            text: sentence,
            cfg: cfg.clone(),
        });
    }
}

/// Apply envelopes coming back from the worker to the avatar's mouth.
fn pump_envelopes(channels: Option<Res<TtsChannels>>, mut lips: Query<&mut LipSync>) {
    let Some(channels) = channels else { return };
    while let Ok(env) = channels.env_rx.try_recv() {
        for mut lip in &mut lips {
            lip.speak_envelope(&env.amps, env.frame_secs);
        }
    }
}

// ─────────────────────────── worker thread (owns audio) ───────────────────────────

fn worker_loop(jobs: Receiver<TtsJob>, env_tx: Sender<Envelope>) {
    // Open the default audio device. If unavailable (headless/no device), we still synth and emit
    // envelopes so the mouth moves — just silently.
    let stream = OutputStream::try_default();
    let sink = match &stream {
        Ok((_stream, handle)) => Sink::try_new(handle).ok(),
        Err(e) => {
            eprintln!("[tts] no audio output ({e}); lipsync-only mode");
            None
        }
    };

    while let Ok(job) = jobs.recv() {
        match synth(&job) {
            Ok(pcm) if !pcm.is_empty() => {
                let amps = envelope(&pcm);
                let _ = env_tx.send(Envelope {
                    amps,
                    frame_secs: ENV_FRAME_SECS,
                });
                if let Some(sink) = &sink {
                    sink.append(SamplesBuffer::new(1, SAMPLE_RATE, pcm));
                    // Keep sentences (and their envelopes) aligned: wait out this one before next.
                    while !sink.empty() {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
            }
            Ok(_) => {}
            Err(e) => eprintln!("[tts] synth failed: {e}"),
        }
    }
}

/// Blocking synth call → raw PCM samples. Both providers return headerless s16le/24k/mono.
fn synth(job: &TtsJob) -> Result<Vec<i16>, String> {
    let cfg = &job.cfg;
    if cfg.base_url.trim().is_empty() {
        return Err("no TTS base URL configured".into());
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let base = cfg.base_url.trim_end_matches('/');

    let (url, body, use_bearer) = match cfg.provider {
        TtsProviderKind::RawPcmStream => (
            format!("{base}/v1/tts/stream"),
            serde_json::json!({ "text": job.text, "voice": cfg.voice }),
            false,
        ),
        TtsProviderKind::OpenAiSpeech => (
            format!("{base}/audio/speech"),
            serde_json::json!({
                "model": cfg.model,
                "voice": cfg.voice,
                "input": job.text,
                "response_format": "pcm",
            }),
            true,
        ),
    };

    let mut req = client.post(&url).json(&body);
    if use_bearer && !cfg.api_key.is_empty() {
        req = req.bearer_auth(&cfg.api_key);
    }
    let resp = req.send().map_err(|e| format!("request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("server returned {}", resp.status()));
    }
    let bytes = resp.bytes().map_err(|e| format!("read body failed: {e}"))?;
    Ok(decode_pcm_s16le(&bytes))
}

/// Raw little-endian `i16` samples. Skips an accidental 44-byte `RIFF`/WAV header if present.
fn decode_pcm_s16le(bytes: &[u8]) -> Vec<i16> {
    let data = if bytes.len() > 44 && &bytes[0..4] == b"RIFF" {
        &bytes[44..]
    } else {
        bytes
    };
    data.chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect()
}

/// Per-frame RMS amplitude (0..1-ish) over ~40 ms windows.
fn envelope(pcm: &[i16]) -> Vec<f32> {
    pcm.chunks(ENV_FRAME_SAMPLES)
        .map(|frame| {
            let sum_sq: f64 = frame
                .iter()
                .map(|&s| {
                    let f = s as f64 / i16::MAX as f64;
                    f * f
                })
                .sum();
            ((sum_sq / frame.len().max(1) as f64).sqrt() as f32).min(1.0)
        })
        .collect()
}

/// Split text into sentence-ish chunks for ordered, lower-latency playback.
fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        cur.push(ch);
        if matches!(ch, '.' | '!' | '?' | '\n') {
            let t = cur.trim();
            if !t.is_empty() {
                out.push(t.to_string());
            }
            cur.clear();
        }
    }
    let t = cur.trim();
    if !t.is_empty() {
        out.push(t.to_string());
    }
    out
}
