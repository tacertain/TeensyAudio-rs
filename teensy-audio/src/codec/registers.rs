//! SGTL5000 register addresses and bitfield definitions.
//!
//! Ported from the C++ `control_sgtl5000.cpp` register definitions and the
//! NXP SGTL5000 datasheet. Register addresses are 16-bit; all registers hold
//! 16-bit values. I2C protocol uses big-endian byte order.

// Some registers are defined for completeness (AVC, status, test, etc.)
// but are not yet used by the driver.
#![allow(dead_code)]

// ── I2C addresses ──────────────────────────────────────────────────────────

/// Default I2C address (CTRL_ADR0_CS pin low).
pub const I2C_ADDR_CS_LOW: u8 = 0x0A;

/// Alternate I2C address (CTRL_ADR0_CS pin high).
pub const I2C_ADDR_CS_HIGH: u8 = 0x2A;

// ── Chip identification ────────────────────────────────────────────────────

/// Chip ID register (read-only).
/// - Bits 15:8 — PARTID (0xA0 for SGTL5000)
/// - Bits  7:0 — REVID
pub const CHIP_ID: u16 = 0x0000;

// ── Digital power ──────────────────────────────────────────────────────────

/// Digital block power control.
/// - Bit 6 — ADC_POWERUP
/// - Bit 5 — DAC_POWERUP
/// - Bit 4 — DAP_POWERUP
/// - Bit 1 — I2S_OUT_POWERUP
/// - Bit 0 — I2S_IN_POWERUP
pub const CHIP_DIG_POWER: u16 = 0x0002;

// ── Clocking ───────────────────────────────────────────────────────────────

/// Clock control.
/// - Bits 5:4 — RATE_MODE (sample rate mode)
/// - Bits 3:2 — SYS_FS (0=32k, 1=44.1k, 2=48k, 3=96k)
/// - Bits 1:0 — MCLK_FREQ (0=256Fs, 1=384Fs, 2=512Fs, 3=PLL)
pub const CHIP_CLK_CTRL: u16 = 0x0004;

/// PLL control.
/// - Bits 15:11 — INT_DIVISOR
/// - Bits 10:0  — FRAC_DIVISOR
pub const CHIP_PLL_CTRL: u16 = 0x0032;

/// Clock top-level control.
/// - Bit 11 — ENABLE_INT_OSC
/// - Bit  3 — INPUT_FREQ_DIV2 (divide SYS_MCLK by 2 before PLL; set when >17 MHz)
pub const CHIP_CLK_TOP_CTRL: u16 = 0x0034;

// ── I2S interface ──────────────────────────────────────────────────────────

/// I2S control.
/// - Bit 8   — SCLKFREQ (0=64Fs, 1=32Fs)
/// - Bit 7   — MS (0=slave, 1=master)
/// - Bit 6   — SCLK_INV
/// - Bits 5:4 — DLEN (0=32bit, 1=24bit, 2=20bit, 3=16bit)
/// - Bits 3:2 — I2S_MODE (0=I2S/LJ, 1=RJ, 2=PCM)
/// - Bit 1   — LRALIGN
/// - Bit 0   — LRPOL
pub const CHIP_I2S_CTRL: u16 = 0x0006;

// ── Signal routing ─────────────────────────────────────────────────────────

/// Source-select control for signal routing.
/// - Bit  14    — DAP_MIX_LRSWAP
/// - Bit  13    — DAP_LRSWAP
/// - Bit  12    — DAC_LRSWAP
/// - Bit  10    — I2S_LRSWAP
/// - Bits  9:8  — DAP_MIX_SELECT (0=ADC, 1=I2S)
/// - Bits  7:6  — DAP_SELECT     (0=ADC, 1=I2S)
/// - Bits  5:4  — DAC_SELECT     (0=ADC, 1=I2S, 3=DAP)
/// - Bits  1:0  — I2S_SELECT     (0=ADC, 1=I2S, 3=DAP)
pub const CHIP_SSS_CTRL: u16 = 0x000A;

// ── ADC/DAC control ────────────────────────────────────────────────────────

/// ADC/DAC shared control.
/// - Bit 13 — VOL_BUSY_DAC_RIGHT
/// - Bit 12 — VOL_BUSY_DAC_LEFT
/// - Bit  9 — VOL_RAMP_EN (default=1)
/// - Bit  8 — VOL_EXPO_RAMP
/// - Bit  3 — DAC_MUTE_RIGHT (default=1)
/// - Bit  2 — DAC_MUTE_LEFT (default=1)
/// - Bit  1 — ADC_HPF_FREEZE
/// - Bit  0 — ADC_HPF_BYPASS
pub const CHIP_ADCDAC_CTRL: u16 = 0x000E;

