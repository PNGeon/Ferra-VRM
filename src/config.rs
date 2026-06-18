//! Settings persistence. Loads `config.toml` from the per-OS config dir on startup and writes it
//! back whenever the user actually changes something — so API keys, model, persona, and idle
//! preferences survive a restart. This is the foundation the Brain / Character / Voice tabs build
//! on (no re-typing your provider every launch).
//!
//! Write strategy: the egui panel holds `ResMut<LlmConfig>` and so marks it "changed" every frame
//! (egui needs `&mut` to edit text), which would thrash the disk. Instead we keep a snapshot of the
//! last-saved config and only write when the current values genuinely differ.

use crate::avatar::IdleSettings;
use crate::brain::LlmConfig;
use crate::character::CharacterCard;
use crate::stt::SttConfig;
use crate::tts::TtsConfig;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub struct ConfigPlugin;

impl Plugin for ConfigPlugin {
    fn build(&self, app: &mut App) {
        // PreStartup so the loaded values are in place before AvatarPlugin's Startup setup reads
        // IdleSettings and spawns the avatar.
        app.add_systems(PreStartup, load_config)
            .add_systems(Update, save_on_change);
    }
}

/// The on-disk shape. Adding workstreams (Character card, TTS) extends this struct; `#[serde(default)]`
/// keeps old config files forward-compatible (missing sections fall back to defaults).
#[derive(Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
struct ViewerConfig {
    llm: LlmConfig,
    idle: IdleSettings,
    character: CharacterCard,
    tts: TtsConfig,
    stt: SttConfig,
}

/// Snapshot of what's currently on disk, so we only rewrite on a real change.
#[derive(Resource)]
struct LastSaved(ViewerConfig);

/// `…/<config dir>/ferra-vrm/config.toml` (e.g. `%APPDATA%\ferra-vrm\config.toml` on Windows).
pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("ferra-vrm").join("config.toml"))
}

/// `…/<config dir>/ferra-vrm/cards/` — where exported character cards live.
pub fn cards_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("ferra-vrm").join("cards"))
}

/// `…/<config dir>/ferra-vrm/screenshots/` — where captured screenshots are saved.
pub fn screenshots_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("ferra-vrm").join("screenshots"))
}

fn load_config(mut commands: Commands) {
    let cfg = config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| match toml::from_str::<ViewerConfig>(&s) {
            Ok(c) => Some(c),
            Err(e) => {
                warn!("[config] ignoring unreadable config.toml: {e}");
                None
            }
        })
        .unwrap_or_default();

    commands.insert_resource(cfg.llm.clone());
    commands.insert_resource(cfg.idle.clone());
    commands.insert_resource(cfg.character.clone());
    commands.insert_resource(cfg.tts.clone());
    commands.insert_resource(cfg.stt.clone());
    commands.insert_resource(LastSaved(cfg));
}

fn save_on_change(
    llm: Res<LlmConfig>,
    idle: Res<IdleSettings>,
    character: Res<CharacterCard>,
    tts: Res<TtsConfig>,
    stt: Res<SttConfig>,
    mut last: ResMut<LastSaved>,
) {
    let current = ViewerConfig {
        llm: llm.clone(),
        idle: idle.clone(),
        character: character.clone(),
        tts: tts.clone(),
        stt: stt.clone(),
    };
    if current == last.0 {
        return;
    }

    let Some(path) = config_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match toml::to_string_pretty(&current) {
        Ok(text) => {
            if let Err(e) = std::fs::write(&path, text) {
                warn!("[config] failed to write {}: {e}", path.display());
            } else {
                last.0 = current;
            }
        }
        Err(e) => warn!("[config] failed to serialize config: {e}"),
    }
}
