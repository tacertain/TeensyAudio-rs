//! Audio sample interleave/deinterleave utilities.
//!
//! These functions convert between separate left/right mono channel buffers
//! and the interleaved stereo format used by the SAI DMA buffer.
//!
//! ## DMA Buffer Format
//!
//! The SAI operates in 32-bit word mode with 2 channels per frame. Each
//! stereo frame occupies **two** `u32` words in the DMA buffer:
//!
//! ```text
//!   dest[i*2]     = left  sample, MSB-aligned (bits 31–16)
//!   dest[i*2 + 1] = right sample, MSB-aligned (bits 31–16)
//! ```
//!
//! 16-bit audio samples are placed in the upper half of each 32-bit I2S
//! word (`<< 16`). The lower 16 bits are zero. This matches the SGTL5000
//! codec in I2S mode with 32-bit BCLK slots (SCLKFREQ=0, 64×Fs).
//!
//! A buffer of `N` mono samples produces `N * 2` u32 words.

/// Interleave left and right channel samples into I2S stereo DMA format.
///
/// Each frame becomes two `u32` words: left (MSB-aligned), then right (MSB-aligned).
///
/// # Panics
///
/// Debug-asserts that `dest.len() == left.len() * 2` and `left.len() == right.len()`.
pub fn interleave_lr(dest: &mut [u32], left: &[i16], right: &[i16]) {
    debug_assert_eq!(dest.len(), left.len() * 2);
    debug_assert_eq!(left.len(), right.len());

    for i in 0..left.len() {
        dest[i * 2] = (left[i] as u16 as u32) << 16;
        dest[i * 2 + 1] = (right[i] as u16 as u32) << 16;
    }
}

/// Interleave left channel only into I2S stereo DMA format.
///
/// The right channel is set to zero (silence).
///
/// # Panics
///
/// Debug-asserts that `dest.len() == left.len() * 2`.
pub fn interleave_l(dest: &mut [u32], left: &[i16]) {
    debug_assert_eq!(dest.len(), left.len() * 2);

    for i in 0..left.len() {
        dest[i * 2] = (left[i] as u16 as u32) << 16;
        dest[i * 2 + 1] = 0;
    }
}

/// Interleave right channel only into I2S stereo DMA format.
///
/// The left channel is set to zero (silence).
///
/// # Panics
///
/// Debug-asserts that `dest.len() == right.len() * 2`.
pub fn interleave_r(dest: &mut [u32], right: &[i16]) {
    debug_assert_eq!(dest.len(), right.len() * 2);

    for i in 0..right.len() {
        dest[i * 2] = 0;
        dest[i * 2 + 1] = (right[i] as u16 as u32) << 16;
    }
}

/// Deinterleave I2S stereo DMA buffer into separate left and right channels.
///
/// Reads the upper 16 bits of each `u32` word (MSB-aligned samples).
///
/// # Panics
///
/// Debug-asserts that `src.len() == left.len() * 2` and `left.len() == right.len()`.
pub fn deinterleave(src: &[u32], left: &mut [i16], right: &mut [i16]) {
    debug_assert_eq!(src.len(), left.len() * 2);
    debug_assert_eq!(left.len(), right.len());

    for i in 0..left.len() {
        left[i] = (src[i * 2] >> 16) as i16;
        right[i] = (src[i * 2 + 1] >> 16) as i16;
    }
}

/// Fill a region of the DMA buffer with silence (zero for both channels).
pub fn silence(dest: &mut [u32]) {
    dest.fill(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleave_lr_basic() {
        let left = [100i16, -200, 300, -400];
        let right = [500i16, -600, 700, -800];
        let mut dest = [0u32; 8]; // 4 frames × 2 words

        interleave_lr(&mut dest, &left, &right);

        for i in 0..4 {
            assert_eq!(
                (dest[i * 2] >> 16) as i16,
                left[i],
                "left mismatch at frame {i}"
            );
            assert_eq!(
                (dest[i * 2 + 1] >> 16) as i16,
                right[i],
                "right mismatch at frame {i}"
            );
            // Lower 16 bits should be zero
            assert_eq!(dest[i * 2] & 0xFFFF, 0, "left low bits at frame {i}");
            assert_eq!(dest[i * 2 + 1] & 0xFFFF, 0, "right low bits at frame {i}");
        }
    }

    #[test]
    fn interleave_l_zeroes_right() {
        let left = [1000i16, -2000];
        let mut dest = [0xFFFF_FFFFu32; 4]; // 2 frames × 2 words

        interleave_l(&mut dest, &left);

        assert_eq!((dest[0] >> 16) as i16, 1000);
        assert_eq!(dest[1], 0); // right = silence
        assert_eq!((dest[2] >> 16) as i16, -2000);
        assert_eq!(dest[3], 0); // right = silence
    }

    #[test]
    fn interleave_r_zeroes_left() {
        let right = [3000i16, -4000];
        let mut dest = [0xFFFF_FFFFu32; 4]; // 2 frames × 2 words

        interleave_r(&mut dest, &right);

        assert_eq!(dest[0], 0); // left = silence
        assert_eq!((dest[1] >> 16) as i16, 3000);
        assert_eq!(dest[2], 0); // left = silence
        assert_eq!((dest[3] >> 16) as i16, -4000);
    }

    #[test]
    fn deinterleave_basic() {
        // Pack known values in the new format: 2 words per frame, MSB-aligned
        let src = [
            (100u16 as u32) << 16,  // left[0]
            (500u16 as u32) << 16,  // right[0]
            ((-200i16 as u16) as u32) << 16, // left[1]
            ((-600i16 as u16) as u32) << 16, // right[1]
        ];
        let mut left = [0i16; 2];
        let mut right = [0i16; 2];

        deinterleave(&src, &mut left, &mut right);

        assert_eq!(left, [100, -200]);
        assert_eq!(right, [500, -600]);
    }

    #[test]
    fn roundtrip_preserves_data() {
        let orig_left = [i16::MIN, -1, 0, 1, i16::MAX, 12345, -12345, 0];
        let orig_right = [0, i16::MAX, i16::MIN, 42, -42, 100, -100, 0];
        let mut packed = [0u32; 16]; // 8 frames × 2 words

        interleave_lr(&mut packed, &orig_left, &orig_right);

        let mut left = [0i16; 8];
        let mut right = [0i16; 8];
        deinterleave(&packed, &mut left, &mut right);

        assert_eq!(left, orig_left);
        assert_eq!(right, orig_right);
    }

    #[test]
    fn empty_slices() {
        let mut dest = [];
        interleave_lr(&mut dest, &[], &[]);
        interleave_l(&mut dest, &[]);
        interleave_r(&mut dest, &[]);

        let mut left = [];
        let mut right = [];
        deinterleave(&[], &mut left, &mut right);
    }

    #[test]
    fn silence_zeroes_buffer() {
        let mut buf = [0xDEAD_BEEFu32; 8];
        silence(&mut buf);
        assert!(buf.iter().all(|&x| x == 0));
    }

    #[test]
    fn extreme_values() {
        let left = [i16::MIN, i16::MAX];
        let right = [i16::MAX, i16::MIN];
        let mut packed = [0u32; 4]; // 2 frames × 2 words

        interleave_lr(&mut packed, &left, &right);

        let mut out_left = [0i16; 2];
        let mut out_right = [0i16; 2];
        deinterleave(&packed, &mut out_left, &mut out_right);

        assert_eq!(out_left, [i16::MIN, i16::MAX]);
        assert_eq!(out_right, [i16::MAX, i16::MIN]);
    }
}
