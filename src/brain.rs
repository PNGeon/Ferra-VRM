//! BYO-LLM brain — the plug-and-play headline. The user pastes their own API key OR points at a
//! local model (Ollama / LM Studio / vLLM / any OpenAI-compatible base URL), picks a model, and
//! chats. There is NO default cloud and NO bundled key: the user always brings their own.
//!
//! The provider layer is a thin trait over the OpenAI-compatible `/chat/completions` SSE stream,
//! so one client covers OpenAI, OpenRouter, Ollama, LM Studio, vLLM, etc. The network call runs on
//! a worker thread (blocking reqwest) and streams tokens back into the Bevy world over a channel;
//! tokens are appended to the transcript and fed to the avatar's lipsync as they arrive.

use crate::character::CharacterCard;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, unbounded};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::time::{Duration, Instant};
use vrm_stage_core::LipSync;

pub struct BrainPlugin;

impl Plugin for BrainPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LlmConfig>()
            .init_resource::<ChatState>()
            .init_resource::<ChatLink>()
            .init_resource::<LlmProbe>()
            .add_message::<SubmitPrompt>()
            .add_message::<StartProbe>()
            .add_systems(
                Update,
                (submit_prompt, pump_responses, start_probe, pump_probe),
            );
    }
}

// ─────────────────────────── config & transcript state ───────────────────────────

/// Connection settings the user edits in the panel. Defaults point at a LOCAL model (Ollama) with
/// no key — the "own your AI" path works out of the box; cloud is opt-in by pasting a key + URL.
#[derive(Resource, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:11434/v1".into(),
            api_key: String::new(),
            model: "llama3.2".into(),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Clone)]
pub struct Turn {
    pub role: Role,
    pub text: String,
}

/// The chat transcript + the in-progress input box, shown and edited by the UI.
#[derive(Resource, Default)]
pub struct ChatState {
    pub messages: Vec<Turn>,
    pub input: String,
    pub streaming: bool,
}

/// UI → brain: send the current input as a prompt.
#[derive(Message)]
pub struct SubmitPrompt(pub String);

/// Holds the receiver for an in-flight streaming response (None when idle).
#[derive(Resource, Default)]
struct ChatLink {
    rx: Option<Receiver<ChatEvent>>,
}

/// What the worker thread streams back to the main world.
pub(crate) enum ChatEvent {
    Token(String),
    Done,
    Error(String),
}

// ─────────────────────────── provider abstraction ───────────────────────────

/// One message in the OpenAI-compatible wire format.
#[derive(Serialize, Clone)]
pub struct ChatMessage {
    pub role: &'static str,
    pub content: String,
}

/// The extension point. Any backend that can stream a chat completion implements this; today we
/// ship one OpenAI-compatible client that covers the whole ecosystem. `stream_chat` runs blocking
/// (it's called on a worker thread) and emits events as tokens arrive.
pub(crate) trait LlmProvider: Send + 'static {
    fn stream_chat(&self, messages: Vec<ChatMessage>, tx: Sender<ChatEvent>);
}

/// OpenAI-compatible `/chat/completions` streaming client (OpenAI, OpenRouter, Ollama, LM Studio,
/// vLLM, …). Bring your own `base_url` + optional `api_key`.
struct OpenAiCompatible {
    base_url: String,
    api_key: String,
    model: String,
}

impl LlmProvider for OpenAiCompatible {
    fn stream_chat(&self, messages: Vec<ChatMessage>, tx: Sender<ChatEvent>) {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "model": self.model,
            "stream": true,
            "messages": messages,
        });

        let client = reqwest::blocking::Client::new();
        let mut req = client.post(&url).json(&body);
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }

        let resp = match req.send() {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(ChatEvent::Error(format!("request failed: {e}")));
                return;
            }
        };
        if !resp.status().is_success() {
            let _ = tx.send(ChatEvent::Error(format!(
                "server returned {}",
                resp.status()
            )));
            return;
        }

        // OpenAI-style SSE: lines of `data: {json}`, terminated by `data: [DONE]`.
        let reader = BufReader::new(resp);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let Some(data) = line.trim().strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" {
                break;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(data)
                && let Some(tok) = v["choices"][0]["delta"]["content"].as_str()
                && !tok.is_empty()
                && tx.send(ChatEvent::Token(tok.to_string())).is_err()
            {
                return; // main world dropped the receiver
            }
        }
        let _ = tx.send(ChatEvent::Done);
    }
}

// ─────────────────────────── systems ───────────────────────────

