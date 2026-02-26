//! DC level source — fills output with a constant value.
//!
//! Port of `TeensyAudio/synth_dc.cpp`. Supports immediate amplitude changes
//! and smooth ramping over a specified duration.

use crate::block::{AudioBlockMut, AudioBlockRef};
use crate::constants::{AUDIO_BLOCK_SAMPLES, AUDIO_SAMPLE_RATE_EXACT};
use crate::node::AudioNode;

/// DC level source. Outputs a constant value every block.
///
/// Source node: 0 inputs, 1 output.
///
/// # Example
/// ```ignore
/// let mut dc = AudioSynthWaveformDc::new();
/// dc.amplitude(0.5);  // 50% positive DC
/// ```
pub struct AudioSynthWaveformDc {
    /// Current magnitude as Q16.16 (upper 16 bits are the i16 sample value).
    magnitude: i32,
    /// Target magnitude for ramping.
    target: i32,
    /// Increment per sample for ramping.
    increment: i32,
    /// true = currently ramping toward `target`.
    transitioning: bool,
}

impl AudioSynthWaveformDc {
    /// Create a new DC source at zero output.
    pub const fn new() -> Self {
        AudioSynthWaveformDc {
            magnitude: 0,
            target: 0,
            increment: 0,
            transitioning: false,
        }
    }

    /// Set DC level immediately (-1.0 to 1.0).
    pub fn amplitude(&mut self, level: f32) {
        let clamped = if level > 1.0 {
            1.0
        } else if level < -1.0 {
            -1.0
        } else {
            level
        };
        // Scale to match C++ behavior: magnitude uses upper 16 bits as sample value
        // C++ uses 2147418112.0 ≈ 0x7FFF0000 for 1.0
        self.magnitude = (clamped * 2_147_418_112.0) as i32;
        self.transitioning = false;
    }

    /// Set DC level with a smooth ramp over the specified duration.
    pub fn amplitude_ramp(&mut self, level: f32, milliseconds: f32) {
        let clamped = if level > 1.0 {
            1.0
        } else if level < -1.0 {
            -1.0
        } else {
            level
        };
        let new_target = (clamped * 2_147_418_112.0) as i32;

        if milliseconds <= 0.0 {
            self.magnitude = new_target;
            self.transitioning = false;
            return;
        }

        let samples = (milliseconds * AUDIO_SAMPLE_RATE_EXACT / 1000.0) as i32;
        if samples <= 0 {
            self.magnitude = new_target;
            self.transitioning = false;
            return;
        }

        self.target = new_target;
        let diff = (new_target as i64) - (self.magnitude as i64);
        self.increment = (diff / samples as i64) as i32;
        if self.increment == 0 {
            // Difference is too small for the given duration; snap to target
            self.magnitude = new_target;
            self.transitioning = false;
        } else {
            self.transitioning = true;
        }
    }
}

/// Extract the upper 16 bits of a Q16.16 value as an i16 sample.
#[inline(always)]
fn magnitude_to_sample(mag: i32) -> i16 {
    (mag >> 16) as i16
}

impl AudioNode for AudioSynthWaveformDc {
    const NUM_INPUTS: usize = 0;
    const NUM_OUTPUTS: usize = 1;

    fn update(
        &mut self,
        _inputs: &[Option<AudioBlockRef>],
        outputs: &mut [Option<AudioBlockMut>],
    ) {
        let mut out = match outputs[0].take() {
            Some(b) => b,
            None => return,
        };

        if !self.transitioning {
            // Steady: fill with constant value
            let sample = magnitude_to_sample(self.magnitude);
            out.fill(sample);
        } else {
            // Ramping toward target
            for i in 0..AUDIO_BLOCK_SAMPLES {
                self.magnitude = self.magnitude.wrapping_add(self.increment);

                // Check if we've reached or passed the target
                if (self.increment > 0 && self.magnitude >= self.target)
                    || (self.increment < 0 && self.magnitude <= self.target)
                {
                    self.magnitude = self.target;
                    self.transitioning = false;
                    // Fill remainder with target value
                    let sample = magnitude_to_sample(self.magnitude);
                    for j in i..AUDIO_BLOCK_SAMPLES {
                        out[j] = sample;
                    }
                    break;
                }

                out[i] = magnitude_to_sample(self.magnitude);
            }
        }

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
    fn dc_zero_output() {
        reset_pool();
        let mut dc = AudioSynthWaveformDc::new();

        let output = AudioBlockMut::alloc().unwrap();
        let mut outputs = [Some(output)];
        let inputs: [Option<AudioBlockRef>; 0] = [];

        dc.update(&inputs, &mut outputs);

        let out = outputs[0].as_ref().unwrap();
        for &s in out.iter() {
            assert_eq!(s, 0);
        }
    }

    #[test]
    fn dc_positive_level() {
        reset_pool();
        let mut dc = AudioSynthWaveformDc::new();
        dc.amplitude(1.0);

        let output = AudioBlockMut::alloc().unwrap();
        let mut outputs = [Some(output)];
        let inputs: [Option<AudioBlockRef>; 0] = [];

        dc.update(&inputs, &mut outputs);

        let out = outputs[0].as_ref().unwrap();
        // 1.0 should produce approximately 32767
        assert!(out[0] >= 32766, "expected ~32767, got {}", out[0]);
        // All samples should be the same
        for &s in out.iter() {
            assert_eq!(s, out[0]);
        }
    }

    #[test]
    fn dc_negative_level() {
        reset_pool();
        let mut dc = AudioSynthWaveformDc::new();
        dc.amplitude(-1.0);

        let output = AudioBlockMut::alloc().unwrap();
        let mut outputs = [Some(output)];
        let inputs: [Option<AudioBlockRef>; 0] = [];

        dc.update(&inputs, &mut outputs);

        let out = outputs[0].as_ref().unwrap();
        assert!(out[0] <= -32766, "expected ~-32767, got {}", out[0]);
    }

    #[test]
    fn dc_half_level() {
        reset_pool();
        let mut dc = AudioSynthWaveformDc::new();
        dc.amplitude(0.5);

        let output = AudioBlockMut::alloc().unwrap();
        let mut outputs = [Some(output)];
        let inputs: [Option<AudioBlockRef>; 0] = [];

        dc.update(&inputs, &mut outputs);

        let out = outputs[0].as_ref().unwrap();
        // 0.5 * 32767 ≈ 16383
        assert!((out[0] - 16383).abs() <= 1, "expected ~16383, got {}", out[0]);
    }

    #[test]
    fn dc_ramp() {
        reset_pool();
        let mut dc = AudioSynthWaveformDc::new();
        dc.amplitude(0.0);
        // Ramp to 1.0 over ~100ms. In 100ms at 44117Hz, that's ~4411 samples (~34 blocks).
        dc.amplitude_ramp(1.0, 100.0);

        let output = AudioBlockMut::alloc().unwrap();
        let mut outputs = [Some(output)];
        let inputs: [Option<AudioBlockRef>; 0] = [];

        dc.update(&inputs, &mut outputs);

        let out = outputs[0].as_ref().unwrap();
        // First sample should be near zero (just started ramping)
        assert!(out[0].abs() < 2000, "first sample should be small, got {}", out[0]);
        // Last sample should be larger than first (ramping up)
        assert!(out[127] > out[0], "last sample should be > first");
        // Should be monotonically non-decreasing
        for i in 1..AUDIO_BLOCK_SAMPLES {
            assert!(out[i] >= out[i - 1], "not monotonic at {}: {} < {}", i, out[i], out[i - 1]);
        }
    }
}
