# Contributing to Ferra-VRM

Thanks for your interest! Ferra-VRM is a native-Rust AI VTuber companion, and contributions are
welcome — especially new LLM/TTS providers, animations, avatar/expression support, and
platform fixes.

## Prerequisites

- A recent stable [Rust toolchain](https://rustup.rs/) (2024 edition).
- **Linux** also needs the audio/input dev headers Bevy + rodio build against:
  ```sh
  sudo apt-get install -y libasound2-dev libudev-dev
  ```

## Build & run

```sh
cargo run --release
```

If you keep a local test VRM at `assets/vrm/sample.vrm`, it loads on startup; otherwise the app
starts empty and prompts you to drop one in.

## Before you open a PR

Please make sure these pass — CI enforces them:

```sh
cargo fmt --all            # formatting
cargo clippy --workspace --all-targets   # lints (no warnings)
cargo check --workspace    # compiles clean
```

Then sanity-run the app and confirm it boots without panics.

## Guidelines

- **Keep `vrm_stage_core` generic.** Only put VRM/animation machinery there — no app- or
  provider-specific code. App features live in the binary crate.
- **Adding a provider?** Implement the trait (`LlmProvider` in `brain.rs`, `TtsProvider` in
  `tts.rs`) and expose it in the relevant settings tab. Keep network/audio work on a worker thread
  behind a channel — never block the render loop.
- **Never hardcode endpoints, keys, or secrets.** Everything is bring-your-own, entered at runtime
  and persisted to local config.
- **Assets must be license-cleared** for redistribution. Don't add VRM/audio/image assets you don't
  have the rights to ship.
- Match the surrounding code style; keep comments focused on the *why*.

## Architecture

See [ARCHITECTURE.md](ARCHITECTURE.md) for the plugin layout and data flow.

## License

By contributing, you agree that your contributions are dual-licensed under
[MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE), matching the project.
