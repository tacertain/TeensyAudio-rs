//! Volume fade effect using the fader wavetable for perceptual smoothness.
//!
//! Port of `TeensyAudio/effect_fade.cpp`. Uses the 257-entry fader table
//! with linear interpolation to provide a perceptually smooth fade curve.

use crate::block::{AudioBlockMut, AudioBlockRef};
use crate::constants::{AUDIO_BLOCK_SAMPLES, AUDIO_SAMPLE_RATE_EXACT};
use crate::dsp::wavetables::FADER_TABLE;
use crate::node::AudioNode;

/// Maximum fade position (fully on).
const MAX_FADE: u32 = 0xFFFF_FFFF;

/// Volume fade effect. Smoothly fades audio in or out.
///
/// Effect node: 1 input, 1 output.
///
/// # Example
/// ```ignore
/// let mut fade = AudioEffectFade::new();
/// fade.fade_in(500);  // fade in over 500ms
/// ```
pub struct AudioEffectFade {
    /// Current fade position: 0 = silent, MAX_FADE = full volume.
    position: u32,
    /// Rate of position change per sample.
    rate: u32,
    /// Fade direction: true = fading in, false = fading out.
    direction_in: bool,
}

impl AudioEffectFade {
    /// Create a new fade effect, initially at full volume (no fade).
    pub const fn new() -> Self {
        AudioEffectFade {
            position: MAX_FADE,
            rate: 0,
            direction_in: true,
        }
    }

    /// Create a new fade effect, initially silent.
    pub const fn new_silent() -> Self {
        AudioEffectFade {
            position: 0,
            rate: 0,
            direction_in: true,
        }
    }

    /// Begin fading in over the given duration in milliseconds.
    pub fn fade_in(&mut self, milliseconds: u32) {
        let samples = if milliseconds == 0 {
            1
        } else {
            ((milliseconds as f32 * AUDIO_SAMPLE_RATE_EXACT) / 1000.0) as u32
        };
        let samples = if samples == 0 { 1 } else { samples };
        self.rate = MAX_FADE / samples;
        self.direction_in = true;
        // Ensure we're not stuck at exactly 0
        if self.position == 0 {
            self.position = 1;
        }
    }

    /// Begin fading out over the given duration in milliseconds.
    pub fn fade_out(&mut self, milliseconds: u32) {
        let samples = if milliseconds == 0 {
            1
        } else {
            ((milliseconds as f32 * AUDIO_SAMPLE_RATE_EXACT) / 1000.0) as u32
        };
        let samples = if samples == 0 { 1 } else { samples };
        self.rate = MAX_FADE / samples;
        self.direction_in = false;
        // Ensure we're not stuck at exactly MAX_FADE
        if self.position == MAX_FADE {
            self.position = MAX_FADE - 1;
        }
    }

    /// Get the current fade position (0.0 = silent, 1.0 = full volume).
    pub fn position_f32(&self) -> f32 {
        self.position as f32 / MAX_FADE as f32
    }
}

/// Look up the fader table with linear interpolation.
/// `pos` is a 32-bit position: upper 8 bits = index, bits 8–23 = fractional part.
#[inline]
fn fader_lookup(pos: u32) -> i32 {
    let index = (pos >> 24) as usize;
    let val1 = FADER_TABLE[index] as i32;
    let val2 = FADER_TABLE[index + 1] as i32;
    let scale = ((pos >> 8) & 0xFFFF) as i32;
    let interpolated = val1 * (0x10000 - scale) + val2 * scale;
    interpolated >> 16
}

impl AudioNode for AudioEffectFade {
    const NUM_INPUTS: usize = 1;
    const NUM_OUTPUTS: usize = 1;

