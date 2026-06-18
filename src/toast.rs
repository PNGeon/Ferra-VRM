//! Tiny transient toast notifications — surface errors (bad file drops, LLM failures) in the UI
//! instead of only in the log. Anyone can `MessageWriter<Toast>::write(Toast("…"))`; the UI renders
//! the active ones and they fade out on a timer.

use bevy::prelude::*;

pub struct ToastPlugin;

impl Plugin for ToastPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Toasts>()
            .add_message::<Toast>()
            .add_systems(Update, (ingest_toasts, tick_toasts));
    }
}

/// Show a transient message to the user.
#[derive(Message)]
pub struct Toast(pub String);

/// A live toast and its remaining lifetime (seconds).
pub struct ToastItem {
    pub text: String,
    pub ttl: f32,
}

/// Currently-visible toasts (read by the UI).
#[derive(Resource, Default)]
pub struct Toasts(pub Vec<ToastItem>);

const TOAST_SECS: f32 = 5.0;
const MAX_TOASTS: usize = 5;

fn ingest_toasts(mut events: MessageReader<Toast>, mut toasts: ResMut<Toasts>) {
    for Toast(text) in events.read() {
        toasts.0.push(ToastItem {
            text: text.clone(),
            ttl: TOAST_SECS,
        });
        while toasts.0.len() > MAX_TOASTS {
            toasts.0.remove(0);
        }
    }
}

fn tick_toasts(time: Res<Time>, mut toasts: ResMut<Toasts>) {
    let dt = time.delta_secs();
    for item in &mut toasts.0 {
        item.ttl -= dt;
    }
    toasts.0.retain(|i| i.ttl > 0.0);
}
