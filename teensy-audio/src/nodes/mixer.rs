//! N-channel audio mixer with per-channel gain.
//!
//! Port of `TeensyAudio/mixer.h` / `mixer.cpp` (`AudioMixer4`).
//! Uses const generic `N` instead of the C++ hardcoded 4 channels.

use crate::block::{AudioBlockMut, AudioBlockRef};
use crate::constants::AUDIO_BLOCK_SAMPLES;
use crate::dsp::intrinsics::saturate16;
use crate::node::AudioNode;

/// Fixed-point unity gain: 1.0 in Q16.16 format = 65536.
const MULTI_UNITYGAIN: i32 = 65536;

/// N-channel mixer. Mixes N input channels into a single mono output with per-channel gain.
///
/// `AudioMixer<4>` matches the C++ `AudioMixer4`, but any count is supported.
///
/// # Example
/// ```ignore
/// let mut mixer = AudioMixer::<4>::new();
/// mixer.gain(0, 1.0);  // channel 0 at unity
/// mixer.gain(1, 0.5);  // channel 1 at half volume
/// ```
pub struct AudioMixer<const N: usize> {
    /// Per-channel gain in Q16.16 fixed-point. 65536 = unity (1.0).
    multiplier: [i32; N],
}

impl<const N: usize> AudioMixer<N> {
    /// Create a new mixer with all channels at unity gain.
    pub const fn new() -> Self {
        AudioMixer {
            multiplier: [MULTI_UNITYGAIN; N],
        }
    }

    /// Set the gain for a specific channel.
    ///
    /// `level` is a floating-point gain: 0.0 = silence, 1.0 = unity, >1.0 = boost.
    /// Clamped to ±32767.0 (matching C++ behavior).
    pub fn gain(&mut self, channel: usize, level: f32) {
        if channel >= N {
            return;
        }
        let clamped = if level > 32767.0 {
            32767.0
        } else if level < -32767.0 {
            -32767.0
        } else {
            level
        };
        self.multiplier[channel] = (clamped * 65536.0) as i32;
    }
}

/// Apply gain to a block in-place: `data[i] = saturate16((data[i] * mult) >> 16)`.
fn apply_gain(data: &mut [i16; AUDIO_BLOCK_SAMPLES], mult: i32) {
    for sample in data.iter_mut() {
        let val = ((*sample as i64) * (mult as i64)) >> 16;
        *sample = saturate16(val as i32);
    }
}

/// Apply gain to `src` and saturating-add into `dst`.
fn apply_gain_then_add(
    dst: &mut [i16; AUDIO_BLOCK_SAMPLES],
    src: &[i16; AUDIO_BLOCK_SAMPLES],
    mult: i32,
) {
    if mult == MULTI_UNITYGAIN {
        // Fast path: just saturating-add
        for (d, &s) in dst.iter_mut().zip(src.iter()) {
            *d = saturate16(*d as i32 + s as i32);
        }
    } else {
        for (d, &s) in dst.iter_mut().zip(src.iter()) {
            let gained = ((s as i64) * (mult as i64)) >> 16;
            let gained_sat = saturate16(gained as i32);
            *d = saturate16(*d as i32 + gained_sat as i32);
        }
    }
}

impl<const N: usize> AudioNode for AudioMixer<N> {
    const NUM_INPUTS: usize = N;
    const NUM_OUTPUTS: usize = 1;