/// DAC volume (0.5 dB steps, 0 dB to −90 dB).
/// - Bits 15:8 — DAC_VOL_RIGHT (0x3C = 0 dB, 0xFC+ = muted)
/// - Bits  7:0 — DAC_VOL_LEFT
pub const CHIP_DAC_VOL: u16 = 0x0010;

// ── Pad strength ───────────────────────────────────────────────────────────

/// I/O pad drive-strength control.
pub const CHIP_PAD_STRENGTH: u16 = 0x0014;

// ── Analog ADC/HP/control ──────────────────────────────────────────────────

/// Analog ADC control.
/// - Bit 8   — ADC_VOL_M6DB (range reduction by 6 dB)
/// - Bits 7:4 — ADC_VOL_RIGHT (0–15, 1.5 dB steps)
/// - Bits 3:0 — ADC_VOL_LEFT
pub const CHIP_ANA_ADC_CTRL: u16 = 0x0020;

/// Headphone volume (0.5 dB steps).
/// - Bits 14:8 — HP_VOL_RIGHT (0x00 = +12 dB, 0x7F = −51.5 dB)
/// - Bits  6:0 — HP_VOL_LEFT
pub const CHIP_ANA_HP_CTRL: u16 = 0x0022;

/// Analog control (mutes, input/output selection, zero-cross detect).
/// - Bit 8 — MUTE_LO (lineout mute)
/// - Bit 6 — SELECT_HP (headphone source)
/// - Bit 5 — EN_ZCD_HP
/// - Bit 4 — MUTE_HP
/// - Bit 2 — SELECT_ADC (0=mic, 1=linein)
/// - Bit 1 — EN_ZCD_ADC
/// - Bit 0 — MUTE_ADC
pub const CHIP_ANA_CTRL: u16 = 0x0024;

// ── Voltage regulation and reference ───────────────────────────────────────

/// Linear regulator control.
/// - Bit 6 — VDDC_MAN_ASSN
/// - Bit 5 — VDDC_ASSN_OVRD
/// - Bits 3:0 — D_PROGRAMMING (VDDD output voltage)
pub const CHIP_LINREG_CTRL: u16 = 0x0026;

/// Reference voltage / bias control.
/// - Bits 8:4 — VAG_VAL (analog ground, 25 mV steps, 0x00=0.8V .. 0x1F=1.575V)
/// - Bits 3:1 — BIAS_CTRL
/// - Bit    0 — SMALL_POP (slow VAG ramp)
pub const CHIP_REF_CTRL: u16 = 0x0028;

// ── Microphone ─────────────────────────────────────────────────────────────

/// Microphone gain and bias control.
/// - Bits 9:8 — BIAS_RESISTOR (0=off, 1=2k, 2=4k, 3=8k)
/// - Bits 6:4 — BIAS_VOLT (250 mV steps, 0=1.25V .. 7=3.0V)
/// - Bits 1:0 — GAIN (0=0dB, 1=+20dB, 2=+30dB, 3=+40dB)
pub const CHIP_MIC_CTRL: u16 = 0x002A;

// ── Line output ────────────────────────────────────────────────────────────

/// Line-out amplifier control.
/// - Bits 11:8 — OUT_CURRENT (bias current)
/// - Bits  5:0 — LO_VAGCNTRL (analog ground, 25 mV steps)
pub const CHIP_LINE_OUT_CTRL: u16 = 0x002C;

/// Line-out volume (0.5 dB steps).
/// - Bits 12:8 — LO_VOL_RIGHT
/// - Bits  4:0 — LO_VOL_LEFT
pub const CHIP_LINE_OUT_VOL: u16 = 0x002E;

// ── Analog power ───────────────────────────────────────────────────────────

