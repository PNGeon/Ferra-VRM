//! Speech-to-text — "Ears". Push-to-talk (hold F2): the mic is captured on a `!Send`-safe cpal
//! thread, the utterance is transcribed locally by a **pure-Rust Whisper** (candle) on a second
//! worker thread, and the text is submitted to the LLM — closing the voice loop
//! (speak → LLM → TTS speaks back). Nothing touches the render thread.
//!
//! The Whisper model is downloaded from HuggingFace on first use (via the `reqwest` we already
//! ship) and cached under the config dir, so the repo stays light. CPU by default.

use crate::brain::{ChatState, SubmitPrompt};
use crate::toast::Toast;
use bevy::prelude::*;
use candle_core::{Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::whisper;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{Receiver, Sender, unbounded};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokenizers::Tokenizer;

pub struct SttPlugin;

impl Plugin for SttPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SttConfig>()
            .add_systems(Startup, spawn_threads)
            .add_systems(Update, (push_to_talk, pump_transcript));
    }
}

/// Default push-to-talk key (chosen to not collide with chat text entry).
const PUSH_TO_TALK: KeyCode = KeyCode::F2;
/// Cap the decode loop (Whisper text context is 448; utterances are short).
const MAX_DECODE_TOKENS: usize = 224;

/// Speech-to-text settings (persisted with the rest of the config).
#[derive(Resource, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SttConfig {
    pub enabled: bool,
    /// HuggingFace model id, downloaded on first use.
    pub model: String,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model: "distil-whisper/distil-small.en".into(),
        }
    }
}

enum CaptureCmd {
    Start,
    Stop,
}

/// A finished utterance: mono f32 at the device sample rate.
struct CaptureResult {
    samples: Vec<f32>,
    sample_rate: u32,
}

/// Inference worker → main world.
enum TranscriptEvent {
    Status(String),
    Text(String),
    Error(String),
}

#[derive(Resource)]
struct SttChannels {
    cmd: Sender<CaptureCmd>,
    transcript_rx: Receiver<TranscriptEvent>,
}

fn spawn_threads(mut commands: Commands, cfg: Res<SttConfig>) {
    let (cmd_tx, cmd_rx) = unbounded::<CaptureCmd>();
    let (cap_tx, cap_rx) = unbounded::<CaptureResult>();
    let (tr_tx, transcript_rx) = unbounded::<TranscriptEvent>();
    let model = cfg.model.clone();
    std::thread::spawn(move || capture_loop(cmd_rx, cap_tx));
    std::thread::spawn(move || inference_loop(cap_rx, tr_tx, model));
    commands.insert_resource(SttChannels {
        cmd: cmd_tx,
        transcript_rx,
    });
}

/// Hold the push-to-talk key → Start; release → Stop.
fn push_to_talk(
    input: Res<ButtonInput<KeyCode>>,
    cfg: Res<SttConfig>,
    channels: Option<Res<SttChannels>>,
) {
    if !cfg.enabled {
        return;
    }
    let Some(channels) = channels else { return };
    if input.just_pressed(PUSH_TO_TALK) {
        let _ = channels.cmd.send(CaptureCmd::Start);
    }
    if input.just_released(PUSH_TO_TALK) {
        let _ = channels.cmd.send(CaptureCmd::Stop);
    }
}

/// Drain transcription events: status/errors → toasts; recognized speech → submit to the LLM.
fn pump_transcript(
    channels: Option<Res<SttChannels>>,
    chat: Res<ChatState>,
    mut submit: MessageWriter<SubmitPrompt>,
    mut toasts: MessageWriter<Toast>,
) {
    let Some(channels) = channels else { return };
    while let Ok(event) = channels.transcript_rx.try_recv() {
        match event {
            TranscriptEvent::Status(s) => {
                toasts.write(Toast(s));
            }
            TranscriptEvent::Error(e) => {
                toasts.write(Toast(format!("STT: {e}")));
            }
            TranscriptEvent::Text(t) => {
                let t = t.trim().to_string();
                if t.is_empty() {
                    toasts.write(Toast("🎤 (didn't catch that)".into()));
                    continue;
                }
                // Always show what was heard (feedback even with no LLM configured);
                // only auto-submit when the model isn't mid-reply.
                toasts.write(Toast(format!("🎤 {t}")));
                if !chat.streaming {
                    submit.write(SubmitPrompt(t));
                }
            }
        }
    }
}

