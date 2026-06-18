//! Product-grade staging: a ground plane, a tuned 3-point light rig, ambient fill, and a calm
//! backdrop color. This is the difference between "tech demo in a void" and "a thing you'd ship."
//! (A CC0 KTX2 room can later replace the plane via `vrm_stage_core::spawn_room` — see PLAN.md.)

use bevy::prelude::*;

pub struct StagingPlugin;

impl Plugin for StagingPlugin {
    fn build(&self, app: &mut App) {
        // GlobalAmbientLight is the scene-wide ambient resource in bevy 0.18
        // (per-camera `AmbientLight` is now a component).
        app.insert_resource(ClearColor(Color::srgb(0.10, 0.11, 0.14)))
            .insert_resource(GlobalAmbientLight {
                brightness: 250.0,
                ..default()
            })
            .add_systems(Startup, setup_stage);
    }
}

fn setup_stage(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Ground — large, matte, slightly cool so the avatar's skin tones pop against it.
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::new(Vec3::Y, Vec2::new(20.0, 20.0)))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.14, 0.15, 0.18),
            perceptual_roughness: 0.95,
            ..default()
        })),
    ));

    // Key light — warm, casts shadows, the primary form-defining light.
    commands.spawn((
        DirectionalLight {
            illuminance: 8_000.0,
            color: Color::srgb(1.0, 0.96, 0.9),
            shadows_enabled: true,
            ..default()
        },
        Transform::from_xyz(3.0, 4.0, 2.5).looking_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
    ));

    // Fill light — cool, soft, no shadows; lifts the shadow side.
    commands.spawn((
        DirectionalLight {
            illuminance: 2_500.0,
            color: Color::srgb(0.8, 0.85, 1.0),
            shadows_enabled: false,
            ..default()
        },
        Transform::from_xyz(-3.5, 2.0, 1.5).looking_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
    ));

    // Rim/back light — separates the silhouette from the backdrop.
    commands.spawn((
        DirectionalLight {
            illuminance: 4_000.0,
            color: Color::srgb(0.9, 0.92, 1.0),
            shadows_enabled: false,
            ..default()
        },
        Transform::from_xyz(0.0, 3.0, -3.0).looking_at(Vec3::new(0.0, 1.2, 0.0), Vec3::Y),
    ));
}
