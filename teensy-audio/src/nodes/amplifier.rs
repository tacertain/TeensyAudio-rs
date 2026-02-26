//! Single-channel amplifier (volume control).
//!
//! Port of `AudioAmplifier` from `TeensyAudio/mixer.h` / `mixer.cpp`.

use crate::block::{AudioBlockMut, AudioBlockRef};
use crate::constants::AUDIO_BLOCK_SAMPLES;
use crate::dsp::intrinsics::saturate16;
use crate::node::AudioNode;

/// Fixed-point unity gain: 1.0 in Q16.16 format.
const MULTI_UNITYGAIN: i32 = 65536;

/// Single-channel amplifier. One input, one output.
///
/// # Example
/// ```ignore
/// let mut amp = AudioAmplifier::new();
/// amp.gain(0.75); // 75% volume
/// ```
pub struct AudioAmplifier {
    /// Gain in Q16.16 fixed-point. 65536 = unity (1.0).
    multiplier: i32,
}

impl AudioAmplifier {
    /// Create a new amplifier at unity gain.
    pub const fn new() -> Self {
        AudioAmplifier {
            multiplier: MULTI_UNITYGAIN,
        }
    }

    /// Set amplification level.
    ///
    /// 0.0 = silence, 1.0 = unity, >1.0 = boost. Clamped to ±32767.0.
    pub fn gain(&mut self, level: f32) {
        let clamped = if level > 32767.0 {
            32767.0
        } else if level < -32767.0 {
            -32767.0
        } else {
            level
        };
        self.multiplier = (clamped * 65536.0) as i32;
    }
}

impl AudioNode for AudioAmplifier {
    const NUM_INPUTS: usize = 1;
    const NUM_OUTPUTS: usize = 1;

    fn update(
        &mut self,
        inputs: &[Option<AudioBlockRef>],
        outputs: &mut [Option<AudioBlockMut>],
    ) {
        let input = match inputs[0] {
            Some(ref b) => b,
            None => return, // No input, leave output as None (silence)
        };

        let mult = self.multiplier;

        let mut out = match outputs[0].take() {
            Some(b) => b,
            None => return,
        };

        if mult == 0 {
            // Zero gain: discard input and output block (silence)
            drop(out);
            return;
        }

        if mult == MULTI_UNITYGAIN {
            // Unity gain: pass through (copy)
            out.copy_from_slice(&input[..]);
        } else {
            // Apply gain: Q16.16 multiply with saturation
            for i in 0..AUDIO_BLOCK_SAMPLES {
                let val = ((input[i] as i64) * (mult as i64)) >> 16;
                out[i] = saturate16(val as i32);
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

    fn alloc_block_with(values: &[i16]) -> AudioBlockMut {
        let mut block = AudioBlockMut::alloc().unwrap();
        block.fill(0);
        for (i, &v) in values.iter().enumerate() {
            if i < AUDIO_BLOCK_SAMPLES {
                block[i] = v;
            }
        }
        block
    }

    #[test]
    fn amplifier_unity_gain() {
        reset_pool();
        let mut amp = AudioAmplifier::new();

        let input = alloc_block_with(&[1000, -2000, 32767, -32768]);
        let output = AudioBlockMut::alloc().unwrap();

        let input_ref = input.into_shared();
        let mut outputs = [Some(output)];
        let inputs = [Some(input_ref)];

        amp.update(&inputs, &mut outputs);

        let out = outputs[0].as_ref().unwrap();
        assert_eq!(out[0], 1000);
        assert_eq!(out[1], -2000);
        assert_eq!(out[2], 32767);
        assert_eq!(out[3], -32768);
    }

    #[test]
    fn amplifier_half_gain() {
        reset_pool();
        let mut amp = AudioAmplifier::new();
        amp.gain(0.5);

        let input = alloc_block_with(&[10000, -10000]);
        let output = AudioBlockMut::alloc().unwrap();

        let input_ref = input.into_shared();
        let mut outputs = [Some(output)];
        let inputs = [Some(input_ref)];

        amp.update(&inputs, &mut outputs);

        let out = outputs[0].as_ref().unwrap();
        assert!((out[0] - 5000).abs() <= 1);
        assert!((out[1] - (-5000)).abs() <= 1);
    }

    #[test]
    fn amplifier_zero_gain_produces_no_output() {
        reset_pool();
        let mut amp = AudioAmplifier::new();
        amp.gain(0.0);

        let input = alloc_block_with(&[1000, 2000]);
        let output = AudioBlockMut::alloc().unwrap();

        let input_ref = input.into_shared();
        let mut outputs = [Some(output)];
        let inputs = [Some(input_ref)];

        amp.update(&inputs, &mut outputs);

        // Zero gain discards input, output block is not returned
        assert!(outputs[0].is_none());
    }

    #[test]
    fn amplifier_boost() {
        reset_pool();
        let mut amp = AudioAmplifier::new();
        amp.gain(2.0);

        let input = alloc_block_with(&[10000, -10000]);
        let output = AudioBlockMut::alloc().unwrap();

        let input_ref = input.into_shared();
        let mut outputs = [Some(output)];
        let inputs = [Some(input_ref)];

        amp.update(&inputs, &mut outputs);

        let out = outputs[0].as_ref().unwrap();
        assert!((out[0] - 20000).abs() <= 1);
        assert!((out[1] - (-20000)).abs() <= 1);
    }

    #[test]
    fn amplifier_saturation() {
        reset_pool();
        let mut amp = AudioAmplifier::new();
        amp.gain(2.0);

        let input = alloc_block_with(&[20000]);
        let output = AudioBlockMut::alloc().unwrap();

        let input_ref = input.into_shared();
        let mut outputs = [Some(output)];
        let inputs = [Some(input_ref)];

        amp.update(&inputs, &mut outputs);

        let out = outputs[0].as_ref().unwrap();
        assert_eq!(out[0], 32767); // saturated
    }

    #[test]
    fn amplifier_no_input() {
        reset_pool();
        let mut amp = AudioAmplifier::new();
        let output = AudioBlockMut::alloc().unwrap();

        let mut outputs = [Some(output)];
        let inputs: [Option<AudioBlockRef>; 1] = [None];

        amp.update(&inputs, &mut outputs);

        // No input: output block left untouched (taken back by caller)
        // The amplifier returns early, so output should still be Some
        assert!(outputs[0].is_some());
    }
}