// ─────────────────────────── capture thread (owns the cpal stream) ───────────────────────────

fn capture_loop(cmd_rx: Receiver<CaptureCmd>, result_tx: Sender<CaptureResult>) {
    let host = cpal::default_host();
    while let Ok(cmd) = cmd_rx.recv() {
        if !matches!(cmd, CaptureCmd::Start) {
            continue;
        }
        let Some(device) = host.default_input_device() else {
            eprintln!("[stt] no input device");
            wait_for_stop(&cmd_rx);
            continue;
        };
        let Ok(supported) = device.default_input_config() else {
            eprintln!("[stt] no default input config");
            wait_for_stop(&cmd_rx);
            continue;
        };
        let sample_rate = supported.sample_rate().0;
        let channels = supported.config().channels as usize;
        let buf = Arc::new(Mutex::new(Vec::<f32>::new()));
        let cfg = supported.config();
        let err_fn = |e| eprintln!("[stt] input stream error: {e}");

        let b = buf.clone();
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &cfg,
                move |data: &[f32], _: &_| append_mono(&b, data, channels),
                err_fn,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                &cfg,
                move |data: &[i16], _: &_| {
                    let f: Vec<f32> = data.iter().map(|s| *s as f32 / 32768.0).collect();
                    append_mono(&b, &f, channels);
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream(
                &cfg,
                move |data: &[u16], _: &_| {
                    let f: Vec<f32> = data.iter().map(|s| (*s as f32 / 32768.0) - 1.0).collect();
                    append_mono(&b, &f, channels);
                },
                err_fn,
                None,
            ),
            other => {
                eprintln!("[stt] unsupported sample format: {other:?}");
                wait_for_stop(&cmd_rx);
                continue;
            }
        };

        let Ok(stream) = stream else {
            eprintln!("[stt] failed to build input stream");
            wait_for_stop(&cmd_rx);
            continue;
        };
        if stream.play().is_err() {
            eprintln!("[stt] failed to start input stream");
            continue;
        }

        wait_for_stop(&cmd_rx);
        drop(stream);
        let samples = std::mem::take(&mut *buf.lock().unwrap());
        let _ = result_tx.send(CaptureResult {
            samples,
            sample_rate,
        });
    }
}

fn wait_for_stop(cmd_rx: &Receiver<CaptureCmd>) {
    while let Ok(cmd) = cmd_rx.recv() {
        if matches!(cmd, CaptureCmd::Stop) {
            return;
        }
    }
}

fn append_mono(buf: &Arc<Mutex<Vec<f32>>>, data: &[f32], channels: usize) {
    if channels == 0 {
        return;
    }
    let mut g = buf.lock().unwrap();
    for frame in data.chunks(channels) {
        g.push(frame.iter().copied().sum::<f32>() / channels as f32);
    }
}

// ─────────────────────────── inference thread (owns the Whisper model) ───────────────────────────

fn inference_loop(rx: Receiver<CaptureResult>, out: Sender<TranscriptEvent>, model_id: String) {
    let mut transcriber: Option<Transcriber> = None;
    while let Ok(cap) = rx.recv() {
        if transcriber.is_none() {
            let _ = out.send(TranscriptEvent::Status("loading speech model…".into()));
            match Transcriber::load(&model_id, &out) {
                Ok(t) => {
                    let _ = out.send(TranscriptEvent::Status("speech model ready".into()));
                    transcriber = Some(t);
                }
                Err(e) => {
                    let _ = out.send(TranscriptEvent::Error(e));
                    continue;
                }
            }
        }
        let _ = out.send(TranscriptEvent::Status("transcribing…".into()));
        let t = transcriber.as_mut().unwrap();
        let pcm16k = resample_to_16k(&cap.samples, cap.sample_rate);
        // Catch panics from the model/audio code so a single bad utterance can't kill the worker.
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| t.transcribe(&pcm16k)));
        match result {
            Ok(Ok(text)) => {
                let _ = out.send(TranscriptEvent::Text(text));
            }
            Ok(Err(e)) => {
                let _ = out.send(TranscriptEvent::Error(e));
            }
            Err(_) => {
                let _ = out.send(TranscriptEvent::Error(
                    "transcription failed (internal)".into(),
                ));
            }
        }
    }
}

