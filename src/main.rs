//! Ferra-VRM — a native-Rust AI VTuber companion. A VRM avatar driven by your own LLM and voice,
//! with idle aliveness and lipsync. Bring your own provider; nothing is hardcoded.
//!
//! Ships no bundled avatar — drop a VRM 1.0 model onto the window to begin.

mod avatar;
mod brain;
mod camera;
mod character;
mod config;
mod staging;
mod stt;
mod toast;
mod tts;
mod ui;

use bevy::asset::{AssetPlugin, UnapprovedPathMode};
use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::prelude::*;
use bevy_vrm1::prelude::*;
use vrm_stage_core::{IdleAlivePlugin, LipSyncPlugin, VrmStageCorePlugin};

use avatar::AvatarPlugin;
use brain::BrainPlugin;
use camera::{ResetCamera, ViewerCameraPlugin};
use character::CharacterPlugin;
use config::ConfigPlugin;
use staging::StagingPlugin;
use stt::SttPlugin;
use toast::ToastPlugin;
use tts::TtsPlugin;
use ui::{TakeScreenshot, UiPlugin};

fn main() {
    App::new()
        .add_plugins((
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Ferra-VRM".into(),
                        ..default()
                    }),
                    ..default()
                })
                // Allow drag-dropping .vrm/.vrma from anywhere on disk (outside the asset root).
                .set(AssetPlugin {
                    unapproved_path_mode: UnapprovedPathMode::Allow,
                    ..default()
                }),
            FrameTimeDiagnosticsPlugin::default(),
            // Shared foundation + generic aliveness layers (all public-safe, asset-agnostic).
            VrmStageCorePlugin,
            IdleAlivePlugin,
            LipSyncPlugin,
            // Viewer product surface.
            ConfigPlugin,
            ViewerCameraPlugin,
            StagingPlugin,
            AvatarPlugin,
            BrainPlugin,
            CharacterPlugin,
            TtsPlugin,
            SttPlugin,
            ToastPlugin,
            UiPlugin,
        ))
        .add_systems(Update, keyboard_shortcuts)
        .run();
}

/// egui-independent fallbacks so the viewer is fully usable (and demoable) from the keyboard:
/// number keys fire expressions, R resets the camera, 0 clears.
fn keyboard_shortcuts(
    mut commands: Commands,
    input: Res<ButtonInput<KeyCode>>,
    vrms: Query<Entity, With<Vrm>>,
    mut reset_cam: MessageWriter<ResetCamera>,
    mut shot: MessageWriter<TakeScreenshot>,
) {
    if input.just_pressed(KeyCode::KeyR) {
        reset_cam.write(ResetCamera);
    }
    if input.just_pressed(KeyCode::F12) {
        shot.write(TakeScreenshot);
    }

    const KEYS: [(KeyCode, &str); 4] = [
        (KeyCode::Digit1, "happy"),
        (KeyCode::Digit2, "angry"),
        (KeyCode::Digit3, "sad"),
        (KeyCode::Digit4, "relaxed"),
    ];
    for (key, expr) in KEYS {
        if input.just_pressed(key) {
            for vrm in &vrms {
                commands.trigger(SetExpressions::single(vrm, expr, 1.0));
            }
        }
    }
    if input.just_pressed(KeyCode::Digit0) {
        for vrm in &vrms {
            commands.trigger(ClearExpressions { entity: vrm });
        }
    }
}
