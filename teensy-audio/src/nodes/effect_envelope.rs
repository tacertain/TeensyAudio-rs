//! ADSR envelope effect.
//!
//! Port of `TeensyAudio/effect_envelope.cpp`. Applies an
//! Attack-Decay-Sustain-Release (ADSR) envelope to audio input.
//! Processes 8 samples at a time with per-sample gain interpolation.

use crate::block::{AudioBlockMut, AudioBlockRef};
use crate::constants::{AUDIO_BLOCK_SAMPLES, AUDIO_SAMPLE_RATE_EXACT};
use crate::dsp::intrinsics::saturate16;
use crate::node::AudioNode;

/// Samples per millisecond at the audio sample rate.
const SAMPLES_PER_MSEC: f32 = AUDIO_SAMPLE_RATE_EXACT / 1000.0;

/// Number of samples per envelope processing group.
const SAMPLES_PER_GROUP: u32 = 8;

/// Unity gain in the high-resolution envelope scale (30-bit).
const UNITY_GAIN: i32 = 0x4000_0000;

/// Envelope state machine states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeState {
    /// No sound output.
    Idle = 0,
    /// Initial delay before attack begins.
    Delay = 1,
    /// Ramping up to full volume.
    Attack = 2,
    /// Holding at full volume.
    Hold = 3,
    /// Ramping down to sustain level.
    Decay = 4,
    /// Holding at sustain level.
    Sustain = 5,
    /// Ramping down to silence after note-off.
    Release = 6,
    /// Fast release for re-triggering.
    Forced = 7,
}

/// ADSR envelope effect. Shapes audio volume over time.
///
/// Effect node: 1 input, 1 output.
///
/// # Example
/// ```ignore
/// let mut env = AudioEffectEnvelope::new();
/// env.attack(10.5);
/// env.decay(35.0);
/// env.sustain(0.5);
/// env.release(300.0);
///
/// env.note_on();   // trigger
/// // ... some time later ...
/// env.note_off();  // release
/// ```
pub struct AudioEffectEnvelope {
    /// Current state.
    state: EnvelopeState,
    /// Remaining time in current state, in 8-sample groups.
    count: u16,
    /// Current envelope level (0 = off, UNITY_GAIN = full).
    mult_hires: i32,
    /// Change in mult_hires per 8-sample group.
    inc_hires: i32,

    // Configuration (in 8-sample group counts)
    delay_count: u16,
    attack_count: u16,
    hold_count: u16,
    decay_count: u16,
    sustain_mult: i32,
    release_count: u16,
    release_forced_count: u16,
}

impl AudioEffectEnvelope {
    /// Create a new envelope with default settings matching the C++ library:
    /// - delay: 0 ms
    /// - attack: 10.5 ms
    /// - hold: 2.5 ms
    /// - decay: 35 ms
    /// - sustain: 0.5
    /// - release: 300 ms
    /// - releaseNoteOn: 5 ms
    pub fn new() -> Self {
        let mut env = AudioEffectEnvelope {
            state: EnvelopeState::Idle,
            count: 0,
            mult_hires: 0,
            inc_hires: 0,
            delay_count: 0,
            attack_count: 1,
            hold_count: 0,
            decay_count: 1,
            sustain_mult: 0,
            release_count: 1,
            release_forced_count: 0,
        };
        env.delay(0.0);
        env.attack(10.5);
        env.hold(2.5);
        env.decay(35.0);
        env.sustain(0.5);
        env.release(300.0);
        env.release_note_on(5.0);
        env
    }

    /// Convert milliseconds to count of 8-sample groups.
    fn milliseconds2count(milliseconds: f32) -> u16 {
        let ms = if milliseconds < 0.0 { 0.0 } else { milliseconds };
        let c = ((ms * SAMPLES_PER_MSEC) as u32 + 7) >> 3;
        if c > 65535 { 65535 } else { c as u16 }
    }