/// Analog power-down control.
/// - Bit 14 — DAC_MONO
/// - Bit 13 — LINREG_SIMPLE_POWERUP
/// - Bit 12 — STARTUP_POWERUP
/// - Bit 11 — VDDC_CHRGPMP_POWERUP
/// - Bit 10 — PLL_POWERUP
/// - Bit  9 — LINREG_D_POWERUP
/// - Bit  8 — VCOAMP_POWERUP
/// - Bit  7 — VAG_POWERUP
/// - Bit  6 — ADC_MONO
/// - Bit  5 — REFTOP_POWERUP
/// - Bit  4 — HEADPHONE_POWERUP
/// - Bit  3 — DAC_POWERUP
/// - Bit  2 — CAPLESS_HEADPHONE_POWERUP
/// - Bit  1 — ADC_POWERUP
/// - Bit  0 — LINEOUT_POWERUP
pub const CHIP_ANA_POWER: u16 = 0x0030;

// ── Status and test ────────────────────────────────────────────────────────

/// Analog status (read-only).
/// - Bit 9 — LRSHORT_STS
/// - Bit 8 — CSHORT_STS
/// - Bit 4 — PLL_IS_LOCKED
pub const CHIP_ANA_STATUS: u16 = 0x0036;

/// Analog test registers (debug only).
pub const CHIP_ANA_TEST1: u16 = 0x0038;
pub const CHIP_ANA_TEST2: u16 = 0x003A;

// ── Short-circuit protection ───────────────────────────────────────────────

/// Short-circuit detection control.
/// - Bits 14:12 — LVLADJR (right HP short threshold, 25 mA steps)
/// - Bits 10:8  — LVLADJL (left HP short threshold)
/// - Bits  6:4  — LVLADJC (center short threshold, 50 mA steps)
/// - Bits  3:2  — MODE_LR (0=disable, 1=auto-reset, 3=manual-reset)
/// - Bits  1:0  — MODE_CM
pub const CHIP_SHORT_CTRL: u16 = 0x003C;

// ── Digital Audio Processor (DAP) ──────────────────────────────────────────

/// DAP master enable.
pub const DAP_CONTROL: u16 = 0x0100;

/// Parametric EQ filter count.
pub const DAP_PEQ: u16 = 0x0102;

/// Bass enhancement enable/config.
pub const DAP_BASS_ENHANCE: u16 = 0x0104;

/// Bass enhancement level control.
pub const DAP_BASS_ENHANCE_CTRL: u16 = 0x0106;

/// Audio EQ mode select (0=off, 1=PEQ, 2=tone, 3=graphic).
pub const DAP_AUDIO_EQ: u16 = 0x0108;

/// Surround-sound control.
pub const DAP_SGTL_SURROUND: u16 = 0x010A;

/// Filter coefficient access control.
pub const DAP_FILTER_COEF_ACCESS: u16 = 0x010C;

/// Biquad coefficient write registers.
pub const DAP_COEF_WR_B0_MSB: u16 = 0x010E;
pub const DAP_COEF_WR_B0_LSB: u16 = 0x0110;

/// 5-band graphic EQ band registers (read/write).
pub const DAP_AUDIO_EQ_BASS_BAND0: u16 = 0x0116; // 115 Hz
pub const DAP_AUDIO_EQ_BAND1: u16 = 0x0118;       // 330 Hz
pub const DAP_AUDIO_EQ_BAND2: u16 = 0x011A;       // 990 Hz
pub const DAP_AUDIO_EQ_BAND3: u16 = 0x011C;       // 3000 Hz
pub const DAP_AUDIO_EQ_TREBLE_BAND4: u16 = 0x011E; // 9900 Hz

/// DAP main and mix channel volume.
pub const DAP_MAIN_CHAN: u16 = 0x0120;
pub const DAP_MIX_CHAN: u16 = 0x0122;

/// Auto Volume Control registers.
pub const DAP_AVC_CTRL: u16 = 0x0124;
pub const DAP_AVC_THRESHOLD: u16 = 0x0126;
pub const DAP_AVC_ATTACK: u16 = 0x0128;
pub const DAP_AVC_DECAY: u16 = 0x012A;

/// Additional biquad coefficient write registers.
pub const DAP_COEF_WR_B1_MSB: u16 = 0x012C;
pub const DAP_COEF_WR_B1_LSB: u16 = 0x012E;
pub const DAP_COEF_WR_B2_MSB: u16 = 0x0130;
pub const DAP_COEF_WR_B2_LSB: u16 = 0x0132;
pub const DAP_COEF_WR_A1_MSB: u16 = 0x0134;
pub const DAP_COEF_WR_A1_LSB: u16 = 0x0136;
pub const DAP_COEF_WR_A2_MSB: u16 = 0x0138;
pub const DAP_COEF_WR_A2_LSB: u16 = 0x013A;
