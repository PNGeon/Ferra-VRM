# Changelog

All notable changes to Ferra-VRM are documented here. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/); versions follow [SemVer](https://semver.org/).

## [0.1.0] — 2026-06-18

First public release. A native-Rust AI VTuber companion: bring your own LLM and voice.

### Added

- **VRM 1.0 avatars** with automatic spring-bone physics and **VRMA** animation playback
  (via `bevy_vrm1`).
- **Drag-and-drop**: `.vrm` swaps the avatar, `.vrma` animates it, `.toml` loads a character card.
  Files load from anywhere on disk.
- **Idle aliveness**: natural auto-blink.
- **Bring-your-own LLM** (Brain tab): any OpenAI-compatible endpoint, with provider presets,
  a connection test (`GET /v1/models`) showing model count + latency, a model picker, and a
  show/hide API-key toggle. Streaming replies; reasoning-model `reasoning_content` is filtered out.
- **Character cards** (Character tab): structured TOML (name / persona / speaking style / scenario /
  greeting / examples) composed into the system prompt; the greeting seeds the first turn. Export to
  share or drop a card onto the window to load.
- **Bring-your-own TTS** (Voice tab): an OpenAI-compatible `/audio/speech` provider and a raw-PCM
  streaming provider, both normalizing to PCM s16le/24k/mono. Replies are spoken sentence-by-sentence
  and the mouth lip-syncs to the voice amplitude (an asymmetric envelope follower).
- **Speech-to-text — Ears** (Voice tab): push-to-talk (hold **F2**) → microphone capture (`cpal`) →
  **pure-Rust Whisper** (`candle`) → text auto-submitted to the LLM, closing the voice loop. The
  model downloads once on first use and is cached locally; CPU by default.
- **Orbit camera** with HDR + bloom + ACES tonemapping; **R** resets, **F12** screenshots.
- **Product staging**: ground plane, 3-point light rig, ambient fill, backdrop.
- **Settings persistence** to the OS config dir (`config.toml`).
- Transient error **toasts**, empty-state drop hint, and a startup splash with Twitch + Ko-fi links.

### Notes

- The public build ships **no bundled avatar** — drop in any VRM 1.0 model to begin.
- Window/taskbar icon and an in-splash logo image are planned for a follow-up.

[0.1.0]: https://github.com/PNGeon/Ferra-VRM/releases/tag/v0.1.0