    /// Set initial delay before attack (milliseconds).
    pub fn delay(&mut self, milliseconds: f32) {
        self.delay_count = Self::milliseconds2count(milliseconds);
    }

    /// Set attack time (milliseconds). Minimum 1 group.
    pub fn attack(&mut self, milliseconds: f32) {
        let count = Self::milliseconds2count(milliseconds);
        self.attack_count = if count == 0 { 1 } else { count };
    }

    /// Set hold time at peak level (milliseconds).
    pub fn hold(&mut self, milliseconds: f32) {
        self.hold_count = Self::milliseconds2count(milliseconds);
    }

    /// Set decay time (milliseconds). Minimum 1 group.
    pub fn decay(&mut self, milliseconds: f32) {
        let count = Self::milliseconds2count(milliseconds);
        self.decay_count = if count == 0 { 1 } else { count };
    }

    /// Set sustain level (0.0 = silent, 1.0 = full volume).
    pub fn sustain(&mut self, level: f32) {
        let clamped = if level < 0.0 { 0.0 } else if level > 1.0 { 1.0 } else { level };
        self.sustain_mult = (clamped * 1_073_741_824.0) as i32;
    }

    /// Set release time (milliseconds). Minimum 1 group.
    pub fn release(&mut self, milliseconds: f32) {
        let count = Self::milliseconds2count(milliseconds);
        self.release_count = if count == 0 { 1 } else { count };
    }

    /// Set the forced-release time for re-triggering notes (milliseconds).
    pub fn release_note_on(&mut self, milliseconds: f32) {
        let count = Self::milliseconds2count(milliseconds);
        self.release_forced_count = if count == 0 { 1 } else { count };
    }

    /// Trigger the envelope (start the attack phase).
    pub fn note_on(&mut self) {
        if self.state == EnvelopeState::Idle
            || self.state == EnvelopeState::Delay
            || self.release_forced_count == 0
        {
            self.mult_hires = 0;
            self.count = self.delay_count;
            if self.count > 0 {
                self.state = EnvelopeState::Delay;
                self.inc_hires = 0;
            } else {
                self.state = EnvelopeState::Attack;
                self.count = self.attack_count;
                self.inc_hires = UNITY_GAIN / self.count as i32;
            }
        } else if self.state != EnvelopeState::Forced {
            self.state = EnvelopeState::Forced;
            self.count = self.release_forced_count;
            self.inc_hires = (-self.mult_hires) / self.count as i32;
        }
    }

    /// Release the envelope (start the release phase).
    pub fn note_off(&mut self) {
        if self.state != EnvelopeState::Release
            && self.state != EnvelopeState::Idle
            && self.state != EnvelopeState::Forced
        {
            self.state = EnvelopeState::Release;
            self.count = self.release_count;
            self.inc_hires = (-self.mult_hires) / self.count as i32;
        }
    }

    /// Check if the envelope is currently active (not idle).
    pub fn is_active(&self) -> bool {
        self.state != EnvelopeState::Idle
    }

    /// Check if the envelope is in the sustain phase.
    pub fn is_sustain(&self) -> bool {
        self.state == EnvelopeState::Sustain
    }

    /// Get the current envelope state.
    pub fn state(&self) -> EnvelopeState {
        self.state
    }
}

impl AudioNode for AudioEffectEnvelope {
    const NUM_INPUTS: usize = 1;
    const NUM_OUTPUTS: usize = 1;

