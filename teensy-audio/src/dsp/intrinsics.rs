//! ARM DSP instruction wrappers with pure-Rust fallbacks.
//!
//! On `thumbv7em` targets (Cortex-M4/M7 with DSP extension), these compile to
//! single-cycle ARM instructions. On other targets (host tests, Cortex-M0),
//! equivalent pure-Rust implementations are used.

/// Signed saturate with arithmetic right shift.
///
/// Computes `saturate(val >> RSHIFT, -(2^(BITS-1))..2^(BITS-1)-1)`.
///
/// Maps to ARM `SSAT` instruction. `BITS` and `RSHIFT` must be compile-time constants
/// because the ARM instruction requires immediate operands.
#[inline(always)]
pub fn signed_saturate_rshift<const BITS: u32, const RSHIFT: u32>(val: i32) -> i32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let out: i32;
        unsafe {
            core::arch::asm!(
                "ssat {out}, #{bits}, {val}, asr #{rshift}",
                out = out(reg) out,
                val = in(reg) val,
                bits = const BITS,
                rshift = const RSHIFT,
            );
        }
        out
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        let shifted = val >> RSHIFT;
        let max = (1i32 << (BITS - 1)) - 1;
        let min = -(1i32 << (BITS - 1));
        if shifted > max {
            max
        } else if shifted < min {
            min
        } else {
            shifted
        }
    }
}

/// Saturate an `i32` to `i16` range (`-32768..=32767`).
///
/// Maps to ARM `SSAT #16`.
#[inline(always)]
pub fn saturate16(val: i32) -> i16 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let out: i32;
        unsafe {
            core::arch::asm!(
                "ssat {out}, #16, {val}",
                out = out(reg) out,
                val = in(reg) val,
            );
        }
        out as i16
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        if val > 32767 {
            32767
        } else if val < -32768 {
            -32768
        } else {
            val as i16
        }
    }
}

/// Multiply 32-bit by bottom 16 bits, right-shift 16.
///
/// Computes `(a * b[15:0]) >> 16`. Maps to ARM `SMULWB`.
#[inline(always)]
pub fn mul_32x16b(a: i32, b: u32) -> i32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let out: i32;
        unsafe {
            core::arch::asm!(
                "smulwb {out}, {a}, {b}",
                out = out(reg) out,
                a = in(reg) a,
                b = in(reg) b,
            );
        }
        out
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        ((a as i64 * (b as i16 as i64)) >> 16) as i32
    }
}

/// Multiply 32-bit by top 16 bits, right-shift 16.
///
/// Computes `(a * b[31:16]) >> 16`. Maps to ARM `SMULWT`.
#[inline(always)]
pub fn mul_32x16t(a: i32, b: u32) -> i32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let out: i32;
        unsafe {
            core::arch::asm!(
                "smulwt {out}, {a}, {b}",
                out = out(reg) out,
                a = in(reg) a,
                b = in(reg) b,
            );
        }
        out
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        ((a as i64 * ((b as i32 >> 16) as i64)) >> 16) as i32
    }
}

/// Multiply two 32-bit values, return upper 32 bits.
///
/// Computes `(a * b) >> 32`. Maps to ARM `SMMUL`.
#[inline(always)]
pub fn mul_32x32_rshift32(a: i32, b: i32) -> i32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let out: i32;
        unsafe {
            core::arch::asm!(
                "smmul {out}, {a}, {b}",
                out = out(reg) out,
                a = in(reg) a,
                b = in(reg) b,
            );
        }
        out
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        ((a as i64 * b as i64) >> 32) as i32
    }
}

/// Multiply two 32-bit values, return upper 32 bits, rounded.
///
/// Computes `(a * b + 0x80000000) >> 32`. Maps to ARM `SMMULR`.
#[inline(always)]
pub fn mul_32x32_rshift32_rounded(a: i32, b: i32) -> i32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let out: i32;
        unsafe {
            core::arch::asm!(
                "smmulr {out}, {a}, {b}",
                out = out(reg) out,
                a = in(reg) a,
                b = in(reg) b,
            );
        }
        out
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        ((a as i64 * b as i64 + 0x80000000i64) >> 32) as i32
    }
}

/// Multiply-accumulate: `sum + (a * b + 0x80000000) >> 32`. Maps to ARM `SMMLAR`.
#[inline(always)]
pub fn multiply_accumulate_32x32_rshift32_rounded(sum: i32, a: i32, b: i32) -> i32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let out: i32;
        unsafe {
            core::arch::asm!(
                "smmlar {out}, {a}, {b}, {sum}",
                out = out(reg) out,
                a = in(reg) a,
                b = in(reg) b,
                sum = in(reg) sum,
            );
        }
        out
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        sum + ((a as i64 * b as i64 + 0x80000000i64) >> 32) as i32
    }
}

