//! The egui control panel — a single resizable side panel with a tab bar:
//! **Avatar · Brain · Character · Voice · Settings**. Avatar holds expressions / lip-sync test /
//! idle toggles / camera; Brain is the BYO-LLM chat; Character and Voice are scaffolded for the
//! character-card and TTS workstreams; Settings shows app/config info.
//!
//! All egui work happens in one `SidePanel::show` closure so the `Commands` borrow (expression
//! buttons) flows cleanly into the per-tab rendering.

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy_egui::{
    EguiContexts, EguiPlugin, EguiPrimaryContextPass, EguiTextureHandle, EguiUserTextures, egui,
};
use bevy_vrm1::prelude::*;

use crate::avatar::{CurrentAvatar, IdleSettings};
use crate::brain::{ChatState, LlmConfig, LlmProbe, ProbeStatus, Role, StartProbe, SubmitPrompt};
use crate::camera::ResetCamera;
use crate::character::{self, CharacterCard};
use crate::config;
use crate::stt::SttConfig;
use crate::toast::Toasts;
use crate::tts::{TtsConfig, TtsProviderKind};

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .init_resource::<UiTab>()
            .init_resource::<BrainUiState>()
            .insert_resource(Splash { open: true })
            .add_message::<TakeScreenshot>()
            .add_systems(Startup, load_brand)
            .add_systems(
                EguiPrimaryContextPass,
                (splash_window, control_panel).chain(),
            )
            .add_systems(Update, handle_screenshot);
    }
}

/// Which tab the panel is showing.
#[derive(Resource, Default, Clone, Copy, PartialEq)]
enum UiTab {
    Avatar,
    #[default]
    Brain,
    Character,
    Voice,
    Settings,
}

/// Emotion presets (use `SetExpressions` — one clean emotion at a time).
const EMOTIONS: [(&str, &str); 6] = [
    ("Happy", "happy"),
    ("Angry", "angry"),
    ("Sad", "sad"),
    ("Relaxed", "relaxed"),
    ("Surprised", "surprised"),
    ("Blink", "blink"),
];

/// Viseme shapes for the lip-sync tester.
const VISEMES: [&str; 5] = ["aa", "ih", "ou", "ee", "oh"];

/// Provider presets — conventional default endpoints only (the user pastes their own otherwise).
const PROVIDER_PRESETS: [(&str, &str); 6] = [
    ("llama.cpp (local)", "http://localhost:8080/v1"),
    ("Ollama (local)", "http://localhost:11434/v1"),
    ("LM Studio (local)", "http://localhost:1234/v1"),
    ("vLLM (local)", "http://localhost:8000/v1"),
    ("OpenAI", "https://api.openai.com/v1"),
    ("OpenRouter", "https://openrouter.ai/api/v1"),
];

/// Transient panel state that isn't persisted (e.g. whether the API key is unmasked).
#[derive(Resource, Default)]
struct BrainUiState {
    show_key: bool,
}

/// Whether the startup splash (brand + Twitch + Ko-fi) is still showing.
#[derive(Resource)]
struct Splash {
    open: bool,
}

/// egui texture id for the veraCoded brand banner (used in the splash + bottom-right watermark).
#[derive(Resource)]
struct BrandTextures {
    banner: egui::TextureId,
}

/// Load the brand banner as an egui texture at startup.
fn load_brand(
    asset_server: Res<AssetServer>,
    mut user_textures: ResMut<EguiUserTextures>,
    mut commands: Commands,
) {
    let handle = asset_server.load("brand/banner.png");
    let banner = user_textures.add_image(EguiTextureHandle::Strong(handle));
    commands.insert_resource(BrandTextures { banner });
}

/// Outgoing messages from the panel, bundled to stay under Bevy's 16-param system limit.
#[derive(SystemParam)]
struct PanelEvents<'w> {
    reset_cam: MessageWriter<'w, ResetCamera>,
    submit: MessageWriter<'w, SubmitPrompt>,
    start_probe: MessageWriter<'w, StartProbe>,
    shot: MessageWriter<'w, TakeScreenshot>,
}