    fn update(
        &mut self,
        inputs: &[Option<AudioBlockRef>],
        outputs: &mut [Option<AudioBlockMut>],
    ) {
        let out_block = match outputs[0].take() {
            Some(b) => b,
            None => return,
        };

        let mut out = out_block;
        let mut initialized = false;

        for ch in 0..N {
            if let Some(ref input) = inputs[ch] {
                let mult = self.multiplier[ch];
                if !initialized {
                    // First active channel: copy (with gain) into output buffer
                    out.copy_from_slice(&input[..]);
                    if mult != MULTI_UNITYGAIN {
                        apply_gain(&mut out, mult);
                    }
                    initialized = true;
                } else {
                    // Subsequent channels: gain + accumulate
                    apply_gain_then_add(&mut out, input, mult);
                }
            }
        }

        if !initialized {
            // No active inputs: output silence
            out.fill(0);
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
    fn mixer_unity_gain_single_channel() {
        reset_pool();
        let mut mixer = AudioMixer::<2>::new();

        let input = alloc_block_with(&[1000, -2000, 32767, -32768]);
        let output = AudioBlockMut::alloc().unwrap();

        let input_ref = input.into_shared();
        let mut outputs = [Some(output)];
        let inputs = [Some(input_ref), None];

        mixer.update(&inputs, &mut outputs);

        let out = outputs[0].as_ref().unwrap();
        assert_eq!(out[0], 1000);
        assert_eq!(out[1], -2000);
        assert_eq!(out[2], 32767);
        assert_eq!(out[3], -32768);
    }

    #[test]
    fn mixer_half_gain() {
        reset_pool();
        let mut mixer = AudioMixer::<1>::new();
        mixer.gain(0, 0.5);

        let input = alloc_block_with(&[10000, -10000, 32767]);
        let output = AudioBlockMut::alloc().unwrap();

        let input_ref = input.into_shared();
        let mut outputs = [Some(output)];
        let inputs = [Some(input_ref)];

        mixer.update(&inputs, &mut outputs);

        let out = outputs[0].as_ref().unwrap();
        // 10000 * 32768 / 65536 = 5000
        assert!((out[0] - 5000).abs() <= 1);
        assert!((out[1] - (-5000)).abs() <= 1);
    }

    #[test]
    fn mixer_two_channels_sum() {
        reset_pool();
        let mut mixer = AudioMixer::<2>::new();

        let input0 = alloc_block_with(&[1000, 2000]);
        let input1 = alloc_block_with(&[3000, 4000]);
        let output = AudioBlockMut::alloc().unwrap();

        let ref0 = input0.into_shared();
        let ref1 = input1.into_shared();
        let mut outputs = [Some(output)];
        let inputs = [Some(ref0), Some(ref1)];

        mixer.update(&inputs, &mut outputs);

        let out = outputs[0].as_ref().unwrap();
        assert_eq!(out[0], 4000);
        assert_eq!(out[1], 6000);
    }

    #[test]
    fn mixer_saturation() {
        reset_pool();
        let mut mixer = AudioMixer::<2>::new();

        let input0 = alloc_block_with(&[30000]);
        let input1 = alloc_block_with(&[30000]);
        let output = AudioBlockMut::alloc().unwrap();

        let ref0 = input0.into_shared();
        let ref1 = input1.into_shared();
        let mut outputs = [Some(output)];
        let inputs = [Some(ref0), Some(ref1)];

        mixer.update(&inputs, &mut outputs);

        let out = outputs[0].as_ref().unwrap();
        assert_eq!(out[0], 32767); // saturated
    }

    #[test]
    fn mixer_no_inputs_produces_silence() {
        reset_pool();
        let mut mixer = AudioMixer::<2>::new();
        let output = AudioBlockMut::alloc().unwrap();

        let mut outputs = [Some(output)];
        let inputs: [Option<AudioBlockRef>; 2] = [None, None];

        mixer.update(&inputs, &mut outputs);

        let out = outputs[0].as_ref().unwrap();
        for &s in out.iter() {
            assert_eq!(s, 0);
        }
    }

    #[test]
    fn mixer_gain_out_of_range_ignored() {
        let mut mixer = AudioMixer::<2>::new();
        mixer.gain(5, 1.0); // out of range, should not panic
    }

    #[test]
    fn mixer_const_generic_8() {
        reset_pool();
        let mut mixer = AudioMixer::<8>::new();
        mixer.gain(7, 0.5);

        let input = alloc_block_with(&[20000]);
        let output = AudioBlockMut::alloc().unwrap();

        let input_ref = input.into_shared();
        let mut outputs = [Some(output)];
        let inputs: [Option<AudioBlockRef>; 8] = [
            None, None, None, None, None, None, None, Some(input_ref),
        ];

        mixer.update(&inputs, &mut outputs);

        let out = outputs[0].as_ref().unwrap();
        assert!((out[0] - 10000).abs() <= 1);
    }
}
