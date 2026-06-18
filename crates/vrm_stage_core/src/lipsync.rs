//! Lightweight lipsync. Drop a [`LipSync`] component on a VRM, then drive the mouth one of two ways:
//! - [`LipSync::speak`] / [`LipSync::feed`] — text → viseme heuristic (cheap; used when no TTS).
//! - [`LipSync::speak_envelope`] — an audio RMS amplitude envelope → mouth-open over time (used by
//!   the TTS path so the mouth tracks the actual voice).
//!
//! The audio path drives a single `aa` jaw shape modulated by `min(0.85, rms * 13)` through an
//! **asymmetric envelope follower** (fast open, slow close). RMS is on a normalized [-1,1] scale,
//! and the follower is dt-normalized so it behaves the same at any framerate.

use bevy::prelude::*;
use bevy_vrm1::prelude::*;
use std::collections::VecDeque;

/// Drives every [`LipSync`] component. Add once.
pub struct LipSyncPlugin;

impl Plugin for LipSyncPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, drive_lipsync);
    }
}

/// Text mode: how long each viseme is held. ~14/sec reads as natural-ish speech.
const TEXT_FRAME_SECS: f32 = 0.07;

// Audio mode tuning: RMS gain, mouth-open cap, and envelope-follower time constants.
const LIP_GAIN: f32 = 13.0;
const LIP_CAP: f32 = 0.85;
/// Envelope-follower time constants: fast open (~24 ms), slow close (~200 ms).
const LIP_TAU_ATTACK: f32 = 0.024;
const LIP_TAU_RELEASE: f32 = 0.20;
/// The single open-mouth shape the audio path modulates.
const LIP_VISEME: &str = "aa";

/// Mouth driver for a VRM. Text mode steps a `(viseme, weight)` queue; audio mode follows a
/// precomputed RMS envelope with a smoothing follower.
#[derive(Component, Default)]
pub struct LipSync {
    // text mode
    queue: VecDeque<(Option<&'static str>, f32)>,
    t: f32,
    frame_secs: f32,
    // audio (envelope) mode
    env: Vec<f32>,
    env_frame_secs: f32,
    env_clock: f32,
    env_active: bool,
    amp: f32, // smoothed mouth-open (the follower state)
}

impl LipSync {
    /// Replace the queue with text-derived visemes (whole line). Used when TTS is off.
    pub fn speak(&mut self, text: &str) {
        self.queue.clear();
        self.frame_secs = TEXT_FRAME_SECS;
        self.push_text(text);
    }

    /// Append text-derived visemes for a streamed chunk (per-token while a response streams in).
    pub fn feed(&mut self, chunk: &str) {
        if self.frame_secs <= 0.0 {
            self.frame_secs = TEXT_FRAME_SECS;
        }
        self.push_text(chunk);
    }

    /// Drive the mouth from an audio RMS envelope (one amplitude per `frame_secs`). The follower in
    /// `drive_lipsync` turns it into smooth jaw motion synced to the voice.
    pub fn speak_envelope(&mut self, amps: &[f32], frame_secs: f32) {
        self.queue.clear();
        self.env = amps.to_vec();
        self.env_frame_secs = frame_secs.max(0.005);
        self.env_clock = 0.0;
        self.env_active = true;
    }

    /// True while there's anything left to play (text frames or a live audio envelope).
    pub fn is_speaking(&self) -> bool {
        !self.queue.is_empty() || self.env_active
    }

    /// Stop immediately; the mouth releases shut.
    pub fn silence(&mut self) {
        self.queue.clear();
        self.env_active = false;
    }

    fn push_text(&mut self, text: &str) {
        for ch in text.chars() {
            let frame = match ch.to_ascii_lowercase() {
                'a' => (Some("aa"), 0.85),
                'i' | 'y' => (Some("ih"), 0.85),
                'u' | 'w' => (Some("ou"), 0.85),
                'e' => (Some("ee"), 0.85),
                'o' => (Some("oh"), 0.85),
                ' ' | '\n' | '\t' | '.' | ',' | '!' | '?' => (None, 0.0),
                c if c.is_alphabetic() => (Some("aa"), 0.5),
                _ => continue,
            };
            self.queue.push_back(frame);
        }
    }
}

fn drive_lipsync(
    mut commands: Commands,
    mut q: Query<(Entity, &mut LipSync), With<Vrm>>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    for (entity, mut lip) in &mut q {
        // Audio (envelope) mode takes precedence, including the release tail after it ends.
        if lip.env_active || lip.amp > 0.001 {
            let target = if lip.env_active {
                lip.env_clock += dt;
                let idx = (lip.env_clock / lip.env_frame_secs) as usize;
                if idx >= lip.env.len() {
                    lip.env_active = false;
                    0.0
                } else {
                    (lip.env[idx] * LIP_GAIN).min(1.0)
                }
            } else {
                0.0
            };
            // Asymmetric follower: fast open, slow close (dt-normalized → framerate-independent).
            let tau = if target > lip.amp {
                LIP_TAU_ATTACK
            } else {
                LIP_TAU_RELEASE
            };
            let coeff = 1.0 - (-dt / tau).exp();
            lip.amp += (target - lip.amp) * coeff;
            if !lip.env_active && lip.amp < 0.005 {
                lip.amp = 0.0;
            }
            commands.trigger(ModifyExpressions::mouth(
                entity,
                LIP_VISEME,
                lip.amp.min(LIP_CAP),
            ));
            continue;
        }

        // Text mode: step the viseme queue.
        if lip.queue.is_empty() {
            continue;
        }
        let frame_secs = lip.frame_secs.max(0.01);
        lip.t += dt;
        if lip.t < frame_secs {
            continue;
        }
        lip.t = 0.0;
        match lip.queue.pop_front() {
            Some((Some(v), w)) => commands.trigger(ModifyExpressions::mouth(entity, v, w)),
            _ => commands.trigger(ModifyExpressions::mouth(entity, "aa", 0.0)),
        }
    }
}
