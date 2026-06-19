# Architecture

Ferra-VRM is a small, readable Bevy app. The design goal is that each capability is an independent
**plugin**, all generic VRM machinery lives in a shared library, and anything that talks to the
network or audio runs off the render thread.

## Workspace

```
ferra-vrm/                  # the binary crate (this repo root)
  src/
    main.rs                 # app assembly + keyboard shortcuts
    camera.rs               # orbit camera (PanOrbitCamera) + HDR/bloom/ACES + reset
    staging.rs              # ground, 3-point light rig, ambient, backdrop
    avatar.rs               # avatar lifecycle, drag-drop load, idle reconcile
    brain.rs                # BYO-LLM: streaming chat client + connection probe
    character.rs            # character card → system prompt, greeting, drop-to-load
    tts.rs                  # BYO-TTS: worker thread, raw-PCM decode, envelope → lipsync
    stt.rs                  # speech-to-text: cpal capture + candle Whisper worker → LLM
    toast.rs                # transient error notifications
    config.rs               # settings persistence (config.toml)
    ui.rs                   # the egui control panel (tabs) + watermark + splash
  assets/                   # vrm/, vrma/, brand/
crates/
  vrm_stage_core/           # the shared, app-agnostic library
    src/{lib,idle,lipsync}.rs
```

`vrm_stage_core` is deliberately generic — VRM/VRMA loading, spring bones, expressions, idle
aliveness, and lipsync — with **no** app- or product-specific code, so it can back more than one
front end.

## Plugins

`main.rs` composes the app from one plugin per concern:

| Plugin | Responsibility |
|---|---|
| `VrmStageCorePlugin` | VRM + VRMA loading/playback (from `bevy_vrm1`) |
| `IdleAlivePlugin` | auto-blink |
| `LipSyncPlugin` | mouth driver (text heuristic **or** audio envelope) |
| `ConfigPlugin` | load/save `config.toml` |
| `ViewerCameraPlugin` | orbit camera + reset |
| `StagingPlugin` | lighting + ground |
| `AvatarPlugin` | spawn/swap avatar, drag-drop |
| `BrainPlugin` | LLM chat + model probe |
| `CharacterPlugin` | character card + greeting |
| `TtsPlugin` | TTS synth/playback worker + envelope bridge |
| `SttPlugin` | mic capture + Whisper transcription → LLM |
| `ToastPlugin` | error toasts |
| `UiPlugin` | egui panel, watermark, splash |

## Bring-your-own provider pattern

Both AI integrations are **provider traits** behind a runtime-configured endpoint — nothing is
hardcoded:

- **LLM** (`brain.rs`): `LlmProvider` with one `OpenAiCompatible` implementation that streams
  `POST /v1/chat/completions` (SSE `data:` deltas). One client covers llama.cpp, Ollama, LM Studio,
  vLLM, OpenAI, OpenRouter. Reasoning models that stream a separate `reasoning_content` channel are
  handled — only `content` is surfaced.
- **TTS** (`tts.rs`): `TtsProvider` with two presets that **normalize to raw PCM s16le / mono /
  24 kHz** so there's one decode + playback path — a `raw-PCM`-style `/v1/tts/stream` and an
  OpenAI `/audio/speech` (`response_format: "pcm"`). Bytes are decoded as headerless `i16` LE (with
  a `RIFF` guard), never through a WAV sniffer.

## Off-thread I/O

The render loop never blocks on the network or audio. Each integration spawns a worker thread and
talks to the Bevy world over a `crossbeam-channel`:

- **Chat**: submit → worker streams tokens → channel → a system drains them into the transcript.
- **TTS**: a finished reply → sentences → a worker that owns the `rodio` audio output synthesizes
  each in order, plays it, and sends back an **RMS amplitude envelope**. A system applies the
  envelope to the avatar's `LipSync`.
- **STT** (`stt.rs`): a `cpal` capture thread owns the (`!Send`) mic stream (push-to-talk via
  channel); a second worker owns the pure-Rust **candle Whisper** model (lazy-loaded, downloaded
  once via `reqwest`). Captured audio → resample to 16 kHz → log-mel → encoder/greedy-decoder →
  text → auto-submitted to the LLM. Mel filterbank is computed in-Rust (librosa-slaney).

## Lipsync

`vrm_stage_core::LipSync` has two modes:

- **Text** — a vowel-class heuristic over streamed tokens (used when TTS is off).
- **Audio** — an amplitude envelope drives a single `aa` jaw shape through an asymmetric envelope
  follower (fast attack, slow release), `mouth = min(0.85, rms * 13)`. The follower is dt-normalized
  so it behaves identically at any framerate.

## Persistence

`config.rs` serializes `LlmConfig` + `IdleSettings` + `CharacterCard` + `TtsConfig` to
`config.toml` in the OS config dir. To avoid disk thrash from egui's per-frame mutable access, it
writes only when a snapshot comparison detects a real change.

## Notable Bevy 0.18 specifics

Buffered events are **`Message`** (`MessageReader`/`MessageWriter`/`add_message`), not `Event`.
`Bloom` lives in `bevy::post_process`; `Hdr` is a marker component; scene-wide ambient is
`GlobalAmbientLight`. Fallible UI systems return `Result` and run in `EguiPrimaryContextPass`.

## Vendored dependency

`vendor/bevy_vrm1/` is a patched copy of `bevy_vrm1` 0.7.1 (wired in via `[patch.crates-io]`), with
two kinds of change (all marked `// PATCH (Ferra-VRM)`):

1. **Crash-hardening.** Upstream panics on some real-world VRM variants — a springbone extension
   missing `colliderGroups`, and expression-based lookAt (`todo!`) — which would abort the whole
   app on a dropped VRM. Both degrade to graceful no-ops.
2. **VRM 0.0 support** (`src/vrm/gltf/coordinate.rs` + a hook in `loader.rs`). Upstream is VRM 1.0
   only. `convert_vrm0_glb` runs on the raw glb bytes before Bevy builds the meshes: it migrates the
   legacy `VRM` extension → `VRMC_vrm` (humanoid bones, expressions) and negates the X axis across
   all geometry + transforms (VRM 0.0 is authored left-handed → loads X-mirrored). A 180° facing
   correction is applied as a runtime root transform in `initialize.rs` (kept out of the skeleton so
   VRMA retargeting cancels it). Bails gracefully — model still renders, just mirrored — on
   compressed/sparse/external-buffer glbs it can't safely rewrite.

Upstream license (MIT/Apache-2.0) is retained in the vendored directory; the patches are tracked
there for a future upstream PR (the VRM 0.0 work especially).
