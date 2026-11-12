//! Movement prediction and reconciliation.
//!
//! The client applies its own inputs immediately (see `input::controller`), so movement feels
//! instant at any latency. When the server's authoritative position arrives it can disagree
//! with the prediction; small disagreements are removed smoothly over a short window instead of
//! snapping the camera, and large ones are applied immediately.
//!
//! The rewind-and-replay buffer is driven by the transport: it needs the server's
//! acknowledgement sequence numbers, which only exist once the multiplayer layer is connected.
//! [`PredictionState`] already holds the buffer and its bookkeeping so the transport only has to
//! call [`PredictionState::record`] and [`PredictionState::reconcile`].
//!
//! Until the transport exists, these types are exercised only by the unit tests below, so
//! dead-code analysis is silenced for this module rather than deleting the client half of the
//! contract.
#![allow(dead_code)]

use bevy::prelude::*;
use deepcore_shared::movement::PlayerBody;

/// One input the client has applied locally and is waiting to have acknowledged.
#[derive(Debug, Clone, Copy)]
pub struct PendingInput {
    /// Sequence number, increasing with every input sent.
    pub seq: u32,
    /// The body state produced by applying this input locally.
    pub predicted: PlayerBody,
}

/// The client's prediction buffer and smoothing state.
#[derive(Resource, Default)]
pub struct PredictionState {
    /// Inputs awaiting acknowledgement, oldest first.
    pub pending: Vec<PendingInput>,
    /// The next sequence number to send.
    pub next_seq: u32,
    /// Corrections applied since the last stats reset, for the debug overlay.
    pub corrections_this_second: u32,
    /// A visual offset still being smoothed away.
    smoothing_offset: Vec3,
    /// Remaining smoothing time.
    smoothing_remaining: f32,
}

/// Positions closer than this are smoothed rather than snapped, in blocks.
pub const SMOOTH_THRESHOLD: f32 = 0.3;

/// How long a small correction is smoothed over, in seconds.
pub const SMOOTH_DURATION: f32 = 0.1;

/// The maximum number of unacknowledged inputs kept in the buffer.
pub const MAX_PENDING: usize = 256;

/// What reconciliation decided to do.
///
/// The transport uses this to decide whether to replay inputs or just show the server state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reconciliation {
    /// A small correction, being removed smoothly.
    Smoothed {
        /// The offset being removed.
        delta: Vec3,
    },
    /// A large correction: the client snapped to the server.
    Snapped,
}

impl PredictionState {
    /// Records an input that was just applied locally and should be sent to the server.
    ///
    /// Returns the sequence number to put on the wire.
    pub fn record(&mut self, body: PlayerBody) -> u32 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        self.pending.push(PendingInput {
            seq,
            predicted: body,
        });
        if self.pending.len() > MAX_PENDING {
            self.pending.remove(0);
        }
        seq
    }

    /// Handles an authoritative state from the server.
    pub fn reconcile(&mut self, authoritative: PlayerBody, acked_seq: u32) -> Reconciliation {
        // Compare against the newest local prediction *before* dropping acknowledged inputs:
        // the acknowledged ones are exactly the states the server is about to correct.
        let last_predicted = self
            .pending
            .last()
            .map_or(authoritative.position, |p| p.predicted.position);

        self.pending.retain(|p| p.seq > acked_seq);

        let delta = Vec3::new(
            authoritative.position[0] - last_predicted[0],
            authoritative.position[1] - last_predicted[1],
            authoritative.position[2] - last_predicted[2],
        );

        if delta.length() <= SMOOTH_THRESHOLD {
            self.smoothing_offset = delta;
            self.smoothing_remaining = SMOOTH_DURATION;
            Reconciliation::Smoothed { delta }
        } else {
            self.corrections_this_second += 1;
            self.smoothing_offset = Vec3::ZERO;
            self.smoothing_remaining = 0.0;
            Reconciliation::Snapped
        }
    }

    /// Decays the smoothing offset and returns the offset the camera should still apply.
    pub fn tick_smoothing(&mut self, dt: f32) -> Vec3 {
        if self.smoothing_remaining <= 0.0 {
            self.smoothing_offset = Vec3::ZERO;
            return Vec3::ZERO;
        }
        self.smoothing_remaining = (self.smoothing_remaining - dt).max(0.0);
        let fraction = self.smoothing_remaining / SMOOTH_DURATION;
        self.smoothing_offset * fraction
    }

    /// The number of inputs still awaiting acknowledgement.
    #[must_use]
    pub fn unacked(&self) -> usize {
        self.pending.len()
    }

    /// Clears the per-second correction counter, called once a second by the HUD.
    pub fn reset_correction_counter(&mut self) {
        self.corrections_this_second = 0;
    }
}

/// Prediction plugin.
pub struct PredictionPlugin;

impl Plugin for PredictionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PredictionState>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(x: f32) -> PlayerBody {
        PlayerBody::new([x, 0.0, 0.0])
    }

    #[test]
    fn small_corrections_are_smoothed() {
        let mut state = PredictionState::default();
        state.record(body(1.0));
        let result = state.reconcile(body(1.1), 0);
        assert!(matches!(result, Reconciliation::Smoothed { .. }));
        assert_eq!(state.corrections_this_second, 0);
    }

    #[test]
    fn large_corrections_snap() {
        let mut state = PredictionState::default();
        state.record(body(1.0));
        let result = state.reconcile(body(9.0), 0);
        assert_eq!(result, Reconciliation::Snapped);
        assert_eq!(state.corrections_this_second, 1);
    }

    #[test]
    fn acknowledged_inputs_are_dropped() {
        let mut state = PredictionState::default();
        state.record(body(1.0));
        state.record(body(2.0));
        state.record(body(3.0));
        assert_eq!(state.pending.len(), 3);
        state.reconcile(body(2.0), 1);
        assert_eq!(state.pending.len(), 1, "only the unacked input remains");
        assert_eq!(state.pending[0].seq, 2);
    }

    #[test]
    fn buffer_is_bounded() {
        let mut state = PredictionState::default();
        for i in 0..(MAX_PENDING + 50) {
            state.record(body(i as f32));
        }
        assert_eq!(state.pending.len(), MAX_PENDING);
    }

    #[test]
    fn smoothing_decays_to_zero() {
        let mut state = PredictionState::default();
        state.record(body(1.0));
        state.reconcile(body(1.2), 0);
        let first = state.tick_smoothing(0.05);
        let second = state.tick_smoothing(0.05);
        let third = state.tick_smoothing(0.05);
        assert!(first.length() > 0.0);
        assert!(second.length() < first.length());
        assert_eq!(third, Vec3::ZERO);
    }
}
