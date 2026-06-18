# Setup — bring your own LLM & voice

Ferra-VRM never ships keys or default cloud endpoints. You point it at your own model and voice; the
settings are saved to `config.toml` in your OS config dir and reloaded next launch.

## LLM (Brain tab)

1. Open the **Brain** tab.
2. Pick a **Provider** preset to fill the Base URL, or type your own.
3. Local servers need no key. For cloud, paste your **API key** (toggle "show" to reveal it).
4. Click **Test connection** — it calls `GET {base}/v1/models`; on success you get a model count +
   latency and the **model picker** fills with the server's models.
5. Type in the chat box and send.

### Supported backends

Any OpenAI-compatible chat-completions endpoint works:

| Backend | Base URL (example) | API key |
|---|---|---|
| [llama.cpp server](https://github.com/ggml-org/llama.cpp) | `http://localhost:8080/v1` | – |
| [Ollama](https://ollama.com) | `http://localhost:11434/v1` | – |
| [LM Studio](https://lmstudio.ai) | `http://localhost:1234/v1` | – |
| [vLLM](https://github.com/vllm-project/vllm) | `http://localhost:8000/v1` | – |
| [OpenAI](https://platform.openai.com) | `https://api.openai.com/v1` | required |
| [OpenRouter](https://openrouter.ai) | `https://openrouter.ai/api/v1` | required |

> **Reasoning models**: if your model streams a separate "thinking" channel
> (`delta.reasoning_content`), Ferra-VRM shows and speaks only the final answer. With very small
> token limits a reply can be all reasoning and show nothing — give it room.

## Voice / TTS (Voice tab)

1. Open the **Voice** tab and check **Speak replies aloud**.
2. Choose a provider and fill the endpoint:

**OpenAI-compatible `/audio/speech`** — set the Base URL, a `model` (e.g. `tts-1`), a `voice`, and
a key if the server needs one. Ferra-VRM requests `response_format: "pcm"`.

**raw-PCM stream** — for servers that expose `POST {base}/v1/tts/stream` with body
`{"text": "...", "voice": "..."}` returning **raw PCM, signed 16-bit little-endian, mono, 24 kHz,
with no WAV header**. Set the Base URL and a `voice`.

A finished reply is split into sentences and synthesized in order; the avatar's mouth tracks the
voice amplitude.

> Both providers must return **24 kHz, 16-bit, mono PCM**. The decoder reads raw `i16` samples
> directly (a `RIFF`/WAV header, if present, is skipped) — do not expect MP3/Opus to play.

## Speech (STT / Ears)

In the **Voice** tab, enable **Listen for speech**. Then **hold F2 and talk**; release to send.
Your microphone is captured locally and transcribed by a **pure-Rust Whisper** (`candle`) — nothing
leaves your machine.

- **Model**: `distil-whisper/distil-small.en` by default (fast, English). Change it in the Voice
  tab to any HuggingFace Whisper repo that has `config.json`, `tokenizer.json`, and
  `model.safetensors` (e.g. `openai/whisper-base.en`, or a multilingual `openai/whisper-base`).
- **First use** downloads the model (a few hundred MB) to `<config dir>/ferra-vrm/models/…` and
  caches it; you'll see "downloading… / ready" toasts. Subsequent launches are instant.
- Transcribed text is **sent to the LLM automatically**, so with TTS on you get the full loop:
  speak → reply → spoken answer.
- CPU by default (fine for short utterances). GPU acceleration is available to those who build with
  candle's `metal`/`cuda` features.

## Where settings are stored

`config.toml` lives in your OS config dir, e.g.:

- Windows: `%APPDATA%\ferra-vrm\config.toml`
- macOS: `~/Library/Application Support/ferra-vrm/config.toml`
- Linux: `~/.config/ferra-vrm/config.toml`

Keys are stored in plaintext **locally only** — they never leave your machine.

## Troubleshooting

- **Test connection fails** — verify the Base URL ends in `/v1`, the server is running, and (for
  cloud) the key is set. Try the URL in a browser/curl.
- **No audio** — confirm the TTS server returns raw PCM (not MP3/WAV) at 24 kHz mono, and that an
  audio output device is available. Without a device, the mouth still lip-syncs silently.
- **Chat works but no voice** — make sure "Speak replies aloud" is checked in the Voice tab.
