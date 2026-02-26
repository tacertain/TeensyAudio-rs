//! Sine wave oscillator using phase accumulator and wavetable lookup.
//!
//! Port of `TeensyAudio/synth_sine.cpp`. Uses a 257-entry sine wavetable
//! with linear interpolation between adjacent entries.

use crate::block::{AudioBlockMut, AudioBlockRef};
use crate::constants::{AUDIO_BLOCK_SAMPLES, AUDIO_SAMPLE_RATE_EXACT};
use crate::dsp::intrinsics::mul_32x32_rshift32;
use crate::dsp::wavetables::SINE_TABLE;
use crate::node::AudioNode;

/// Sine wave oscillator.
///
/// Generates a sine wave using a phase accumulator with wavetable lookup
/// and linear interpolation. Source node: 0 inputs, 1 output.
///
/// # Example
/// ```ignore
/// let mut sine = AudioSynthSine::new();
/// sine.frequency(440.0);
/// sine.amplitude(0.8);
/// ```
pub struct AudioSynthSine {
    /// Phase accumulator (wraps naturally at 32 bits = 360°).
    phase_accumulator: u32,
    /// Phase increment per sample: `freq / SAMPLE_RATE * 2^32`.
    phase_increment: u32,
    /// Output magnitude in Q16.16 format. 0 = silent, 65536 = full scale.
    magnitude: i32,
}

impl AudioSynthSine {
    /// Create a new sine oscillator, initially silent (magnitude = 0).
    pub const fn new() -> Self {
        AudioSynthSine {
            phase_accumulator: 0,
            phase_increment: 0,
            magnitude: 0,
        }
    }

    /// Set the oscillator frequency in Hz.
    ///
    /// Phase increment is computed as `freq / AUDIO_SAMPLE_RATE_EXACT * 2^32`.
    pub fn frequency(&mut self, hz: f32) {
        let inc = hz * (4_294_967_296.0 / AUDIO_SAMPLE_RATE_EXACT);
        self.phase_increment = inc as u32;
    }

    /// Set the output amplitude (0.0 = silent, 1.0 = full scale).
    ///
    /// The magnitude is stored as Q16.16: `level * 65536`.
    pub fn amplitude(&mut self, level: f32) {
        let clamped = if level < 0.0 { 0.0 } else if level > 1.0 { 1.0 } else { level };
        self.magnitude = (clamped * 65536.0) as i32;
    }

    /// Set the phase offset in degrees (0–360).
    pub fn phase(&mut self, angle: f32) {
        self.phase_accumulator = (angle * (4_294_967_296.0 / 360.0)) as u32;
    }
}

impl AudioNode for AudioSynthSine {
    const NUM_INPUTS: usize = 0;
    const NUM_OUTPUTS: usize = 1;