struct Transcriber {
    model: whisper::model::Whisper,
    tokenizer: Tokenizer,
    config: whisper::Config,
    filters: Vec<f32>,
    device: Device,
    eot: u32,
    prefix: Vec<u32>,
}

impl Transcriber {
    fn load(model_id: &str, out: &Sender<TranscriptEvent>) -> Result<Self, String> {
        let dir = cache_dir(model_id)?;
        let config_path = ensure_file(model_id, "config.json", &dir, out)?;
        let tok_path = ensure_file(model_id, "tokenizer.json", &dir, out)?;
        let weights_path = ensure_file(model_id, "model.safetensors", &dir, out)?;

        let config: whisper::Config =
            serde_json::from_str(&std::fs::read_to_string(&config_path).map_err(stringify)?)
                .map_err(stringify)?;
        let tokenizer = Tokenizer::from_file(&tok_path).map_err(|e| e.to_string())?;
        let device = Device::Cpu;
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights_path], whisper::DTYPE, &device)
                .map_err(stringify)?
        };
        let model = whisper::model::Whisper::load(&vb, config.clone()).map_err(stringify)?;
        let filters = mel_filterbank(config.num_mel_bins);

        let tok = |s: &str| {
            tokenizer
                .token_to_id(s)
                .ok_or_else(|| format!("tokenizer missing token {s}"))
        };
        let eot = tok(whisper::EOT_TOKEN)?;
        // Forced decoder prefix. Multilingual models need a language + task token; .en models don't.
        let multilingual = config.vocab_size >= 51865;
        let mut prefix = vec![tok(whisper::SOT_TOKEN)?];
        if multilingual {
            prefix.push(tok("<|en|>")?);
            prefix.push(tok(whisper::TRANSCRIBE_TOKEN)?);
        }
        prefix.push(tok(whisper::NO_TIMESTAMPS_TOKEN)?);

        Ok(Self {
            model,
            tokenizer,
            config,
            filters,
            device,
            eot,
            prefix,
        })
    }

    fn transcribe(&mut self, samples_16k: &[f32]) -> Result<String, String> {
        self.model.reset_kv_cache();

        // Pad/trim to Whisper's 30 s window.
        let mut pcm = samples_16k.to_vec();
        pcm.truncate(whisper::N_SAMPLES);
        pcm.resize(whisper::N_SAMPLES, 0.0);

        let mel = whisper::audio::pcm_to_mel(&self.config, &pcm, &self.filters);
        let n_frames = mel.len() / self.config.num_mel_bins;
        let mel_t = Tensor::from_vec(mel, (1, self.config.num_mel_bins, n_frames), &self.device)
            .map_err(stringify)?;
        // candle's mel includes extra padding frames; Whisper's encoder consumes exactly one 30 s
        // chunk (N_FRAMES = 3000 → 1500 positions, matching the positional embedding).
        let mel_t = mel_t.narrow(2, 0, whisper::N_FRAMES).map_err(stringify)?;

        let features = self
            .model
            .encoder
            .forward(&mel_t, true)
            .map_err(stringify)?;

        let mut tokens = self.prefix.clone();
        for _ in 0..MAX_DECODE_TOKENS {
            let tt = Tensor::new(tokens.as_slice(), &self.device)
                .map_err(stringify)?
                .unsqueeze(0)
                .map_err(stringify)?;
            let ys = self
                .model
                .decoder
                .forward(&tt, &features, true)
                .map_err(stringify)?;
            let logits = self.model.decoder.final_linear(&ys).map_err(stringify)?;
            let last = logits.i((0, tokens.len() - 1)).map_err(stringify)?;
            let mut v: Vec<f32> = last.to_vec1().map_err(stringify)?;
            for &s in &self.config.suppress_tokens {
                if let Some(slot) = v.get_mut(s as usize) {
                    *slot = f32::NEG_INFINITY;
                }
            }
            let next = argmax(&v);
            if next == self.eot {
                break;
            }
            tokens.push(next);
        }

        let out_ids = &tokens[self.prefix.len()..];
        let text = self
            .tokenizer
            .decode(out_ids, true)
            .map_err(|e| e.to_string())?;
        Ok(text.trim().to_string())
    }
}

// ─────────────────────────── helpers ───────────────────────────