fn submit_prompt(
    mut events: MessageReader<SubmitPrompt>,
    cfg: Res<LlmConfig>,
    card: Res<CharacterCard>,
    mut chat: ResMut<ChatState>,
    mut link: ResMut<ChatLink>,
) {
    for SubmitPrompt(text) in events.read() {
        let text = text.trim();
        if text.is_empty() || chat.streaming {
            continue;
        }

        // Assemble the wire history (system from the character card + prior turns + this user turn)
        // before we add the empty assistant turn that streamed tokens will fill in.
        let mut wire = vec![ChatMessage {
            role: "system",
            content: card.to_system_prompt(),
        }];
        for turn in &chat.messages {
            wire.push(ChatMessage {
                role: match turn.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                },
                content: turn.text.clone(),
            });
        }
        wire.push(ChatMessage {
            role: "user",
            content: text.to_string(),
        });

        chat.messages.push(Turn {
            role: Role::User,
            text: text.to_string(),
        });
        chat.messages.push(Turn {
            role: Role::Assistant,
            text: String::new(),
        });
        chat.streaming = true;

        let (tx, rx) = unbounded();
        link.rx = Some(rx);
        let provider: Box<dyn LlmProvider> = Box::new(OpenAiCompatible {
            base_url: cfg.base_url.clone(),
            api_key: cfg.api_key.clone(),
            model: cfg.model.clone(),
        });
        std::thread::spawn(move || provider.stream_chat(wire, tx));
    }
}

fn pump_responses(
    mut link: ResMut<ChatLink>,
    mut chat: ResMut<ChatState>,
    mut lips: Query<&mut LipSync>,
    tts_cfg: Option<Res<crate::tts::TtsConfig>>,
    mut toasts: MessageWriter<crate::toast::Toast>,
) {
    // When TTS is on, the audio-amplitude envelope drives the mouth; skip the text heuristic so the
    // two don't fight. When TTS is off, text tokens drive lipsync as before.
    let tts_on = tts_cfg.is_some_and(|c| c.enabled);
    let mut finished = false;
    if let Some(rx) = &link.rx {
        for event in rx.try_iter() {
            match event {
                ChatEvent::Token(tok) => {
                    if let Some(last) = chat.messages.last_mut() {
                        last.text.push_str(&tok);
                    }
                    if !tts_on {
                        for mut lip in &mut lips {
                            lip.feed(&tok);
                        }
                    }
                }
                ChatEvent::Done => finished = true,
                ChatEvent::Error(e) => {
                    if let Some(last) = chat.messages.last_mut() {
                        last.text.push_str(&format!("\n⚠ {e}"));
                    }
                    toasts.write(crate::toast::Toast(format!("LLM error: {e}")));
                    finished = true;
                }
            }
        }
    }
    if finished {
        link.rx = None;
        chat.streaming = false;
        for mut lip in &mut lips {
            lip.silence();
        }
    }
}

// ─────────────────────────── connection probe (Test + model list) ───────────────────────────

/// UI → brain: test the current LLM endpoint and fetch its model list.
#[derive(Message)]
pub struct StartProbe;

/// Result of a connection probe, surfaced in the Brain tab.
#[derive(Default, Clone)]
pub enum ProbeStatus {
    #[default]
    Idle,
    Testing,
    Ok {
        count: usize,
        latency_ms: u64,
    },
    Failed(String),
}

/// Live probe state: the in-flight receiver, the latest status, and the discovered model ids
/// (used to populate the model picker).
#[derive(Resource, Default)]
pub struct LlmProbe {
    rx: Option<Receiver<Result<ProbeOk, String>>>,
    pub status: ProbeStatus,
    pub models: Vec<String>,
}

struct ProbeOk {
    latency_ms: u64,
    models: Vec<String>,
}

fn start_probe(
    mut events: MessageReader<StartProbe>,
    cfg: Res<LlmConfig>,
    mut probe: ResMut<LlmProbe>,
) {
    if events.is_empty() {
        return;
    }
    events.clear();

    probe.status = ProbeStatus::Testing;
    let (tx, rx) = unbounded();
    probe.rx = Some(rx);
    let base = cfg.base_url.clone();
    let key = cfg.api_key.clone();
    std::thread::spawn(move || {
        let _ = tx.send(run_probe(&base, &key));
    });
}

/// Blocking `GET {base}/models` (OpenAI-compatible). Returns the model ids + round-trip latency.
fn run_probe(base_url: &str, api_key: &str) -> Result<ProbeOk, String> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| format!("client init failed: {e}"))?;

    let mut req = client.get(&url);
    if !api_key.is_empty() {
        req = req.bearer_auth(api_key);
    }

    let start = Instant::now();
    let resp = req.send().map_err(|e| format!("unreachable: {e}"))?;
    let latency_ms = start.elapsed().as_millis() as u64;

    if !resp.status().is_success() {
        return Err(format!("server returned {}", resp.status()));
    }
    // OpenAI shape: { "data": [ { "id": "..." }, ... ] }.
    let models = resp
        .json::<serde_json::Value>()
        .ok()
        .and_then(|v| {
            v["data"].as_array().map(|arr| {
                arr.iter()
                    .filter_map(|m| m["id"].as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
        })
        .unwrap_or_default();

    Ok(ProbeOk { latency_ms, models })
}

fn pump_probe(mut probe: ResMut<LlmProbe>) {
    let msg = probe.rx.as_ref().and_then(|rx| rx.try_recv().ok());
    let Some(msg) = msg else {
        return;
    };
    probe.rx = None;
    match msg {
        Ok(ok) => {
            probe.status = ProbeStatus::Ok {
                count: ok.models.len(),
                latency_ms: ok.latency_ms,
            };
            probe.models = ok.models;
        }
        Err(e) => probe.status = ProbeStatus::Failed(e),
    }
}