    fn update(
        &mut self,
        _inputs: &[Option<AudioBlockRef>],
        outputs: &mut [Option<AudioBlockMut>],
    ) {
        if self.magnitude == 0 {
            // Silent: advance phase but produce no output
            self.phase_accumulator = self.phase_accumulator
                .wrapping_add(self.phase_increment.wrapping_mul(AUDIO_BLOCK_SAMPLES as u32));
            return;
        }

        let mut out = match outputs[0].take() {
            Some(b) => b,
            None => {
                self.phase_accumulator = self.phase_accumulator
                    .wrapping_add(self.phase_increment.wrapping_mul(AUDIO_BLOCK_SAMPLES as u32));
                return;
            }
        };

        let mut ph = self.phase_accumulator;
        let inc = self.phase_increment;
        let mag = self.magnitude;

        for i in 0..AUDIO_BLOCK_SAMPLES {
            // Upper 8 bits = table index (0–255)
            let index = (ph >> 24) as usize;
            let val1 = SINE_TABLE[index] as i32;
            let val2 = SINE_TABLE[index + 1] as i32;

            // Fractional part from bits 8–23 (16-bit interpolation weight)
            let scale = ((ph >> 8) & 0xFFFF) as i32;
            let interpolated = val1 * (0x10000 - scale) + val2 * scale;

            // `interpolated` is in Q16 format. `mul_32x32_rshift32` scales by magnitude
            // and shifts down 32 bits, producing a Q15 result when magnitude is Q16.16.
            out[i] = mul_32x32_rshift32(interpolated, mag) as i16;

            ph = ph.wrapping_add(inc);
        }

        self.phase_accumulator = ph;
        outputs[0] = Some(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::pool::POOL;

    fn reset_pool() {
        POOL.reset();
    }

    #[test]
    fn sine_silent_when_no_amplitude() {
        reset_pool();
        let mut sine = AudioSynthSine::new();
        sine.frequency(440.0);
        // amplitude defaults to 0

        let output = AudioBlockMut::alloc().unwrap();
        let mut outputs = [Some(output)];
        let inputs: [Option<AudioBlockRef>; 0] = [];

        sine.update(&inputs, &mut outputs);

        // No output produced (magnitude == 0, returns early, output block untouched)
        assert!(outputs[0].is_some());
    }

    #[test]
    fn sine_produces_output() {
        reset_pool();
        let mut sine = AudioSynthSine::new();
        sine.frequency(440.0);
        sine.amplitude(1.0);

        let output = AudioBlockMut::alloc().unwrap();
        let mut outputs = [Some(output)];
        let inputs: [Option<AudioBlockRef>; 0] = [];

        sine.update(&inputs, &mut outputs);

        let out = outputs[0].as_ref().unwrap();
        // First sample at phase 0 should be approximately 0
        assert!(out[0].abs() < 500, "first sample should be near zero, got {}", out[0]);

        // Check that the block contains non-zero values (it's a 440Hz sine)
        let max = out.iter().map(|s| s.abs()).max().unwrap();
        assert!(max > 10000, "sine should have significant amplitude, max={}", max);
    }

    #[test]
    fn sine_phase_accumulates() {
        reset_pool();
        let mut sine = AudioSynthSine::new();
        sine.frequency(440.0);
        sine.amplitude(1.0);

        let output1 = AudioBlockMut::alloc().unwrap();
        let mut outputs = [Some(output1)];
        let inputs: [Option<AudioBlockRef>; 0] = [];

        sine.update(&inputs, &mut outputs);
        let phase_after_1 = sine.phase_accumulator;

        let output2 = AudioBlockMut::alloc().unwrap();
        outputs[0] = Some(output2);
        sine.update(&inputs, &mut outputs);
        let phase_after_2 = sine.phase_accumulator;

        // Phase should continue advancing
        assert_ne!(phase_after_1, 0);
        assert_ne!(phase_after_2, phase_after_1);
    }

    #[test]
    fn sine_half_amplitude() {
        reset_pool();
        let mut sine_full = AudioSynthSine::new();
        sine_full.frequency(1000.0);
        sine_full.amplitude(1.0);

        let mut sine_half = AudioSynthSine::new();
        sine_half.frequency(1000.0);
        sine_half.amplitude(0.5);

        let out_full = AudioBlockMut::alloc().unwrap();
        let out_half = AudioBlockMut::alloc().unwrap();

        let mut outputs_full = [Some(out_full)];
        let mut outputs_half = [Some(out_half)];
        let inputs: [Option<AudioBlockRef>; 0] = [];

        sine_full.update(&inputs, &mut outputs_full);
        sine_half.update(&inputs, &mut outputs_half);

        let full = outputs_full[0].as_ref().unwrap();
        let half = outputs_half[0].as_ref().unwrap();

        // For samples with significant amplitude, half should be ~50% of full
        for i in 10..30 {
            if full[i].abs() > 1000 {
                let ratio = half[i] as f32 / full[i] as f32;
                assert!(
                    (ratio - 0.5).abs() < 0.1,
                    "sample {}: full={}, half={}, ratio={}",
                    i, full[i], half[i], ratio
                );
            }
        }
    }

    #[test]
    fn sine_frequency_zero_is_dc() {
        reset_pool();
        let mut sine = AudioSynthSine::new();
        sine.frequency(0.0);
        sine.amplitude(1.0);

        let output = AudioBlockMut::alloc().unwrap();
        let mut outputs = [Some(output)];
        let inputs: [Option<AudioBlockRef>; 0] = [];

        sine.update(&inputs, &mut outputs);

        let out = outputs[0].as_ref().unwrap();
        // At 0 Hz, phase never advances, all samples should be the same (near 0 for sine at phase 0)
        let first = out[0];
        for i in 1..AUDIO_BLOCK_SAMPLES {
            assert_eq!(out[i], first);
        }
    }
}
