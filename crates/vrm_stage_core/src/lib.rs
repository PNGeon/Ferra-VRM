//! vrm_stage_core — the shared foundation the viewer builds on. Everything here is generic and
//! asset-agnostic: VRM/VRMA loading, spring bones (free from bevy_vrm1), expressions, idle
//! aliveness, lipsync, and optional KTX2-textured rooms.

use bevy::animation::RepeatAnimation;
use bevy::prelude::*;
use bevy_vrm1::prelude::*;
use std::time::Duration;

mod idle;
mod lipsync;
pub use idle::{Blink, GazeTarget, IdleAlivePlugin, spawn_gaze_target};
pub use lipsync::{LipSync, LipSyncPlugin};

/// Bundles the VRM + VRMA plugins. Add once per app.
pub struct VrmStageCorePlugin;

impl Plugin for VrmStageCorePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((VrmPlugin, VrmaPlugin));
    }
}

/// Spawn a VRM avatar and loop an idle VRMA on it (retargeted as a child, played on load).
/// Spring bones run automatically. `vrm_path` / `idle_vrma_path` are asset-server paths.
/// Returns the root VRM entity so callers can attach `LookAt` / [`Blink`] / [`LipSync`] etc.
pub fn spawn_avatar(
    commands: &mut Commands,
    asset_server: &AssetServer,
    vrm_path: &str,
    idle_vrma_path: &str,
) -> Entity {
    commands
        .spawn(VrmHandle(asset_server.load(vrm_path.to_string())))
        .with_children(|cmd| {
            cmd.spawn(VrmaHandle(asset_server.load(idle_vrma_path.to_string())))
                .observe(play_vrma_on_load);
        })
        .id()
}

fn play_vrma_on_load(trigger: On<LoadedVrma>, mut commands: Commands) {
    commands.trigger(PlayVrma {
        repeat: RepeatAnimation::Forever,
        transition_duration: Duration::ZERO,
        vrma: trigger.vrma,
        reset_spring_bones: false,
    });
}

/// Load a KTX2/Basis-textured room glb as a scene at the origin.
/// (Rooms must be transcoded to KTX2 — Bevy can't read EXT_texture_webp directly.)
pub fn spawn_room(commands: &mut Commands, asset_server: &AssetServer, room_path: &str) {
    commands.spawn((
        SceneRoot(asset_server.load(GltfAssetLabel::Scene(0).from_asset(room_path.to_string()))),
        Transform::default(),
    ));
}

/// A key directional light with shadows — a sane default for a single avatar.
pub fn spawn_default_lighting(commands: &mut Commands) {
    commands.spawn((
        DirectionalLight {
            shadows_enabled: true,
            ..default()
        },
        Transform::from_xyz(3.0, 3.0, 1.5).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// A camera framing a standing avatar at human scale (eye-level, front, ~1.6m back).
pub fn spawn_portrait_camera(commands: &mut Commands) {
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 1.3, 1.6).looking_at(Vec3::new(0.0, 1.2, 0.0), Vec3::Y),
    ));
}

/// Optional "she's alive" demo: cycles visemes (aa/ih/ou/ee) + an emotion on a timer.
/// Proves the expression/lipsync API; useful for the public demo and for stage testing.
pub struct ExpressionDemoPlugin;

impl Plugin for ExpressionDemoPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, cycle_expressions);
    }
}

fn cycle_expressions(
    mut commands: Commands,
    vrms: Query<Entity, With<Vrm>>,
    time: Res<Time>,
    mut t: Local<f32>,
    mut i: Local<usize>,
) {
    *t += time.delta_secs();
    if *t < 0.6 {
        return;
    }
    *t = 0.0;
    const VOWELS: [&str; 4] = ["aa", "ih", "ou", "ee"];
    const FACES: [&str; 3] = ["happy", "angry", "blink"];
    for vrm in vrms.iter() {
        commands.trigger(ModifyExpressions::mouth(
            vrm,
            VOWELS[*i % VOWELS.len()],
            1.0,
        ));
        if (*i).is_multiple_of(4) {
            commands.trigger(SetExpressions::single(
                vrm,
                FACES[(*i / 4) % FACES.len()],
                1.0,
            ));
        }
    }
    *i += 1;
}