    fn update(
        &mut self,
        inputs: &[Option<AudioBlockRef>],
        outputs: &mut [Option<AudioBlockMut>],
    ) {
        let has_input = inputs[0].is_some();

        if self.state == EnvelopeState::Idle {
            // Idle: no output
            return;
        }

        let mut out = if has_input {
            match outputs[0].take() {
                Some(b) => Some(b),
                None => None,
            }
        } else {
            None
        };

        // Process 128 samples in groups of 8 (16 groups total)
        let num_groups = AUDIO_BLOCK_SAMPLES / SAMPLES_PER_GROUP as usize;
        let mut sample_idx = 0usize;

        for _ in 0..num_groups {
            // State transition when count reaches 0
            if self.count == 0 {
                match self.state {
                    EnvelopeState::Attack => {
                        if self.hold_count > 0 {
                            self.state = EnvelopeState::Hold;
                            self.count = self.hold_count;
                            self.mult_hires = UNITY_GAIN;
                            self.inc_hires = 0;
                        } else {
                            self.state = EnvelopeState::Decay;
                            self.count = self.decay_count;
                            self.inc_hires =
                                (self.sustain_mult - UNITY_GAIN) / self.count as i32;
                        }
                    }
                    EnvelopeState::Hold => {
                        self.state = EnvelopeState::Decay;
                        self.count = self.decay_count;
                        self.inc_hires =
                            (self.sustain_mult - UNITY_GAIN) / self.count as i32;
                    }
                    EnvelopeState::Decay => {
                        self.state = EnvelopeState::Sustain;
                        self.count = 0xFFFF;
                        self.mult_hires = self.sustain_mult;
                        self.inc_hires = 0;
                    }
                    EnvelopeState::Sustain => {
                        self.count = 0xFFFF;
                    }
                    EnvelopeState::Release => {
                        self.state = EnvelopeState::Idle;
                        // Zero remaining output
                        if let Some(ref mut out_block) = out {
                            if let Some(ref input) = inputs[0] {
                                let _ = input; // consume reference
                            }
                            for j in sample_idx..AUDIO_BLOCK_SAMPLES {
                                out_block[j] = 0;
                            }
                        }
                        // Early return handled by break
                        outputs[0] = out;
                        return;
                    }
                    EnvelopeState::Forced => {
                        self.mult_hires = 0;
                        self.count = self.delay_count;
                        if self.count > 0 {
                            self.state = EnvelopeState::Delay;
                            self.inc_hires = 0;
                        } else {
                            self.state = EnvelopeState::Attack;
                            self.count = self.attack_count;
                            self.inc_hires = UNITY_GAIN / self.count as i32;
                        }
                    }
                    EnvelopeState::Delay => {
                        self.state = EnvelopeState::Attack;
                        self.count = self.attack_count;
                        self.inc_hires = UNITY_GAIN / self.count as i32;
                    }
                    EnvelopeState::Idle => {}
                }
            }

            // Process 8 samples with linearly interpolated gain
            if let (Some(ref mut out_block), Some(ref input)) = (&mut out, &inputs[0]) {
                // Downshift to 16-bit resolution for per-sample multiply
                let mut mult = self.mult_hires >> 14;
                let inc = self.inc_hires >> 17;

                for j in 0..SAMPLES_PER_GROUP as usize {
                    mult += inc;
                    let sample = input[sample_idx + j] as i32;
                    let result = (sample * mult) >> 16;
                    out_block[sample_idx + j] = saturate16(result);
                }
            }

            sample_idx += SAMPLES_PER_GROUP as usize;
            self.mult_hires += self.inc_hires;
            self.count = self.count.saturating_sub(1);
        }

        outputs[0] = out;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::pool::POOL;

    fn reset_pool() {
        POOL.reset();
    }

    fn alloc_block_with_value(value: i16) -> AudioBlockMut {
        let mut block = AudioBlockMut::alloc().unwrap();
        block.fill(value);
        block
    }

    #[test]
    fn envelope_default_state_idle() {
        let env = AudioEffectEnvelope::new();
        assert_eq!(env.state(), EnvelopeState::Idle);
        assert!(!env.is_active());
    }

    #[test]
    fn envelope_note_on_starts_attack() {
        let mut env = AudioEffectEnvelope::new();
        env.note_on();
        assert!(env.is_active());
        assert!(
            env.state() == EnvelopeState::Attack || env.state() == EnvelopeState::Delay,
            "state should be Attack or Delay, got {:?}",
            env.state()
        );
    }

    #[test]
    fn envelope_idle_produces_no_output() {
        reset_pool();
        let mut env = AudioEffectEnvelope::new();

        let input = alloc_block_with_value(32767);
        let output = AudioBlockMut::alloc().unwrap();

        let input_ref = input.into_shared();
        let mut outputs = [Some(output)];
        let inputs = [Some(input_ref)];

        env.update(&inputs, &mut outputs);

        // Idle state: output block is not consumed (still Some but untouched)
        assert!(outputs[0].is_some());
        assert_eq!(env.state(), EnvelopeState::Idle);
    }

    #[test]
    fn envelope_attack_ramps_up() {
        reset_pool();
        let mut env = AudioEffectEnvelope::new();
        env.delay(0.0);
        env.attack(50.0); // 50ms attack
        env.hold(0.0);
        env.sustain(1.0);
        env.note_on();

        let input = alloc_block_with_value(32767);
        let output = AudioBlockMut::alloc().unwrap();

        let input_ref = input.into_shared();
        let mut outputs = [Some(output)];
        let inputs = [Some(input_ref)];

        env.update(&inputs, &mut outputs);

        let out = outputs[0].as_ref().unwrap();
        // During attack, should be ramping up from 0
        // First samples should be quieter than last
        let first_group_avg: i32 =
            out[0..8].iter().map(|&s| s as i32).sum::<i32>() / 8;
        let last_group_avg: i32 =
            out[120..128].iter().map(|&s| s as i32).sum::<i32>() / 8;
        assert!(
            last_group_avg > first_group_avg,
            "attack should ramp up: first_avg={}, last_avg={}",
            first_group_avg, last_group_avg
        );
    }

    #[test]
    fn envelope_note_off_triggers_release() {
        let mut env = AudioEffectEnvelope::new();
        env.note_on();
        env.note_off();
        assert_eq!(env.state(), EnvelopeState::Release);
    }

    #[test]
    fn envelope_reaches_sustain() {
        reset_pool();
        let mut env = AudioEffectEnvelope::new();
        env.delay(0.0);
        env.attack(1.0); // very fast
        env.hold(0.0);
        env.decay(1.0); // very fast
        env.sustain(0.5);
        env.release(300.0);
        env.note_on();

        // Process enough blocks to get through attack and decay
        for _ in 0..15 {
            let input = alloc_block_with_value(32767);
            let output = AudioBlockMut::alloc().unwrap();
            let input_ref = input.into_shared();
            let mut outputs = [Some(output)];
            let inputs = [Some(input_ref)];
            env.update(&inputs, &mut outputs);
            // outputs dropped here, releasing blocks back to pool
        }

        assert_eq!(env.state(), EnvelopeState::Sustain);
    }

    #[test]
    fn envelope_milliseconds2count() {
        // 10.5ms at ~44117 Hz: 10.5 * 44.117647 = 463.23 samples
        // (463 + 7) / 8 = 58 groups
        let count = AudioEffectEnvelope::milliseconds2count(10.5);
        assert!(count >= 57 && count <= 59, "expected ~58, got {}", count);
    }

    #[test]
    fn envelope_is_sustain() {
        let mut env = AudioEffectEnvelope::new();
        assert!(!env.is_sustain());
        // Manually set state for testing
        env.state = EnvelopeState::Sustain;
        assert!(env.is_sustain());
    }

    #[test]
    fn envelope_retrigger_forced() {
        let mut env = AudioEffectEnvelope::new();
        env.sustain(0.5);
        env.note_on();

        // Move to sustain manually for simplicity
        env.state = EnvelopeState::Sustain;
        env.mult_hires = UNITY_GAIN / 2;

        // Re-trigger while sustaining
        env.note_on();
        assert_eq!(env.state(), EnvelopeState::Forced);
    }
}
