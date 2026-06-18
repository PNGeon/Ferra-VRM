//! Character card — the personality layer. A small structured card (name, persona, speaking
//! style, scenario, greeting, example dialogue) is composed into the system prompt that drives the
//! LLM, and the greeting seeds the first assistant turn so the companion says hello on launch.
//!
//! Cards are plain TOML: the active card is persisted in the main config, and you can **drop a
//! `.toml` card onto the window** to load one (or export the current card to share it). Kept
//! deliberately simple/ours; the struct is a friendly superset so SillyTavern import could be
//! layered on later without reworking it.

use crate::brain::{ChatState, Role, Turn};
use crate::config;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

pub struct CharacterPlugin;

impl Plugin for CharacterPlugin {
    fn build(&self, app: &mut App) {
        // A default exists immediately; ConfigPlugin (PreStartup) overwrites it from disk if present.
        app.init_resource::<CharacterCard>()
            .add_systems(Startup, seed_greeting)
            .add_systems(Update, handle_card_drops);
    }
}

/// The persisted, user-editable character definition.
#[derive(Resource, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CharacterCard {
    pub name: String,
    pub persona: String,
    pub speaking_style: String,
    pub scenario: String,
    pub greeting: String,
    pub examples: String,
}

impl Default for CharacterCard {
    fn default() -> Self {
        Self {
            name: "Aria".into(),
            persona: "A cheerful, curious AI companion who loves chatting and helping out.".into(),
            speaking_style: "Warm, concise, and playful. Keep replies short.".into(),
            scenario: "Hanging out with you at your desk.".into(),
            greeting: "Hey! I'm Aria — what are we working on today?".into(),
            examples: String::new(),
        }
    }
}

impl CharacterCard {
    /// Compose the card into a system prompt (empty fields are skipped).
    pub fn to_system_prompt(&self) -> String {
        let mut p = String::new();
        let push = |p: &mut String, label: &str, val: &str| {
            let v = val.trim();
            if !v.is_empty() {
                if label.is_empty() {
                    p.push_str(v);
                } else {
                    p.push_str(label);
                    p.push_str(v);
                }
                p.push('\n');
            }
        };
        if !self.name.trim().is_empty() {
            push(&mut p, "You are ", &self.name);
        }
        push(&mut p, "", &self.persona);
        push(&mut p, "Speaking style: ", &self.speaking_style);
        push(&mut p, "Scenario: ", &self.scenario);
        if !self.examples.trim().is_empty() {
            p.push_str("\nExample dialogue:\n");
            p.push_str(self.examples.trim());
            p.push('\n');
        }
        if p.trim().is_empty() {
            p = "You are a friendly AI companion.".into();
        }
        p
    }
}

/// On launch, greet the user with the card's greeting (if the transcript is empty).
fn seed_greeting(card: Res<CharacterCard>, mut chat: ResMut<ChatState>) {
    if chat.messages.is_empty() && !card.greeting.trim().is_empty() {
        chat.messages.push(Turn {
            role: Role::Assistant,
            text: card.greeting.trim().to_string(),
        });
    }
}

/// Drop a `.toml` character card onto the window to load it (clears the chat + re-greets).
fn handle_card_drops(
    mut drops: MessageReader<FileDragAndDrop>,
    mut chat: ResMut<ChatState>,
    mut card: ResMut<CharacterCard>,
) {
    for event in drops.read() {
        let FileDragAndDrop::DroppedFile { path_buf, .. } = event else {
            continue;
        };
        let is_toml = path_buf
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("toml"));
        if !is_toml {
            continue;
        }

        match std::fs::read_to_string(path_buf)
            .ok()
            .and_then(|s| toml::from_str::<CharacterCard>(&s).ok())
        {
            Some(new_card) => {
                let greeting = new_card.greeting.trim().to_string();
                *card = new_card;
                chat.messages.clear();
                if !greeting.is_empty() {
                    chat.messages.push(Turn {
                        role: Role::Assistant,
                        text: greeting,
                    });
                }
                info!("[character] loaded card: {}", card.name);
            }
            None => warn!(
                "[character] could not parse character card: {}",
                path_buf.display()
            ),
        }
    }
}

/// Export the current card to `<config>/cards/<name>.toml`. Returns the written path on success.
pub fn export_card(card: &CharacterCard) -> Option<std::path::PathBuf> {
    let dir = config::cards_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let safe: String = card
        .name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let safe = if safe.trim_matches('_').is_empty() {
        "character".to_string()
    } else {
        safe
    };
    let path = dir.join(format!("{safe}.toml"));
    let text = toml::to_string_pretty(card).ok()?;
    std::fs::write(&path, text).ok()?;
    Some(path)
}