/// Multiply-subtract: `sum - (a * b + 0x80000000) >> 32`. Maps to ARM `SMMLSR`.
#[inline(always)]
pub fn multiply_subtract_32x32_rshift32_rounded(sum: i32, a: i32, b: i32) -> i32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let out: i32;
        unsafe {
            core::arch::asm!(
                "smmlsr {out}, {a}, {b}, {sum}",
                out = out(reg) out,
                a = in(reg) a,
                b = in(reg) b,
                sum = in(reg) sum,
            );
        }
        out
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        sum - ((a as i64 * b as i64 + 0x80000000i64) >> 32) as i32
    }
}

/// Pack bottom 16 bits of `a` into top half, bottom 16 bits of `b` into bottom half.
///
/// Computes `(a[15:0] << 16) | b[15:0]`. Maps to ARM `PKHBT`.
#[inline(always)]
pub fn pack_16b_16b(a: i32, b: i32) -> u32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let out: u32;
        unsafe {
            core::arch::asm!(
                "pkhbt {out}, {b}, {a}, lsl #16",
                out = out(reg) out,
                a = in(reg) a,
                b = in(reg) b,
            );
        }
        out
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        ((a as u32) << 16) | (b as u32 & 0x0000FFFF)
    }
}

/// Pack top 16 bits of `a` into top half, bottom 16 bits of `b` into bottom half.
///
/// Computes `a[31:16] | b[15:0]`. Maps to ARM `PKHTB`.
#[inline(always)]
pub fn pack_16t_16b(a: i32, b: i32) -> u32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let out: u32;
        unsafe {
            core::arch::asm!(
                "pkhtb {out}, {a}, {b}",
                out = out(reg) out,
                a = in(reg) a,
                b = in(reg) b,
            );
        }
        out
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        (a as u32 & 0xFFFF0000) | (b as u32 & 0x0000FFFF)
    }
}

/// Pack top 16 bits of `a` into top half, top 16 bits of `b` into bottom half.
///
/// Computes `a[31:16] | (b >> 16)`. Maps to ARM `PKHTB ASR #16`.
#[inline(always)]
pub fn pack_16t_16t(a: i32, b: i32) -> u32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let out: u32;
        unsafe {
            core::arch::asm!(
                "pkhtb {out}, {a}, {b}, asr #16",
                out = out(reg) out,
                a = in(reg) a,
                b = in(reg) b,
            );
        }
        out
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        (a as u32 & 0xFFFF0000) | ((b as u32) >> 16)
    }
}

/// Saturating dual 16-bit addition.
///
/// Independently saturate-adds the top and bottom 16-bit halfwords.
/// Maps to ARM `QADD16`.
#[inline(always)]
pub fn qadd16(a: u32, b: u32) -> u32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let out: u32;
        unsafe {
            core::arch::asm!(
                "qadd16 {out}, {a}, {b}",
                out = out(reg) out,
                a = in(reg) a,
                b = in(reg) b,
            );
        }
        out
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        let a_lo = a as i16 as i32;
        let a_hi = (a >> 16) as i16 as i32;
        let b_lo = b as i16 as i32;
        let b_hi = (b >> 16) as i16 as i32;
        let lo = (a_lo + b_lo).clamp(-32768, 32767) as i16 as u16;
        let hi = (a_hi + b_hi).clamp(-32768, 32767) as i16 as u16;
        (hi as u32) << 16 | lo as u32
    }
}

/// Saturating dual 16-bit subtraction.
///
/// Independently saturate-subtracts the top and bottom 16-bit halfwords.
/// Maps to ARM `QSUB16`.
#[inline(always)]
pub fn qsub16(a: u32, b: u32) -> u32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let out: u32;
        unsafe {
            core::arch::asm!(
                "qsub16 {out}, {a}, {b}",
                out = out(reg) out,
                a = in(reg) a,
                b = in(reg) b,
            );
        }
        out
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        let a_lo = a as i16 as i32;
        let a_hi = (a >> 16) as i16 as i32;
        let b_lo = b as i16 as i32;
        let b_hi = (b >> 16) as i16 as i32;
        let lo = (a_lo - b_lo).clamp(-32768, 32767) as i16 as u16;
        let hi = (a_hi - b_hi).clamp(-32768, 32767) as i16 as u16;
        (hi as u32) << 16 | lo as u32
    }
}