fn stringify<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn argmax(v: &[f32]) -> u32 {
    let mut best_i = 0usize;
    let mut best_v = f32::NEG_INFINITY;
    for (i, &x) in v.iter().enumerate() {
        if x > best_v {
            best_v = x;
            best_i = i;
        }
    }
    best_i as u32
}

/// `<config dir>/ferra-vrm/models/<sanitized model id>/`.
fn cache_dir(model_id: &str) -> Result<PathBuf, String> {
    let base = dirs::config_dir().ok_or("no config dir")?;
    let safe: String = model_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    Ok(base.join("ferra-vrm").join("models").join(safe))
}

/// Return the cached file, downloading it from HuggingFace on first use.
fn ensure_file(
    model_id: &str,
    file: &str,
    dir: &Path,
    out: &Sender<TranscriptEvent>,
) -> Result<PathBuf, String> {
    let path = dir.join(file);
    if path.exists() {
        return Ok(path);
    }
    let _ = out.send(TranscriptEvent::Status(format!("downloading {file}…")));
    let url = format!("https://huggingface.co/{model_id}/resolve/main/{file}");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(900))
        .build()
        .map_err(stringify)?;
    let resp = client.get(&url).send().map_err(stringify)?;
    if !resp.status().is_success() {
        return Err(format!("download {file}: HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().map_err(stringify)?;
    std::fs::create_dir_all(dir).map_err(stringify)?;
    std::fs::write(&path, &bytes).map_err(stringify)?;
    Ok(path)
}

/// Linear-resample mono audio to 16 kHz (good enough for Whisper).
fn resample_to_16k(input: &[f32], sample_rate: u32) -> Vec<f32> {
    if sample_rate == 16_000 || input.is_empty() {
        return input.to_vec();
    }
    let ratio = 16_000.0 / sample_rate as f32;
    let out_len = (input.len() as f32 * ratio) as usize;
    let last = input.len() - 1;
    (0..out_len)
        .map(|i| {
            let src = i as f32 / ratio;
            let i0 = (src.floor() as usize).min(last);
            let i1 = (i0 + 1).min(last);
            let frac = src - i0 as f32;
            input[i0] + (input[i1] - input[i0]) * frac
        })
        .collect()
}

/// Slaney (librosa-style) mel filterbank: `[n_mels][n_fft/2 + 1]` row-major, matching candle's
/// `pcm_to_mel` layout (it uses `n_fft/2 + 1 = 201` freq bins, the FFT including the Nyquist bin).
fn mel_filterbank(n_mels: usize) -> Vec<f32> {
    let sr = whisper::SAMPLE_RATE as f32;
    let n_fft = whisper::N_FFT; // 400
    let n_freqs = n_fft / 2 + 1; // 201, candle's filter row length
    let f_min = 0.0f32;
    let f_max = sr / 2.0;

    // Slaney mel scale (librosa htk=false).
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0f32;
    let min_log_mel = (min_log_hz - f_min) / f_sp;
    let logstep = (6.4f32).ln() / 27.0;
    let hz_to_mel = |f: f32| {
        if f >= min_log_hz {
            min_log_mel + (f / min_log_hz).ln() / logstep
        } else {
            (f - f_min) / f_sp
        }
    };
    let mel_to_hz = |mel: f32| {
        if mel >= min_log_mel {
            min_log_hz * ((mel - min_log_mel) * logstep).exp()
        } else {
            f_min + f_sp * mel
        }
    };

    let mel_min = hz_to_mel(f_min);
    let mel_max = hz_to_mel(f_max);
    // n_mels + 2 band edges in Hz.
    let edges: Vec<f32> = (0..n_mels + 2)
        .map(|i| mel_to_hz(mel_min + (mel_max - mel_min) * i as f32 / (n_mels + 1) as f32))
        .collect();
    let fft_freqs: Vec<f32> = (0..n_freqs).map(|k| k as f32 * sr / n_fft as f32).collect();

    let mut filters = vec![0.0f32; n_mels * n_freqs];
    for m in 0..n_mels {
        let (lower, center, upper) = (edges[m], edges[m + 1], edges[m + 2]);
        let enorm = 2.0 / (upper - lower);
        for (k, &f) in fft_freqs.iter().enumerate() {
            let down = (f - lower) / (center - lower);
            let up = (upper - f) / (upper - center);
            filters[m * n_freqs + k] = down.min(up).max(0.0) * enorm;
        }
    }
    filters
}
