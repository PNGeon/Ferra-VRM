//! Orbit camera + HDR/bloom/ACES so the avatar reads as a polished product, not a debug void.
//! Drag-orbit, scroll-zoom, and a reset-to-home shortcut (R) / button (egui panel).

use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::render::view::Hdr;
use bevy_panorbit_camera::{PanOrbitCamera, PanOrbitCameraPlugin};

pub struct ViewerCameraPlugin;

impl Plugin for ViewerCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(PanOrbitCameraPlugin)
            .add_message::<ResetCamera>()
            .add_systems(Startup, spawn_camera)
            .add_systems(Update, apply_reset);
    }
}

/// Sent by the UI/keyboard to snap the camera back to its framing of the avatar.
#[derive(Message, Default)]
pub struct ResetCamera;

// Home framing — eye level, slightly above center, a comfortable portrait distance.
const HOME_FOCUS: Vec3 = Vec3::new(0.0, 1.0, 0.0);
const HOME_YAW: f32 = 0.0;
const HOME_PITCH: f32 = -0.08;
const HOME_RADIUS: f32 = 2.4;

fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Hdr, // required for bloom to register highlights
        Tonemapping::AcesFitted,
        Bloom::NATURAL,
        PanOrbitCamera {
            focus: HOME_FOCUS,
            target_focus: HOME_FOCUS,
            yaw: Some(HOME_YAW),
            pitch: Some(HOME_PITCH),
            radius: Some(HOME_RADIUS),
            target_yaw: HOME_YAW,
            target_pitch: HOME_PITCH,
            target_radius: HOME_RADIUS,
            ..default()
        },
        Transform::from_xyz(0.0, 1.0, HOME_RADIUS).looking_at(HOME_FOCUS, Vec3::Y),
    ));
}

fn apply_reset(mut events: MessageReader<ResetCamera>, mut cams: Query<&mut PanOrbitCamera>) {
    if events.is_empty() {
        return;
    }
    events.clear();
    for mut cam in &mut cams {
        cam.target_focus = HOME_FOCUS;
        cam.target_yaw = HOME_YAW;
        cam.target_pitch = HOME_PITCH;
        cam.target_radius = HOME_RADIUS;
    }
}
