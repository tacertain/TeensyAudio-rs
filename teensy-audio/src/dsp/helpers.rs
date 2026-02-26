//! Block-level DSP helper functions and Q15 arithmetic.

use crate::constants::AUDIO_BLOCK_SAMPLES;
use super::intrinsics::saturate16;

/// Saturating multiply of two Q15 values.
///
/// Computes `(a * b) >> 15`, saturated to `i16` range.
#[inline(always)]
pub fn saturating_multiply_q15(a: i16, b: i16) -> i16 {
    saturate16(((a as i32 * b as i32) >> 15) as i32)
}

/// Saturating addition of two Q15 (`i16`) values.
#[inline(always)]
pub fn saturating_add_q15(a: i16, b: i16) -> i16 {
    saturate16(a as i32 + b as i32)
}

/// Multiply every sample in `block` by `gain` (Q15 fixed-point, in an `i32`).
///
/// Each sample is computed as `saturate16((sample * gain) >> 15)`.
pub fn block_multiply(block: &mut [i16; AUDIO_BLOCK_SAMPLES], gain: i32) {
    for sample in block.iter_mut() {
        *sample = saturate16((*sample as i32 * gain) >> 15);
    }
}

/// Saturating-add `src` into `dst` sample-by-sample.
pub fn block_accumulate(
    dst: &mut [i16; AUDIO_BLOCK_SAMPLES],
    src: &[i16; AUDIO_BLOCK_SAMPLES],
) {
    for (d, &s) in dst.iter_mut().zip(src.iter()) {
        *d = saturate16(*d as i32 + s as i32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_saturating_multiply_q15() {
        // 1.0 * 1.0 in Q15: 32767 * 32767 >> 15 = 32766 (due to Q15 representation)
        assert_eq!(saturating_multiply_q15(32767, 32767), 32766);
        // 0 * anything = 0
        assert_eq!(saturating_multiply_q15(0, 32767), 0);
        // -1.0 * ~1.0: -32768 * 32767 >> 15 = -32767
        assert_eq!(saturating_multiply_q15(-32768, 32767), -32767);
        // 0.5 * 0.5 = 0.25 (16384 * 16384 >> 15 = 8192)
        assert_eq!(saturating_multiply_q15(16384, 16384), 8192);
    }

    #[test]
    fn test_saturating_add_q15() {
        assert_eq!(saturating_add_q15(100, 200), 300);
        assert_eq!(saturating_add_q15(32767, 1), 32767); // saturates
        assert_eq!(saturating_add_q15(-32768, -1), -32768); // saturates
        assert_eq!(saturating_add_q15(32000, 1000), 32767); // saturates
    }

    #[test]
    fn test_block_multiply() {
        let mut block = [0i16; AUDIO_BLOCK_SAMPLES];
        block[0] = 1000;
        block[1] = -1000;
        block[127] = 32767;

        // gain = 16384 = 0.5 in Q15
        block_multiply(&mut block, 16384);
        assert_eq!(block[0], 500);
        assert_eq!(block[1], -500);
        // 32767 * 16384 >> 15 = 16383
        assert_eq!(block[127], 16383);
    }

    #[test]
    fn test_block_accumulate() {
        let mut dst = [0i16; AUDIO_BLOCK_SAMPLES];
        let mut src = [0i16; AUDIO_BLOCK_SAMPLES];
        dst[0] = 100;
        src[0] = 200;
        dst[1] = 32000;
        src[1] = 1000;

        block_accumulate(&mut dst, &src);
        assert_eq!(dst[0], 300);
        assert_eq!(dst[1], 32767); // saturated
    }
}