    fn update(
        &mut self,
        inputs: &[Option<AudioBlockRef>],
        outputs: &mut [Option<AudioBlockMut>],
    ) {
        let input = match inputs[0] {
            Some(ref b) => b,
            None => {
                // No input: still advance position
                if self.rate > 0 {
                    let advance = (self.rate as u64) * (AUDIO_BLOCK_SAMPLES as u64);
                    if self.direction_in {
                        let new_pos = (self.position as u64).saturating_add(advance);
                        self.position = if new_pos > MAX_FADE as u64 { MAX_FADE } else { new_pos as u32 };
                    } else {
                        let new_pos = (self.position as u64).wrapping_sub(advance);
                        self.position = if self.position as u64 <= advance { 0 } else { new_pos as u32 };
                    }
                }
                return;
            }
        };

        let pos = self.position;

        if pos == 0 {
            // Fully silent: discard input
            return;
        }

        if pos == MAX_FADE && self.rate == 0 {
            // Full volume, not transitioning: pass through
            let mut out = match outputs[0].take() {
                Some(b) => b,
                None => return,
            };
            out.copy_from_slice(&input[..]);
            outputs[0] = Some(out);
            return;
        }

        let mut out = match outputs[0].take() {
            Some(b) => b,
            None => {
                // Still advance position even without output block
                if self.rate > 0 {
                    let advance = (self.rate as u64) * (AUDIO_BLOCK_SAMPLES as u64);
                    if self.direction_in {
                        let new_pos = (self.position as u64).saturating_add(advance);
                        self.position = if new_pos > MAX_FADE as u64 { MAX_FADE } else { new_pos as u32 };
                    } else {
                        self.position = if self.position as u64 <= advance { 0 } else { (self.position as u64 - advance) as u32 };
                    }
                }
                return;
            }
        };

        let mut current_pos = pos;
        let inc = self.rate;

        for i in 0..AUDIO_BLOCK_SAMPLES {
            let gain = fader_lookup(current_pos);
            let sample = input[i] as i32;
            out[i] = ((sample * gain) >> 15) as i16;

            // Advance position
            if self.direction_in {
                if inc < MAX_FADE - current_pos {
                    current_pos += inc;
                } else {
                    current_pos = MAX_FADE;
                }
            } else {
                if inc < current_pos {
                    current_pos -= inc;
                } else {
                    current_pos = 0;
                }
            }
        }

        self.position = current_pos;
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

    fn alloc_block_with_value(value: i16) -> AudioBlockMut {
        let mut block = AudioBlockMut::alloc().unwrap();
        block.fill(value);
        block
    }

    #[test]
    fn fade_full_volume_passthrough() {
        reset_pool();
        let mut fade = AudioEffectFade::new();

        let input = alloc_block_with_value(10000);
        let output = AudioBlockMut::alloc().unwrap();

        let input_ref = input.into_shared();
        let mut outputs = [Some(output)];
        let inputs = [Some(input_ref)];

        fade.update(&inputs, &mut outputs);

        let out = outputs[0].as_ref().unwrap();
        for &s in out.iter() {
            assert_eq!(s, 10000);
        }
    }

    #[test]
    fn fade_silent_discards() {
        reset_pool();
        let mut fade = AudioEffectFade::new_silent();

        let input = alloc_block_with_value(10000);
        let output = AudioBlockMut::alloc().unwrap();

        let input_ref = input.into_shared();
        let mut outputs = [Some(output)];
        let inputs = [Some(input_ref)];

        fade.update(&inputs, &mut outputs);

        // Position is 0, so update returns early; output block remains untouched
        assert!(outputs[0].is_some());
    }

    #[test]
    fn fade_in_increases_volume() {
        reset_pool();
        let mut fade = AudioEffectFade::new_silent();
        fade.fade_in(100); // 100ms fade in

        let input = alloc_block_with_value(20000);
        let output = AudioBlockMut::alloc().unwrap();

        let input_ref = input.into_shared();
        let mut outputs = [Some(output)];
        let inputs = [Some(input_ref)];

        fade.update(&inputs, &mut outputs);

        let out = outputs[0].as_ref().unwrap();
        // Samples should be increasing (fading in)
        assert!(out[127] > out[0], "last should be louder than first: {} vs {}", out[127], out[0]);
    }

    #[test]
    fn fade_out_decreases_volume() {
        reset_pool();
        let mut fade = AudioEffectFade::new();
        fade.fade_out(100); // 100ms fade out

        let input = alloc_block_with_value(20000);
        let output = AudioBlockMut::alloc().unwrap();

        let input_ref = input.into_shared();
        let mut outputs = [Some(output)];
        let inputs = [Some(input_ref)];

        fade.update(&inputs, &mut outputs);

        let out = outputs[0].as_ref().unwrap();
        // Samples should be decreasing (fading out)
        assert!(out[0] > out[127], "first should be louder than last: {} vs {}", out[0], out[127]);
    }

    #[test]
    fn fader_lookup_endpoints() {
        // Position 0 → gain 0
        assert_eq!(fader_lookup(0), 0);
        // Position MAX → gain ~32767
        let gain = fader_lookup(MAX_FADE);
        assert!(gain >= 32766, "expected ~32767, got {}", gain);
    }

    #[test]
    fn fade_position_clamps() {
        reset_pool();
        let mut fade = AudioEffectFade::new_silent();
        fade.fade_in(1); // very fast fade

        // Process multiple blocks to ensure position clamps at MAX_FADE
        for _ in 0..10 {
            let input = alloc_block_with_value(10000);
            let output = AudioBlockMut::alloc().unwrap();
            let input_ref = input.into_shared();
            let mut outputs = [Some(output)];
            let inputs = [Some(input_ref)];
            fade.update(&inputs, &mut outputs);
            // Drop outputs to release blocks back to pool
        }

        assert_eq!(fade.position, MAX_FADE);
    }
}
