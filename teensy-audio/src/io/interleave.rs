//! Audio sample interleave/deinterleave utilities.
//!
//! These functions convert between separate left/right mono channel buffers
//! and the interleaved stereo format used by the SAI DMA buffer.
//!
//! ## DMA Buffer Format
//!
//! Each `u32` in the DMA buffer contains one stereo frame:
//! - Lower 16 bits (bits 0–15): left channel sample (`i16`)
//! - Upper 16 bits (bits 16–31): right channel sample (`i16`)
//!
//! On little-endian ARM, this corresponds to `[left, right]` as consecutive
//! `i16` values in memory, matching the SAI I2S frame format.

/// Interleave left and right channel samples into packed stereo `u32` format.
///
/// Each output `u32` packs: `(right << 16) | (left & 0xFFFF)`.
///
/// # Panics
///
/// Debug-asserts that all slices have the same length.
pub fn interleave_lr(dest: &mut [u32], left: &[i16], right: &[i16]) {
    debug_assert_eq!(dest.len(), left.len());
    debug_assert_eq!(dest.len(), right.len());

    for i in 0..dest.len() {
        dest[i] = (left[i] as u16 as u32) | ((right[i] as u16 as u32) << 16);
    }
}

/// Interleave left channel only into packed stereo `u32` format.
///
/// The right channel is set to zero (silence).
///
/// # Panics
///
/// Debug-asserts that both slices have the same length.
pub fn interleave_l(dest: &mut [u32], left: &[i16]) {
    debug_assert_eq!(dest.len(), left.len());

    for i in 0..dest.len() {
        dest[i] = left[i] as u16 as u32;
    }
}

/// Interleave right channel only into packed stereo `u32` format.
///
/// The left channel is set to zero (silence).
///
/// # Panics
///
/// Debug-asserts that both slices have the same length.
pub fn interleave_r(dest: &mut [u32], right: &[i16]) {
    debug_assert_eq!(dest.len(), right.len());

    for i in 0..dest.len() {
        dest[i] = (right[i] as u16 as u32) << 16;
    }
}

/// Deinterleave packed stereo `u32` buffer into separate left and right channels.
///
/// Extracts left (lower 16 bits) and right (upper 16 bits) from each `u32`.
///
/// # Panics
///
/// Debug-asserts that all slices have the same length.
pub fn deinterleave(src: &[u32], left: &mut [i16], right: &mut [i16]) {
    debug_assert_eq!(src.len(), left.len());
    debug_assert_eq!(src.len(), right.len());

    for i in 0..src.len() {
        left[i] = src[i] as i16;
        right[i] = (src[i] >> 16) as i16;
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
        let mut dest = [0u32; 4];

        interleave_lr(&mut dest, &left, &right);

        for i in 0..4 {
            assert_eq!(dest[i] as i16, left[i], "left mismatch at index {i}");
            assert_eq!((dest[i] >> 16) as i16, right[i], "right mismatch at index {i}");
        }
    }

    #[test]
    fn interleave_l_zeroes_right() {
        let left = [1000i16, -2000];
        let mut dest = [0xFFFF_FFFFu32; 2];

        interleave_l(&mut dest, &left);

        assert_eq!(dest[0] as i16, 1000);
        assert_eq!((dest[0] >> 16) as i16, 0);
        assert_eq!(dest[1] as i16, -2000);
        assert_eq!((dest[1] >> 16) as i16, 0);
    }

    #[test]
    fn interleave_r_zeroes_left() {
        let right = [3000i16, -4000];
        let mut dest = [0xFFFF_FFFFu32; 2];

        interleave_r(&mut dest, &right);

        assert_eq!(dest[0] as i16, 0);
        assert_eq!((dest[0] >> 16) as i16, 3000);
        assert_eq!(dest[1] as i16, 0);
        assert_eq!((dest[1] >> 16) as i16, -4000);
    }

    #[test]
    fn deinterleave_basic() {
        // Pack known values: left=100, right=500; left=-200, right=-600
        let src = [
            (100u16 as u32) | ((500u16 as u32) << 16),
            ((-200i16 as u16) as u32) | (((-600i16 as u16) as u32) << 16),
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
        let mut packed = [0u32; 8];

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
        let mut packed = [0u32; 2];

        interleave_lr(&mut packed, &left, &right);

        let mut out_left = [0i16; 2];
        let mut out_right = [0i16; 2];
        deinterleave(&packed, &mut out_left, &mut out_right);

        assert_eq!(out_left, [i16::MIN, i16::MAX]);
        assert_eq!(out_right, [i16::MAX, i16::MIN]);
    }
}
