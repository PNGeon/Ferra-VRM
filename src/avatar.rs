//! Avatar lifecycle: drag-and-drop loading of any `.vrm` / `.vrma`, and reconciling the
//! idle-aliveness (auto-blink) onto the live avatar.
//!
//! Drag-drop loads files from ANYWHERE on disk: bevy joins the dropped absolute path over the asset
//! root (std discards the base for absolute paths) and `load_override` bypasses the approval check.
//!
//! NOTE: eye-gaze (`LookAt`) is intentionally not attached — `bevy_vrm1` 0.7 only implements
//! bone-based lookAt and panics on expression-based lookAt (common in VRoid models). Auto-blink
//! keeps the avatar alive without that crash risk; gaze returns once it's type-aware.

use bevy::animation::RepeatAnimation;
use bevy::prelude::*;
use bevy_vrm1::prelude::*;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use vrm_stage_core::{Blink, LipSync, spawn_avatar};

pub struct AvatarPlugin;

impl Plugin for AvatarPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CurrentAvatar>()
            .init_resource::<IdleSettings>()
            .add_systems(Startup, setup_world)
            .add_systems(Update, (handle_drops, apply_idle_settings));
    }
}

/// The live avatar's root VRM entity (None before the first load / between swaps).
#[derive(Resource, Default)]
pub struct CurrentAvatar(pub Option<Entity>);

/// User-tunable aliveness, edited in the panel and reconciled onto the avatar each change.
#[derive(Resource, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IdleSettings {
    pub auto_blink: bool,
}

impl Default for IdleSettings {
    fn default() -> Self {
        Self { auto_blink: true }
    }
}

/// Marks avatars spawned by the viewer (so future logic can target only ours).
#[derive(Component)]
pub struct ViewerAvatar;

fn setup_world(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut current: ResMut<CurrentAvatar>,
    settings: Res<IdleSettings>,
) {
    // Load a bundled avatar only if one ships with the build. Public releases ship NONE (we don't
    // redistribute a non-cleared model), so the empty-state drop hint guides the user to drop their
    // own VRM. Local/dev builds with assets/vrm/sample.vrm present load it for convenience.
    if bundled_avatar_present() {
        let entity = spawn_avatar(&mut commands, &assets, "vrm/sample.vrm", "vrma/idle.vrma");
        attach_aliveness(&mut commands, entity, &settings);
        current.0 = Some(entity);
        info!("[ferra-vrm] avatar up — drop a .vrm to swap, .vrma to animate.");
    } else {
        info!("[ferra-vrm] no bundled avatar — drop a .vrm onto the window to begin.");
    }
}

/// True if a bundled sample avatar exists on disk (relative to the asset root). Used to decide
/// whether to auto-load one at startup; public builds ship without it.
fn bundled_avatar_present() -> bool {
    // Assets resolve at {BEVY_ASSET_ROOT}/assets/... (root defaults to the working dir).
    let base = std::env::var("BEVY_ASSET_ROOT").unwrap_or_else(|_| ".".into());
    std::path::Path::new(&base)
        .join("assets")
        .join("vrm")
        .join("sample.vrm")
        .exists()
}

/// Attach lipsync and (optional) auto-blink to a freshly spawned avatar.
fn attach_aliveness(commands: &mut Commands, entity: Entity, settings: &IdleSettings) {
    let mut ec = commands.entity(entity);
    ec.insert((LipSync::default(), ViewerAvatar));
    if settings.auto_blink {
        ec.insert(Blink::default());
    }
}

fn handle_drops(
    mut drops: MessageReader<FileDragAndDrop>,
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut current: ResMut<CurrentAvatar>,
    settings: Res<IdleSettings>,
    mut toasts: MessageWriter<crate::toast::Toast>,
) {
    for event in drops.read() {
        let FileDragAndDrop::DroppedFile { path_buf, .. } = event else {
            continue;
        };
        let ext = path_buf
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);

        match ext.as_deref() {
            Some("vrm") => {
                if let Some(old) = current.0.take() {
                    commands.entity(old).despawn();
                }
                let entity = commands
                    .spawn(VrmHandle(assets.load_override(path_buf.clone())))
                    .with_children(|cmd| {
                        // Keep the bundled idle so a dropped static avatar still breathes.
                        cmd.spawn(VrmaHandle(assets.load("vrma/idle.vrma")))
                            .observe(play_vrma_on_load);
                    })
                    .id();
                attach_aliveness(&mut commands, entity, &settings);
                current.0 = Some(entity);
                toasts.write(crate::toast::Toast(format!(
                    "Loaded avatar: {}",
                    path_buf.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                )));
                info!("[ferra-vrm] loaded VRM: {}", path_buf.display());
            }
            Some("vrma") => {
                if let Some(root) = current.0 {
                    commands.entity(root).with_children(|cmd| {
                        cmd.spawn(VrmaHandle(assets.load_override(path_buf.clone())))
                            .observe(play_vrma_on_load);
                    });
                    info!("[ferra-vrm] playing VRMA: {}", path_buf.display());
                } else {
                    toasts.write(crate::toast::Toast(
                        "Dropped a .vrma but no avatar is loaded yet.".into(),
                    ));
                }
            }
            // .toml character cards are handled by character::handle_card_drops.
            Some("toml") => {}
            _ => {
                toasts.write(crate::toast::Toast(format!(
                    "Unsupported file: {}",
                    path_buf.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                )));
                warn!("[ferra-vrm] unsupported file: {}", path_buf.display());
            }
        }
    }
}

fn play_vrma_on_load(trigger: On<LoadedVrma>, mut commands: Commands) {
    commands.trigger(PlayVrma {
        repeat: RepeatAnimation::Forever,
        transition_duration: Duration::ZERO,
        vrma: trigger.vrma,
        reset_spring_bones: false,
    });
}

/// Reconcile auto-blink onto the avatar whenever the panel toggles it.
fn apply_idle_settings(
    mut commands: Commands,
    settings: Res<IdleSettings>,
    current: Res<CurrentAvatar>,
) {
    if !settings.is_changed() {
        return;
    }
    let Some(entity) = current.0 else {
        return;
    };
    if settings.auto_blink {
        commands.entity(entity).insert(Blink::default());
    } else {
        commands.entity(entity).remove::<Blink>();
    }
}