/// Multiply bottom halfwords: `a[15:0] * b[15:0]`. Maps to ARM `SMULBB`.
#[inline(always)]
pub fn mul_16bx16b(a: u32, b: u32) -> i32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let out: i32;
        unsafe {
            core::arch::asm!(
                "smulbb {out}, {a}, {b}",
                out = out(reg) out,
                a = in(reg) a,
                b = in(reg) b,
            );
        }
        out
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        (a as i16 as i32) * (b as i16 as i32)
    }
}

/// Multiply bottom by top halfword: `a[15:0] * b[31:16]`. Maps to ARM `SMULBT`.
#[inline(always)]
pub fn mul_16bx16t(a: u32, b: u32) -> i32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let out: i32;
        unsafe {
            core::arch::asm!(
                "smulbt {out}, {a}, {b}",
                out = out(reg) out,
                a = in(reg) a,
                b = in(reg) b,
            );
        }
        out
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        (a as i16 as i32) * ((b >> 16) as i16 as i32)
    }
}

/// Multiply top by bottom halfword: `a[31:16] * b[15:0]`. Maps to ARM `SMULTB`.
#[inline(always)]
pub fn mul_16tx16b(a: u32, b: u32) -> i32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let out: i32;
        unsafe {
            core::arch::asm!(
                "smultb {out}, {a}, {b}",
                out = out(reg) out,
                a = in(reg) a,
                b = in(reg) b,
            );
        }
        out
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        ((a >> 16) as i16 as i32) * (b as i16 as i32)
    }
}

/// Multiply top halfwords: `a[31:16] * b[31:16]`. Maps to ARM `SMULTT`.
#[inline(always)]
pub fn mul_16tx16t(a: u32, b: u32) -> i32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let out: i32;
        unsafe {
            core::arch::asm!(
                "smultt {out}, {a}, {b}",
                out = out(reg) out,
                a = in(reg) a,
                b = in(reg) b,
            );
        }
        out
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        ((a >> 16) as i16 as i32) * ((b >> 16) as i16 as i32)
    }
}

/// Multiply-accumulate 32x16 bottom: `sum + (a * b[15:0]) >> 16`. Maps to ARM `SMLAWB`.
#[inline(always)]
pub fn multiply_accumulate_32x16b(sum: i32, a: i32, b: u32) -> i32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let out: i32;
        unsafe {
            core::arch::asm!(
                "smlawb {out}, {a}, {b}, {sum}",
                out = out(reg) out,
                a = in(reg) a,
                b = in(reg) b,
                sum = in(reg) sum,
            );
        }
        out
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        sum + ((a as i64 * (b as i16 as i64)) >> 16) as i32
    }
}

