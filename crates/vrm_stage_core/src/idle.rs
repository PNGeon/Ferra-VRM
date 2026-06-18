//! Idle aliveness — a generic "she's breathing" layer: auto-blink and a slowly drifting gaze
//! target for idle eye movement. Just enough to make a loaded avatar read as alive with no AI
//! attached (auto-blink + auto-look-at + idle eye movement).
//!
//! Everything composes through `ModifyExpressions` (partial updates), so blink layers cleanly
//! over whatever emotion/viseme is active without wiping it.

use bevy::prelude::*;
use bevy_vrm1::prelude::*;

/// Drives [`Blink`] and [`GazeTarget`]. Add once; attach the components where you want them.
pub struct IdleAlivePlugin;

impl Plugin for IdleAlivePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (auto_blink, drift_gaze_target));
    }
}

/// Cheap deterministic noise so we don't pull in `rand`. xorshift32 → `0.0..1.0`.
#[derive(Clone, Copy)]
struct Rng(u32);
impl Rng {
    fn next_f32(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }
    /// Uniform in `[lo, hi)`.
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.next_f32() * (hi - lo)
    }
}

/// Opt-in auto-blink. Attach to a VRM entity; the plugin closes/opens the `blink` expression
/// on a natural, slightly irregular cadence.
#[derive(Component)]
pub struct Blink {
    /// Seconds until the next blink begins.
    wait: f32,
    /// Progress through the current blink, in seconds (None = waiting).
    closing: Option<f32>,
    rng: Rng,
}

impl Default for Blink {
    fn default() -> Self {
        Self {
            wait: 2.0,
            closing: None,
            rng: Rng(0x9E37_79B9),
        }
    }
}

/// One full blink (down then up) lasts this long.
const BLINK_SECS: f32 = 0.14;

fn auto_blink(
    mut commands: Commands,
    mut q: Query<(Entity, &mut Blink), With<Vrm>>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    for (entity, mut blink) in &mut q {
        match blink.closing {
            None => {
                blink.wait -= dt;
                if blink.wait <= 0.0 {
                    blink.closing = Some(0.0);
                }
            }
            Some(mut t) => {
                t += dt;
                // Triangular weight: 0 → 1 → 0 across BLINK_SECS.
                let w = (1.0 - (2.0 * t / BLINK_SECS - 1.0).abs()).clamp(0.0, 1.0);
                commands.trigger(ModifyExpressions::single(entity, "blink", w));
                if t >= BLINK_SECS {
                    commands.trigger(ModifyExpressions::single(entity, "blink", 0.0));
                    blink.closing = None;
                    // Humans blink roughly every 2–6s; double-blinks occasionally.
                    blink.wait = blink.rng.range(2.0, 6.0);
                } else {
                    blink.closing = Some(t);
                }
            }
        }
    }
}

/// A drifting point in world space for a VRM to gaze at when not tracking the cursor.
/// Spawn one with [`spawn_gaze_target`], then set the avatar's `LookAt::Target(that_entity)`.
#[derive(Component)]
pub struct GazeTarget {
    /// Point the gaze loiters around (roughly where a viewer's face would be).
    center: Vec3,
    /// Half-extents of the idle wander box around `center`.
    spread: Vec3,
    aim: Vec3,
    wait: f32,
    rng: Rng,
}

/// Spawn an idle gaze target hovering around `center` (e.g. eye level, slightly toward camera).
/// Returns the entity so you can point `LookAt::Target(..)` at it.
pub fn spawn_gaze_target(commands: &mut Commands, center: Vec3) -> Entity {
    commands
        .spawn((
            Transform::from_translation(center),
            GazeTarget {
                center,
                spread: Vec3::new(0.35, 0.18, 0.0),
                aim: center,
                wait: 0.0,
                rng: Rng(0x1234_5678),
            },
        ))
        .id()
}

fn drift_gaze_target(mut q: Query<(&mut Transform, &mut GazeTarget)>, time: Res<Time>) {
    let dt = time.delta_secs();
    for (mut tf, mut g) in &mut q {
        g.wait -= dt;
        if g.wait <= 0.0 {
            // New saccade target within the wander box; dwell 0.8–2.5s.
            let (cx, cy, cz) = (g.center.x, g.center.y, g.center.z);
            let (sx, sy, sz) = (g.spread.x, g.spread.y, g.spread.z);
            g.aim = Vec3::new(
                cx + g.rng.range(-sx, sx),
                cy + g.rng.range(-sy, sy),
                cz + g.rng.range(-sz, sz),
            );
            g.wait = g.rng.range(0.8, 2.5);
        }
        // Ease toward the aim so eyes glide rather than snap.
        let aim = g.aim;
        tf.translation = tf.translation.lerp(aim, (6.0 * dt).min(1.0));
    }
}