#[allow(clippy::too_many_arguments)]
fn control_panel(
    mut contexts: EguiContexts,
    mut commands: Commands,
    diagnostics: Res<DiagnosticsStore>,
    vrms: Query<Entity, With<Vrm>>,
    mut cfg: ResMut<LlmConfig>,
    mut card: ResMut<CharacterCard>,
    mut tts: ResMut<TtsConfig>,
    mut chat: ResMut<ChatState>,
    mut settings: ResMut<IdleSettings>,
    mut tab: ResMut<UiTab>,
    mut brain_ui: ResMut<BrainUiState>,
    probe: Res<LlmProbe>,
    toasts: Res<Toasts>,
    current: Res<CurrentAvatar>,
    mut stt: ResMut<SttConfig>,
    mut events: PanelEvents,
) -> Result {
    let ctx = contexts.ctx_mut()?;

    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|d| d.smoothed())
        .unwrap_or(0.0);

    // Mirror idle settings into locals so we only mark the resource changed on a real edit
    // (otherwise the reconcile + save systems would fire every frame).
    let mut auto_blink = settings.auto_blink;
    let mut selected = *tab;

    egui::SidePanel::right("control_panel")
        .default_width(340.0)
        .resizable(true)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("🦀 VRM Viewer");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{fps:.0} FPS"));
                });
            });
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut selected, UiTab::Avatar, "Avatar");
                ui.selectable_value(&mut selected, UiTab::Brain, "Brain");
                ui.selectable_value(&mut selected, UiTab::Character, "Character");
                ui.selectable_value(&mut selected, UiTab::Voice, "Voice");
                ui.selectable_value(&mut selected, UiTab::Settings, "Settings");
            });
            ui.separator();

            match selected {
                UiTab::Avatar => {
                    ui.label("Drop a .vrm to load an avatar · .vrma to animate it.");
                    ui.separator();

                    ui.label("Expressions");
                    ui.horizontal_wrapped(|ui| {
                        for (label, key) in EMOTIONS {
                            if ui.button(label).clicked() {
                                for vrm in &vrms {
                                    commands.trigger(SetExpressions::single(vrm, key, 1.0));
                                }
                            }
                        }
                        if ui.button("Neutral").clicked() {
                            for vrm in &vrms {
                                commands.trigger(ClearExpressions { entity: vrm });
                            }
                        }
                    });

                    ui.add_space(6.0);
                    ui.label("Lip-sync test");
                    ui.horizontal_wrapped(|ui| {
                        for v in VISEMES {
                            if ui.button(v).clicked() {
                                for vrm in &vrms {
                                    commands.trigger(ModifyExpressions::mouth(vrm, v, 0.9));
                                }
                            }
                        }
                    });

                    ui.add_space(6.0);
                    ui.label("Idle / aliveness");
                    ui.checkbox(&mut auto_blink, "Auto-blink");

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Reset camera (R)").clicked() {
                            events.reset_cam.write(ResetCamera);
                        }
                        if ui.button("📸 Screenshot (F12)").clicked() {
                            events.shot.write(TakeScreenshot);
                        }
                    });
                }

                UiTab::Brain => {
                    ui.horizontal(|ui| {
                        ui.label("Provider:");
                        egui::ComboBox::from_id_salt("provider_presets")
                            .selected_text("presets…")
                            .show_ui(ui, |ui| {
                                for (name, url) in PROVIDER_PRESETS {
                                    if ui.selectable_label(false, name).clicked() {
                                        cfg.base_url = url.to_string();
                                    }
                                }
                            });
                    });
                    egui::Grid::new("llm_cfg").num_columns(2).show(ui, |ui| {
                        ui.label("Base URL");
                        ui.text_edit_singleline(&mut cfg.base_url);
                        ui.end_row();
                        ui.label("API key");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut cfg.api_key)
                                    .password(!brain_ui.show_key)
                                    .desired_width(170.0),
                            );
                            ui.checkbox(&mut brain_ui.show_key, "show");
                        });
                        ui.end_row();
                        ui.label("Model");
                        ui.text_edit_singleline(&mut cfg.model);
                        ui.end_row();
                    });

                    ui.horizontal(|ui| {
                        if ui.button("Test connection").clicked() {
                            events.start_probe.write(StartProbe);
                        }
                        match &probe.status {
                            ProbeStatus::Idle => {}
                            ProbeStatus::Testing => {
                                ui.spinner();
                            }
                            ProbeStatus::Ok { count, latency_ms } => {
                                ui.colored_label(
                                    egui::Color32::LIGHT_GREEN,
                                    format!("✓ {count} models · {latency_ms} ms"),
                                );
                            }
                            ProbeStatus::Failed(e) => {
                                ui.colored_label(egui::Color32::LIGHT_RED, format!("✗ {e}"));
                            }
                        }
                    });
                    if !probe.models.is_empty() {
                        let current = if cfg.model.is_empty() {
                            "pick a model".to_string()
                        } else {
                            cfg.model.clone()
                        };
                        egui::ComboBox::from_id_salt("model_picker")
                            .selected_text(current)
                            .show_ui(ui, |ui| {
                                for m in &probe.models {
                                    if ui.selectable_label(cfg.model == *m, m).clicked() {
                                        cfg.model = m.clone();
                                    }
                                }
                            });
                    }
                    ui.small("Local (llama.cpp / Ollama / LM Studio) needs no key. Cloud: paste base URL + key.");

                    ui.separator();
                    egui::ScrollArea::vertical()
                        .max_height(260.0)
                        .auto_shrink([false, false])
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for turn in &chat.messages {
                                let (who, color) = match turn.role {
                                    Role::User => ("You", egui::Color32::LIGHT_BLUE),
                                    Role::Assistant => ("AI ", egui::Color32::LIGHT_GREEN),
                                };
                                ui.colored_label(color, format!("{who}: {}", turn.text));
                            }
                        });

                    ui.separator();
                    let mut send = false;
                    ui.horizontal(|ui| {
                        let resp = ui.text_edit_singleline(&mut chat.input);
                        let entered =
                            resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let clicked =
                            ui.add_enabled(!chat.streaming, egui::Button::new("Send")).clicked();
                        send = (entered || clicked) && !chat.input.trim().is_empty();
                    });
                    if chat.streaming {
                        ui.spinner();
                    }
                    if send {
                        let text = chat.input.clone();
                        events.submit.write(SubmitPrompt(text));
                        chat.input.clear();
                    }
                }

                UiTab::Character => {
                    ui.label("🎭 Character card");
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        ui.text_edit_singleline(&mut card.name);
                    });
                    ui.label("Persona");
                    ui.add(
                        egui::TextEdit::multiline(&mut card.persona)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY),
                    );
                    ui.label("Speaking style");
                    ui.add(
                        egui::TextEdit::multiline(&mut card.speaking_style)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY),
                    );
                    ui.label("Scenario");
                    ui.add(
                        egui::TextEdit::singleline(&mut card.scenario).desired_width(f32::INFINITY),
                    );
                    ui.label("Greeting");
                    ui.add(
                        egui::TextEdit::multiline(&mut card.greeting)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY),
                    );
                    ui.label("Example dialogue");
                    ui.add(
                        egui::TextEdit::multiline(&mut card.examples)
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(4.0);
                    if ui.button("Export card…").clicked()
                        && let Some(p) = character::export_card(&card)
                    {
                        info!("[ui] exported character card to {}", p.display());
                    }
                    ui.small("Drop a .toml card onto the window to load one. The active card auto-saves.");
                }

                UiTab::Voice => {
                    ui.label("🔊 Voice (TTS)");
                    ui.checkbox(&mut tts.enabled, "Speak replies aloud");
                    ui.horizontal(|ui| {
                        ui.label("Provider:");
                        ui.radio_value(
                            &mut tts.provider,
                            TtsProviderKind::OpenAiSpeech,
                            "OpenAI /audio/speech",
                        );
                        ui.radio_value(
                            &mut tts.provider,
                            TtsProviderKind::RawPcmStream,
                            "raw-PCM stream",
                        );
                    });
                    egui::Grid::new("tts_cfg").num_columns(2).show(ui, |ui| {
                        ui.label("Base URL");
                        ui.text_edit_singleline(&mut tts.base_url);
                        ui.end_row();
                        ui.label("Voice");
                        ui.text_edit_singleline(&mut tts.voice);
                        ui.end_row();
                        if tts.provider == TtsProviderKind::OpenAiSpeech {
                            ui.label("Model");
                            ui.text_edit_singleline(&mut tts.model);
                            ui.end_row();
                            ui.label("API key");
                            ui.add(
                                egui::TextEdit::singleline(&mut tts.api_key)
                                    .password(!brain_ui.show_key),
                            );
                            ui.end_row();
                        }
                    });
                    ui.small(
                        "Audio plays a finished reply sentence-by-sentence and drives the mouth from \
                         the voice amplitude. Both providers expect raw PCM (s16le, 24 kHz, mono).",
                    );

                    ui.separator();
                    ui.label("🎤 Ears (speech-to-text)");
                    ui.checkbox(&mut stt.enabled, "Listen for speech");
                    ui.horizontal(|ui| {
                        ui.label("Model");
                        ui.text_edit_singleline(&mut stt.model);
                    });
                    ui.small(
                        "Hold F2 to talk, release to send. Pure-Rust Whisper; the model downloads \
                         once on first use.",
                    );
                }

                UiTab::Settings => {
                    ui.label("Settings are saved automatically to:");
                    let path = config::config_path()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "<no config dir available>".into());
                    ui.add(egui::Label::new(egui::RichText::new(path).monospace()).wrap());
                    ui.separator();
                    ui.label("Ferra-VRM");
                    ui.small("Bevy + bevy_vrm1 · bring your own LLM & voice · host your own AI companion.");
                }
            }
        });

    // Empty-state hint over the 3D view when no avatar is loaded.
    if current.0.is_none() {
        egui::Area::new(egui::Id::new("drop_hint"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("Drop a .vrm file to load an avatar")
                        .size(20.0)
                        .color(egui::Color32::from_white_alpha(180)),
                );
            });
    }

    // Transient toasts, top-center.
    if !toasts.0.is_empty() {
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::CENTER_TOP, [0.0, 12.0])
            .show(ctx, |ui| {
                for item in &toasts.0 {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(&item.text);
                    });
                }
            });
    }

    // veraCoded brand watermark — bottom-right text link to the creator's Twitch.
    egui::Area::new(egui::Id::new("watermark"))
        .anchor(egui::Align2::RIGHT_BOTTOM, [-10.0, -8.0])
        .interactable(true)
        .show(ctx, |ui| {
            ui.hyperlink_to(
                egui::RichText::new("veraCoded")
                    .size(13.0)
                    .color(egui::Color32::from_white_alpha(170)),
                "https://www.twitch.tv/veracoded",
            )
            .on_hover_text("twitch.tv/veracoded");
        });

    // Commit edits exactly once, only on a real change.
    if auto_blink != settings.auto_blink {
        settings.auto_blink = auto_blink;
    }
    if selected != *tab {
        *tab = selected;
    }

    Ok(())
}