/// Multiply-accumulate 32x16 top: `sum + (a * b[31:16]) >> 16`. Maps to ARM `SMLAWT`.
#[inline(always)]
pub fn multiply_accumulate_32x16t(sum: i32, a: i32, b: u32) -> i32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let out: i32;
        unsafe {
            core::arch::asm!(
                "smlawt {out}, {a}, {b}, {sum}",
                out = out(reg) out,
                a = in(reg) a,
                b = in(reg) b,
                sum = in(reg) sum,
            );
        }
        out
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        sum + ((a as i64 * ((b as i32 >> 16) as i64)) >> 16) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_saturate16() {
        assert_eq!(saturate16(0), 0);
        assert_eq!(saturate16(32767), 32767);
        assert_eq!(saturate16(32768), 32767);
        assert_eq!(saturate16(-32768), -32768);
        assert_eq!(saturate16(-32769), -32768);
        assert_eq!(saturate16(100000), 32767);
        assert_eq!(saturate16(-100000), -32768);
    }

    #[test]
    fn test_signed_saturate_rshift() {
        // saturate(100 >> 1, 8 bits) = saturate(50, -128..127) = 50
        assert_eq!(signed_saturate_rshift::<8, 1>(100), 50);
        // saturate(1000 >> 2, 8 bits) = saturate(250, -128..127) = 127
        assert_eq!(signed_saturate_rshift::<8, 2>(1000), 127);
        // saturate(-1000 >> 2, 8 bits) = saturate(-250, -128..127) = -128
        assert_eq!(signed_saturate_rshift::<8, 2>(-1000), -128);
        // saturate(256 >> 0, 16 bits) = 256
        assert_eq!(signed_saturate_rshift::<16, 0>(256), 256);
    }

    #[test]
    fn test_mul_32x32_rshift32() {
        // (0x40000000 * 0x40000000) >> 32 = 0x10000000
        assert_eq!(mul_32x32_rshift32(0x40000000, 0x40000000), 0x10000000);
        // (-1 * 1) >> 32 = -1 (arithmetic shift)
        assert_eq!(mul_32x32_rshift32(-1, 1), -1);
    }

    #[test]
    fn test_mul_32x32_rshift32_rounded() {
        assert_eq!(
            mul_32x32_rshift32_rounded(0x40000000, 0x40000000),
            0x10000000
        );
    }

    #[test]
    fn test_pack_16b_16b() {
        // (0x1234 << 16) | 0x5678
        assert_eq!(pack_16b_16b(0x1234, 0x5678), 0x12345678);
    }

    #[test]
    fn test_pack_16t_16b() {
        // 0xABCD0000 | 0x1234
        assert_eq!(pack_16t_16b(0xABCD0000u32 as i32, 0x1234), 0xABCD1234);
    }

    #[test]
    fn test_pack_16t_16t() {
        // 0xABCD0000 | (0x12340000 >> 16) = 0xABCD1234
        assert_eq!(
            pack_16t_16t(0xABCD0000u32 as i32, 0x12340000u32 as i32),
            0xABCD1234
        );
    }

    #[test]
    fn test_qadd16() {
        // Two packed i16 pairs: (1, 2) + (3, 4) = (4, 6)
        let a = pack_16b_16b(1, 2);
        let b = pack_16b_16b(3, 4);
        let result = qadd16(a, b);
        assert_eq!(result as i16, 6i16); // bottom: 2+4
        assert_eq!((result >> 16) as i16, 4i16); // top: 1+3
    }

    #[test]
    fn test_qadd16_saturation() {
        // Test saturation: 32767 + 1 should saturate to 32767
        let a = pack_16b_16b(0, 32767);
        let b = pack_16b_16b(0, 1);
        let result = qadd16(a, b);
        assert_eq!(result as i16, 32767i16);
    }

    #[test]
    fn test_qsub16() {
        let a = pack_16b_16b(10, 20);
        let b = pack_16b_16b(3, 5);
        let result = qsub16(a, b);
        assert_eq!(result as i16, 15i16); // bottom: 20-5
        assert_eq!((result >> 16) as i16, 7i16); // top: 10-3
    }

    #[test]
    fn test_mul_16bx16b() {
        // 3 * 4 = 12
        assert_eq!(mul_16bx16b(3, 4), 12);
        // (-2) * 5 = -10 (pack -2 in bottom halfword)
        let a = (-2i16) as u16 as u32;
        assert_eq!(mul_16bx16b(a, 5), -10);
    }

    #[test]
    fn test_mul_16tx16t() {
        // top halfwords: 3 * 4 = 12
        let a = 3u32 << 16;
        let b = 4u32 << 16;
        assert_eq!(mul_16tx16t(a, b), 12);
    }

    #[test]
    fn test_mul_32x16b() {
        // (0x10000 * 0x0002) >> 16 = 2
        assert_eq!(mul_32x16b(0x10000, 0x0002), 2);
    }

    #[test]
    fn test_mul_32x16t() {
        // (0x10000 * (0x0003 << 16)) >> 16 = (0x10000 * 0x0003) >> 16 = 3
        assert_eq!(mul_32x16t(0x10000, 0x00030000), 3);
    }

    #[test]
    fn test_multiply_accumulate() {
        let sum = 100;
        let result = multiply_accumulate_32x32_rshift32_rounded(sum, 0x40000000, 0x40000000);
        assert_eq!(result, 100 + 0x10000000);
    }

    #[test]
    fn test_multiply_subtract() {
        let sum = 0x20000000;
        let result = multiply_subtract_32x32_rshift32_rounded(sum, 0x40000000, 0x40000000);
        assert_eq!(result, 0x20000000 - 0x10000000);
    }

    #[test]
    fn test_multiply_accumulate_32x16b() {
        let result = multiply_accumulate_32x16b(10, 0x10000, 0x0003);
        // 10 + (0x10000 * 3) >> 16 = 10 + 3 = 13
        assert_eq!(result, 13);
    }

    #[test]
    fn test_multiply_accumulate_32x16t() {
        let result = multiply_accumulate_32x16t(10, 0x10000, 0x00050000);
        // 10 + (0x10000 * 5) >> 16 = 10 + 5 = 15
        assert_eq!(result, 15);
    }
}