/// Startup splash: brand + Twitch + Ko-fi, dismissed with a button. Shown once per launch.
fn splash_window(
    mut contexts: EguiContexts,
    mut splash: ResMut<Splash>,
    brand: Option<Res<BrandTextures>>,
) -> Result {
    if !splash.open {
        return Ok(());
    }
    let banner = brand.map(|b| b.banner);
    let ctx = contexts.ctx_mut()?;
    egui::Window::new("welcome")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                if let Some(banner) = banner {
                    ui.add_space(4.0);
                    ui.image(egui::load::SizedTexture::new(
                        banner,
                        egui::vec2(360.0, 240.0),
                    ));
                }
                ui.add_space(6.0);
                ui.heading("Ferra-VRM");
                ui.label("a native-Rust AI VTuber companion");
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("by veraCoded")
                        .color(egui::Color32::from_rgb(190, 140, 255)),
                );
                ui.add_space(10.0);
                ui.hyperlink_to("📺  twitch.tv/veracoded", "https://www.twitch.tv/veracoded");
                ui.add_space(4.0);
                ui.hyperlink_to("💜  Support on Ko-fi", "https://ko-fi.com/veracoded");
                ui.add_space(12.0);
                if ui.button("Let's go").clicked() {
                    splash.open = false;
                }
                ui.add_space(6.0);
            });
        });
    Ok(())
}

/// Request a screenshot of the primary window (UI button or F12).
#[derive(Message, Default)]
pub struct TakeScreenshot;

fn handle_screenshot(
    mut events: MessageReader<TakeScreenshot>,
    mut commands: Commands,
    mut n: Local<u32>,
) {
    if events.is_empty() {
        return;
    }
    events.clear();

    let Some(dir) = config::screenshots_dir() else {
        warn!("[ui] no config dir for screenshots");
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("shot-{:04}.png", *n));
    *n += 1;
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(path.clone()));
    info!("[ui] screenshot → {}", path.display());
}
