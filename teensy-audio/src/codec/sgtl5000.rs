//! SGTL5000 audio codec driver.
//!
//! Full-featured driver for the NXP SGTL5000 codec on the Teensy Audio Shield
//! (Rev A–D), ported from the C++ `AudioControlSGTL5000` class (~1075 lines).
//!
//! The driver is generic over any [`embedded_hal::i2c::I2c`] and
//! [`embedded_hal::delay::DelayNs`] implementation.
//!
//! # Example
//!
//! ```ignore
//! let mut codec = Sgtl5000::new(i2c, delay);
//! codec.enable()?;           // Full power-on sequence with 400 ms ramp
//! codec.volume(0.6)?;        // Set headphone volume
//! codec.input_select(Input::LineIn)?;
//! ```

use embedded_hal::delay::DelayNs;
use embedded_hal::i2c::I2c;

use super::registers as reg;
use crate::control::AudioControl;

// ── Public enums ───────────────────────────────────────────────────────────

/// ADC input selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// Stereo line-in input (+7.5 dB default gain).
    LineIn,
    /// Microphone input (+40 dB default preamp gain).
    Mic,
}

/// Headphone routing source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadphoneSource {
    /// Route DAC output to headphones.
    Dac,
    /// Route line-in directly to headphones (bypass DAC).
    LineIn,
}

/// EQ mode selection for the Digital Audio Processor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EqMode {
    /// No EQ processing.
    Off = 0,
    /// 7-band parametric EQ (IIR biquad filters).
    ParametricEq = 1,
    /// 2-band tone control (bass + treble).
    ToneControls = 2,
    /// 5-band graphic EQ.
    GraphicEq = 3,
}

// ── Driver struct ──────────────────────────────────────────────────────────

/// SGTL5000 audio codec driver.
///
/// Generic over I2C bus and delay provider. The delay is used only during
/// the power-on sequence (400 ms analog power ramp).
pub struct Sgtl5000<I2C, D> {
    i2c: I2C,
    delay: D,
    address: u8,
    /// Cached CHIP_ANA_CTRL value for fast mute/select operations.
    ana_ctrl: u16,
    /// Whether headphone output is currently muted.
    muted: bool,
    /// Whether the driver auto-configures DAP/EQ modes.
    semi_automated: bool,
}

impl<I2C, D> Sgtl5000<I2C, D>
where
    I2C: I2c,
    D: DelayNs,
{
    /// Default I2C address (CTRL_ADR0_CS pin low).
    pub const DEFAULT_ADDRESS: u8 = reg::I2C_ADDR_CS_LOW;

    /// Alternate I2C address (CTRL_ADR0_CS pin high).
    pub const ALT_ADDRESS: u8 = reg::I2C_ADDR_CS_HIGH;

    /// Create a new driver with the default I2C address (0x0A).
    pub fn new(i2c: I2C, delay: D) -> Self {
        Self {
            i2c,
            delay,
            address: Self::DEFAULT_ADDRESS,
            ana_ctrl: 0,
            muted: true,
            semi_automated: false,
        }
    }

    /// Create a new driver with a specific I2C address.
    pub fn new_with_address(i2c: I2C, delay: D, address: u8) -> Self {
        Self {
            i2c,
            delay,
            address,
            ana_ctrl: 0,
            muted: true,
            semi_automated: false,
        }
    }

    // ── Low-level I2C helpers ──────────────────────────────────────────

    /// Write a 16-bit value to a 16-bit register.
    pub fn write_register(&mut self, register: u16, value: u16) -> Result<(), I2C::Error> {
        // Cache writes to ANA_CTRL for fast read-modify-write
        if register == reg::CHIP_ANA_CTRL {
            self.ana_ctrl = value;
        }
        let buf = [
            (register >> 8) as u8,
            register as u8,
            (value >> 8) as u8,
            value as u8,
        ];
        self.i2c.write(self.address, &buf)
    }

    /// Read a 16-bit value from a 16-bit register.
    pub fn read_register(&mut self, register: u16) -> Result<u16, I2C::Error> {
        let reg_buf = [(register >> 8) as u8, register as u8];
        let mut val_buf = [0u8; 2];
        self.i2c.write_read(self.address, &reg_buf, &mut val_buf)?;
        Ok(((val_buf[0] as u16) << 8) | val_buf[1] as u16)
    }

    /// Read-modify-write: `new = (current & ~mask) | value`.
    fn modify(&mut self, register: u16, value: u16, mask: u16) -> Result<u16, I2C::Error> {
        let current = self.read_register(register)?;
        let new_val = (current & !mask) | value;
        self.write_register(register, new_val)?;
        Ok(new_val)
    }

    // ── Power-on sequence ──────────────────────────────────────────────

    /// Full power-on sequence for I2S slave mode at 44.1 kHz.
    ///
    /// Configures the codec with:
    /// - 44.1 kHz sample rate, 256×Fs MCLK
    /// - 16-bit I2S format, SCLK = 64×Fs
    /// - ADC → I2S output, I2S input → DAC routing
    /// - Zero-cross detection enabled
    /// - Headphone volume at minimum (call [`volume()`](Self::volume) to unmute)
    ///
    /// Includes a 400 ms delay for the analog power ramp.
    pub fn enable(&mut self) -> Result<(), I2C::Error> {
        self.delay.delay_ms(5);
        self.muted = true;

        // VDDD is externally driven with 1.8V
        self.write_register(reg::CHIP_ANA_POWER, 0x4060)?;
        // VDDA & VDDIO both over 3.1V
        self.write_register(reg::CHIP_LINREG_CTRL, 0x006C)?;
        // VAG=1.575V, normal ramp, +12.5% bias current
        self.write_register(reg::CHIP_REF_CTRL, 0x01F2)?;
        // LO_VAGCNTRL=1.65V, OUT_CURRENT=0.54mA
        self.write_register(reg::CHIP_LINE_OUT_CTRL, 0x0F22)?;
        // Short circuit protection: allow up to 125mA
        self.write_register(reg::CHIP_SHORT_CTRL, 0x4446)?;
        // Enable zero cross detectors
        self.write_register(reg::CHIP_ANA_CTRL, 0x0137)?;

        // Power up: lineout, hp, adc, dac (slave mode)
        self.write_register(reg::CHIP_ANA_POWER, 0x40FF)?;
        // Power up all digital blocks
        self.write_register(reg::CHIP_DIG_POWER, 0x0073)?;

        // Wait for analog power ramp
        self.delay.delay_ms(400);

        // Default ~1.3Vpp line output
        self.write_register(reg::CHIP_LINE_OUT_VOL, 0x1D1D)?;
        // 44.1 kHz, 256×Fs
        self.write_register(reg::CHIP_CLK_CTRL, 0x0004)?;
        // SCLK=64×Fs, 16-bit, I2S format
        self.write_register(reg::CHIP_I2S_CTRL, 0x0030)?;
        // ADC → I2S output, I2S input → DAC
        self.write_register(reg::CHIP_SSS_CTRL, 0x0010)?;
        // Disable DAC mute
        self.write_register(reg::CHIP_ADCDAC_CTRL, 0x0000)?;
        // DAC digital volume = 0 dB
        self.write_register(reg::CHIP_DAC_VOL, 0x3C3C)?;
        // Headphone volume at minimum (−51.5 dB)
        self.write_register(reg::CHIP_ANA_HP_CTRL, 0x7F7F)?;
        // Enable zero-cross detectors, select LINEIN, unmute ADC
        self.write_register(reg::CHIP_ANA_CTRL, 0x0036)?;

        self.semi_automated = true;
        Ok(())
    }

    /// Power-on with external MCLK and PLL (master mode).
    ///
    /// The SGTL5000 will generate I2S_LRCLK and I2S_SCLK using its PLL.
    ///
    /// * `ext_mclk` — External MCLK frequency in Hz.
    /// * `pll_freq` — Desired PLL output frequency, typically `4096 × Fs`.
    pub fn enable_with_pll(
        &mut self,
        ext_mclk: u32,
        pll_freq: u32,
    ) -> Result<(), I2C::Error> {
        self.delay.delay_ms(5);

        // Check if already initialized (recovery from Teensy reset)
        let i2s_ctrl = self.read_register(reg::CHIP_I2S_CTRL)?;
        if i2s_ctrl == (0x0030 | (1 << 7)) {
            self.muted = false;
            self.semi_automated = true;
            return Ok(());
        }

        self.muted = true;

        self.write_register(reg::CHIP_ANA_POWER, 0x4060)?;
        self.write_register(reg::CHIP_LINREG_CTRL, 0x006C)?;
        self.write_register(reg::CHIP_REF_CTRL, 0x01F2)?;
        self.write_register(reg::CHIP_LINE_OUT_CTRL, 0x0F22)?;
        self.write_register(reg::CHIP_SHORT_CTRL, 0x4446)?;
        self.write_register(reg::CHIP_ANA_CTRL, 0x0137)?;

        // Divide MCLK by 2 if above 17 MHz
        if ext_mclk > 17_000_000 {
            self.write_register(reg::CHIP_CLK_TOP_CTRL, 1)?;
        } else {
            self.write_register(reg::CHIP_CLK_TOP_CTRL, 0)?;
        }

        // Configure PLL dividers
        let int_divisor = (pll_freq / ext_mclk) & 0x1F;
        let frac_part = (pll_freq as f32 / ext_mclk as f32) - int_divisor as f32;
        let frac_divisor = (frac_part * 2048.0) as u32 & 0x7FF;
        self.write_register(
            reg::CHIP_PLL_CTRL,
            ((int_divisor << 11) | frac_divisor) as u16,
        )?;

        // Power up with PLL and VCO amp enabled
        self.write_register(reg::CHIP_ANA_POWER, 0x40FF | (1 << 10) | (1 << 8))?;
        self.write_register(reg::CHIP_DIG_POWER, 0x0073)?;

        self.delay.delay_ms(400);

        self.write_register(reg::CHIP_LINE_OUT_VOL, 0x1D1D)?;
        // 44.1 kHz, 256×Fs, use PLL
        self.write_register(reg::CHIP_CLK_CTRL, 0x0004 | 0x03)?;
        // SCLK=64×Fs, 16-bit, I2S format, master mode
        self.write_register(reg::CHIP_I2S_CTRL, 0x0030 | (1 << 7))?;

        self.write_register(reg::CHIP_SSS_CTRL, 0x0010)?;
        self.write_register(reg::CHIP_ADCDAC_CTRL, 0x0000)?;
        self.write_register(reg::CHIP_DAC_VOL, 0x3C3C)?;
        self.write_register(reg::CHIP_ANA_HP_CTRL, 0x7F7F)?;
        self.write_register(reg::CHIP_ANA_CTRL, 0x0036)?;

        self.semi_automated = true;
        Ok(())
    }

    /// Disable the codec (no-op, matching C++ behaviour).
    pub fn disable(&mut self) -> Result<(), I2C::Error> {
        Ok(())
    }

    // ── Headphone volume ───────────────────────────────────────────────

    /// Set headphone volume (0.0 = silent/muted, 1.0 = maximum +12 dB).
    ///
    /// Setting to 0.0 mutes the output. Any non-zero value auto-unmutes.
    pub fn volume(&mut self, level: f32) -> Result<(), I2C::Error> {
        let n = (level * 129.0 + 0.499) as u32;
        self.volume_integer(n)
    }

    /// Set headphone volume independently for left and right channels
    /// (0.0 = silent, 1.0 = maximum).
    pub fn volume_lr(&mut self, left: f32, right: f32) -> Result<(), I2C::Error> {
        let l = 0x7F - Self::calc_vol(left, 0x7F);
        let r = 0x7F - Self::calc_vol(right, 0x7F);
        let val = ((r as u16) << 8) | l as u16;
        self.write_register(reg::CHIP_ANA_HP_CTRL, val)
    }

    fn volume_integer(&mut self, n: u32) -> Result<(), I2C::Error> {
        if n == 0 {
            self.muted = true;
            self.write_register(reg::CHIP_ANA_HP_CTRL, 0x7F7F)?;
            return self.mute_headphone();
        }
        let n = if n > 0x80 { 0 } else { 0x80 - n };
        if self.muted {
            self.muted = false;
            self.unmute_headphone()?;
        }
        let val = (n | (n << 8)) as u16;
        self.write_register(reg::CHIP_ANA_HP_CTRL, val)
    }

    // ── Mute / unmute ──────────────────────────────────────────────────

    /// Mute the headphone output (sets MUTE_HP bit in ANA_CTRL).
    pub fn mute_headphone(&mut self) -> Result<(), I2C::Error> {
        self.write_register(reg::CHIP_ANA_CTRL, self.ana_ctrl | (1 << 4))
    }

    /// Unmute the headphone output (clears MUTE_HP bit in ANA_CTRL).
    pub fn unmute_headphone(&mut self) -> Result<(), I2C::Error> {
        self.write_register(reg::CHIP_ANA_CTRL, self.ana_ctrl & !(1 << 4))
    }

    /// Mute the line output (sets MUTE_LO bit in ANA_CTRL).
    pub fn mute_lineout(&mut self) -> Result<(), I2C::Error> {
        self.write_register(reg::CHIP_ANA_CTRL, self.ana_ctrl | (1 << 8))
    }

    /// Unmute the line output (clears MUTE_LO bit in ANA_CTRL).
    pub fn unmute_lineout(&mut self) -> Result<(), I2C::Error> {
        self.write_register(reg::CHIP_ANA_CTRL, self.ana_ctrl & !(1 << 8))
    }

    // ── Input / output selection ───────────────────────────────────────

    /// Select the ADC input source.
    ///
    /// * [`Input::LineIn`] — Sets +7.5 dB gain, selects line-in (CHIP_ANA_CTRL bit 2).
    /// * [`Input::Mic`] — Sets +40 dB preamp + 12 dB gain, selects mic.
    pub fn input_select(&mut self, input: Input) -> Result<(), I2C::Error> {
        match input {
            Input::LineIn => {
                // +7.5 dB gain (1.3Vp-p full scale)
                self.write_register(reg::CHIP_ANA_ADC_CTRL, 0x055)?;
                // SELECT_ADC = 1 → LINEIN
                self.write_register(reg::CHIP_ANA_CTRL, self.ana_ctrl | (1 << 2))
            }
            Input::Mic => {
                // Mic preamp gain = +40 dB
                self.write_register(reg::CHIP_MIC_CTRL, 0x0173)?;
                // Input gain +12 dB
                self.write_register(reg::CHIP_ANA_ADC_CTRL, 0x088)?;
                // SELECT_ADC = 0 → Microphone
                self.write_register(reg::CHIP_ANA_CTRL, self.ana_ctrl & !(1 << 2))
            }
        }
    }

    /// Select the headphone input source.
    ///
    /// Matches C++ `headphoneSelect()` behaviour for SELECT_HP bit.
    pub fn headphone_select(&mut self, source: HeadphoneSource) -> Result<(), I2C::Error> {
        match source {
            HeadphoneSource::Dac => {
                self.write_register(reg::CHIP_ANA_CTRL, self.ana_ctrl | (1 << 6))
            }
            HeadphoneSource::LineIn => {
                self.write_register(reg::CHIP_ANA_CTRL, self.ana_ctrl & !(1 << 6))
            }
        }
    }

    // ── Line levels ────────────────────────────────────────────────────

    /// Set line-in input level (0–15 per channel, 1.5 dB steps).
    pub fn line_in_level(&mut self, left: u8, right: u8) -> Result<(), I2C::Error> {
        let l = left.min(15);
        let r = right.min(15);
        self.write_register(reg::CHIP_ANA_ADC_CTRL, ((l as u16) << 4) | r as u16)
    }

    /// Set line-out output level (13–31 per channel, 0.5 dB steps).
    ///
    /// Values below 13 cause clipping; they are clamped internally.
    pub fn line_out_level(&mut self, left: u8, right: u8) -> Result<(), I2C::Error> {
        let l = left.clamp(13, 31);
        let r = right.clamp(13, 31);
        self.modify(
            reg::CHIP_LINE_OUT_VOL,
            ((r as u16) << 8) | l as u16,
            (31 << 8) | 31,
        )?;
        Ok(())
    }

    /// Set microphone preamp gain (0–63 dB).
    ///
    /// Gain is split between the mic preamp (0/20/30/40 dB) and the
    /// ADC analog input gain (0–22.5 dB in 1.5 dB steps).
    pub fn mic_gain(&mut self, db: u32) -> Result<(), I2C::Error> {
        let (preamp_gain, remaining) = if db >= 40 {
            (3u16, db - 40)
        } else if db >= 30 {
            (2, db - 30)
        } else if db >= 20 {
            (1, db - 20)
        } else {
            (0, db)
        };
        let input_gain = ((remaining * 2) / 3).min(15) as u16;

        self.write_register(reg::CHIP_MIC_CTRL, 0x0170 | preamp_gain)?;
        self.write_register(
            reg::CHIP_ANA_ADC_CTRL,
            (input_gain << 4) | input_gain,
        )
    }

    // ── DAC volume ─────────────────────────────────────────────────────

    /// Set DAC digital volume for both channels (0.0 = muted, 1.0 = 0 dB).
    ///
    /// Resolution is ~0.5 dB over a 90 dB range.
    pub fn dac_volume(&mut self, left: f32, right: f32) -> Result<(), I2C::Error> {
        let mute_bits =
            ((if right > 0.0 { 0u16 } else { 2 }) | (if left > 0.0 { 0 } else { 1 })) << 2;
        let current = self.read_register(reg::CHIP_ADCDAC_CTRL)?;
        if (current & (3 << 2)) != mute_bits {
            self.modify(reg::CHIP_ADCDAC_CTRL, mute_bits, 3 << 2)?;
        }
        let l = 0xFC - Self::calc_vol(left, 0xC0);
        let r = 0xFC - Self::calc_vol(right, 0xC0);
        self.modify(
            reg::CHIP_DAC_VOL,
            ((r as u16) << 8) | l as u16,
            0xFFFF,
        )?;
        Ok(())
    }

    /// Enable exponential DAC volume ramp (soft transitions).
    pub fn dac_volume_ramp(&mut self) -> Result<(), I2C::Error> {
        self.modify(reg::CHIP_ADCDAC_CTRL, 0x300, 0x300)?;
        Ok(())
    }

    /// Enable linear DAC volume ramp.
    pub fn dac_volume_ramp_linear(&mut self) -> Result<(), I2C::Error> {
        self.modify(reg::CHIP_ADCDAC_CTRL, 0x200, 0x300)?;
        Ok(())
    }

    /// Disable DAC volume ramp (immediate volume changes).
    pub fn dac_volume_ramp_disable(&mut self) -> Result<(), I2C::Error> {
        self.modify(reg::CHIP_ADCDAC_CTRL, 0, 0x300)?;
        Ok(())
    }

    // ── ADC high-pass filter ───────────────────────────────────────────

    /// Enable the ADC high-pass filter (normal operation).
    pub fn adc_high_pass_filter_enable(&mut self) -> Result<(), I2C::Error> {
        self.modify(reg::CHIP_ADCDAC_CTRL, 0, 3)?;
        Ok(())
    }

    /// Freeze the ADC high-pass filter offset register.
    pub fn adc_high_pass_filter_freeze(&mut self) -> Result<(), I2C::Error> {
        self.modify(reg::CHIP_ADCDAC_CTRL, 2, 3)?;
        Ok(())
    }

    /// Bypass the ADC high-pass filter.
    pub fn adc_high_pass_filter_disable(&mut self) -> Result<(), I2C::Error> {
        self.modify(reg::CHIP_ADCDAC_CTRL, 1, 3)?;
        Ok(())
    }

    // ── Digital Audio Processor (DAP) ──────────────────────────────────

    /// Enable audio pre-processing (analog input → DAP → Teensy).
    pub fn audio_pre_processor_enable(&mut self) -> Result<(), I2C::Error> {
        self.write_register(reg::DAP_CONTROL, 1)?;
        self.write_register(reg::CHIP_SSS_CTRL, 0x0013)
    }

    /// Enable audio post-processing (Teensy → DAP → output).
    pub fn audio_post_processor_enable(&mut self) -> Result<(), I2C::Error> {
        self.write_register(reg::DAP_CONTROL, 1)?;
        self.write_register(reg::CHIP_SSS_CTRL, 0x0070)
    }

    /// Disable the audio processor and restore default routing.
    pub fn audio_processor_disable(&mut self) -> Result<(), I2C::Error> {
        self.write_register(reg::CHIP_SSS_CTRL, 0x0010)?;
        self.write_register(reg::DAP_CONTROL, 0)
    }

    // ── Equalizer ──────────────────────────────────────────────────────

    /// Set the number of active PEQ filters (0–7).
    pub fn eq_filter_count(&mut self, n: u8) -> Result<(), I2C::Error> {
        self.modify(reg::DAP_PEQ, (n & 7) as u16, 7)?;
        Ok(())
    }

    /// Select the EQ processing mode.
    pub fn eq_select(&mut self, mode: EqMode) -> Result<(), I2C::Error> {
        self.modify(reg::DAP_AUDIO_EQ, mode as u16 & 3, 3)?;
        Ok(())
    }

    /// Set a single EQ band level (−1.0 to +1.0, 0.0 = flat).
    ///
    /// Band indices: 0 = 115 Hz, 1 = 330 Hz, 2 = 990 Hz, 3 = 3 kHz, 4 = 9.9 kHz.
    pub fn eq_band(&mut self, band: u8, level: f32) -> Result<(), I2C::Error> {
        if self.semi_automated {
            self.automate(1, 3)?;
        }
        self.dap_audio_eq_band(band, level)
    }

    /// Set all 5 graphic EQ bands (each −1.0 to +1.0).
    pub fn eq_bands_5(
        &mut self,
        bass: f32,
        mid_bass: f32,
        mid: f32,
        mid_treble: f32,
        treble: f32,
    ) -> Result<(), I2C::Error> {
        if self.semi_automated {
            self.automate(1, 3)?;
        }
        self.dap_audio_eq_band(0, bass)?;
        self.dap_audio_eq_band(1, mid_bass)?;
        self.dap_audio_eq_band(2, mid)?;
        self.dap_audio_eq_band(3, mid_treble)?;
        self.dap_audio_eq_band(4, treble)
    }

    /// Set bass and treble (2-band tone control, each −1.0 to +1.0).
    pub fn eq_bands_2(&mut self, bass: f32, treble: f32) -> Result<(), I2C::Error> {
        if self.semi_automated {
            self.automate(1, 2)?;
        }
        self.dap_audio_eq_band(0, bass)?;
        self.dap_audio_eq_band(4, treble)
    }

    /// Load raw biquad filter coefficients into a PEQ slot (0–6).
    ///
    /// `coefficients` must be `[b0, b1, b2, a1, a2]`.
    pub fn eq_filter(
        &mut self,
        filter_num: u8,
        coefficients: &[i32; 5],
    ) -> Result<(), I2C::Error> {
        if self.semi_automated {
            self.automate_with_filter_count(1, 1, filter_num + 1)?;
        }
        self.modify(reg::DAP_FILTER_COEF_ACCESS, filter_num as u16, 15)?;

        self.write_register(reg::DAP_COEF_WR_B0_MSB, (coefficients[0] >> 4) as u16)?;
        self.write_register(reg::DAP_COEF_WR_B0_LSB, (coefficients[0] & 15) as u16)?;
        self.write_register(reg::DAP_COEF_WR_B1_MSB, (coefficients[1] >> 4) as u16)?;
        self.write_register(reg::DAP_COEF_WR_B1_LSB, (coefficients[1] & 15) as u16)?;
        self.write_register(reg::DAP_COEF_WR_B2_MSB, (coefficients[2] >> 4) as u16)?;
        self.write_register(reg::DAP_COEF_WR_B2_LSB, (coefficients[2] & 15) as u16)?;
        self.write_register(reg::DAP_COEF_WR_A1_MSB, (coefficients[3] >> 4) as u16)?;
        self.write_register(reg::DAP_COEF_WR_A1_LSB, (coefficients[3] & 15) as u16)?;
        self.write_register(reg::DAP_COEF_WR_A2_MSB, (coefficients[4] >> 4) as u16)?;
        self.write_register(reg::DAP_COEF_WR_A2_LSB, (coefficients[4] & 15) as u16)?;

        self.write_register(reg::DAP_FILTER_COEF_ACCESS, 0x100 | filter_num as u16)
    }

    // ── Surround sound ─────────────────────────────────────────────────

    /// Set surround sound width (0–7).
    pub fn surround_sound(&mut self, width: u8) -> Result<(), I2C::Error> {
        self.modify(
            reg::DAP_SGTL_SURROUND,
            ((width & 7) as u16) << 4,
            7 << 4,
        )?;
        Ok(())
    }

    /// Set surround sound width and select mode.
    pub fn surround_sound_with_select(
        &mut self,
        width: u8,
        select: u8,
    ) -> Result<(), I2C::Error> {
        self.modify(
            reg::DAP_SGTL_SURROUND,
            (((width & 7) as u16) << 4) | (select & 3) as u16,
            (7 << 4) | 3,
        )?;
        Ok(())
    }

    /// Enable surround sound processing.
    pub fn surround_sound_enable(&mut self) -> Result<(), I2C::Error> {
        self.modify(reg::DAP_SGTL_SURROUND, 3, 3)?;
        Ok(())
    }

    /// Disable surround sound processing.
    pub fn surround_sound_disable(&mut self) -> Result<(), I2C::Error> {
        self.modify(reg::DAP_SGTL_SURROUND, 0, 3)?;
        Ok(())
    }

    // ── Bass enhance ───────────────────────────────────────────────────

    /// Set bass enhancement levels (each 0.0–1.0).
    pub fn enhance_bass(&mut self, lr_level: f32, bass_level: f32) -> Result<(), I2C::Error> {
        let lr = (0x3F - Self::calc_vol(lr_level, 0x3F)) as u16;
        let bass = (0x7F - Self::calc_vol(bass_level, 0x7F)) as u16;
        self.modify(
            reg::DAP_BASS_ENHANCE_CTRL,
            (lr << 8) | bass,
            (0x3F << 8) | 0x7F,
        )?;
        Ok(())
    }

    /// Set bass enhancement with HPF bypass and cutoff configuration.
    pub fn enhance_bass_with_config(
        &mut self,
        lr_level: f32,
        bass_level: f32,
        hpf_bypass: bool,
        cutoff: u8,
    ) -> Result<(), I2C::Error> {
        self.modify(
            reg::DAP_BASS_ENHANCE,
            ((hpf_bypass as u16) << 8) | (((cutoff & 7) as u16) << 4),
            (1 << 8) | (7 << 4),
        )?;
        self.enhance_bass(lr_level, bass_level)
    }

    /// Enable bass enhancement.
    pub fn enhance_bass_enable(&mut self) -> Result<(), I2C::Error> {
        self.modify(reg::DAP_BASS_ENHANCE, 1, 1)?;
        Ok(())
    }

    /// Disable bass enhancement.
    pub fn enhance_bass_disable(&mut self) -> Result<(), I2C::Error> {
        self.modify(reg::DAP_BASS_ENHANCE, 0, 1)?;
        Ok(())
    }

    // ── Automation control ─────────────────────────────────────────────

    /// Stop automatic DAP/EQ mode management.
    pub fn kill_automation(&mut self) {
        self.semi_automated = false;
    }

    // ── Release ────────────────────────────────────────────────────────

    /// Consume the driver and return the I2C bus and delay.
    pub fn release(self) -> (I2C, D) {
        (self.i2c, self.delay)
    }

    // ── Private helpers ────────────────────────────────────────────────

    /// Convert a float level (0.0–1.0) to an integer in range 0..=range.
    fn calc_vol(n: f32, range: u8) -> u8 {
        let v = n * range as f32 + 0.499;
        if v < 0.0 {
            return 0;
        }
        let vi = v as u8;
        if vi > range {
            range
        } else {
            vi
        }
    }

    /// Write a single DAP EQ band value (maps ±1.0 to 0–95 register range).
    fn dap_audio_eq_band(&mut self, band: u8, level: f32) -> Result<(), I2C::Error> {
        let mut n = level * 48.0 + 0.499;
        if n < -47.0 {
            n = -47.0;
        }
        if n > 48.0 {
            n = 48.0;
        }
        n += 47.0;
        let addr = reg::DAP_AUDIO_EQ_BASS_BAND0 + (band as u16) * 2;
        self.modify(addr, n as u16, 127)?;
        Ok(())
    }

    /// Auto-select EQ mode if it differs from the requested mode.
    fn automate(&mut self, _dap: u8, eq: u8) -> Result<(), I2C::Error> {
        let current_eq = self.read_register(reg::DAP_AUDIO_EQ)? & 3;
        if current_eq != eq as u16 {
            let mode = match eq {
                0 => EqMode::Off,
                1 => EqMode::ParametricEq,
                2 => EqMode::ToneControls,
                _ => EqMode::GraphicEq,
            };
            self.eq_select(mode)?;
        }
        Ok(())
    }

    /// Auto-select EQ mode and ensure minimum filter count.
    fn automate_with_filter_count(
        &mut self,
        dap: u8,
        eq: u8,
        filter_count: u8,
    ) -> Result<(), I2C::Error> {
        self.automate(dap, eq)?;
        let current_count = self.read_register(reg::DAP_PEQ)? & 7;
        if (filter_count as u16) > current_count {
            self.eq_filter_count(filter_count)?;
        }
        Ok(())
    }
}

// ── AudioControl trait implementation ──────────────────────────────────────

impl<I2C, D> AudioControl for Sgtl5000<I2C, D>
where
    I2C: I2c,
    D: DelayNs,
{
    type Error = I2C::Error;

    fn enable(&mut self) -> Result<(), Self::Error> {
        // Delegate to the inherent method
        Sgtl5000::enable(self)
    }

    fn disable(&mut self) -> Result<(), Self::Error> {
        Sgtl5000::disable(self)
    }

    fn volume(&mut self, level: f32) -> Result<(), Self::Error> {
        Sgtl5000::volume(self, level)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_hal::delay::DelayNs;
    use embedded_hal::i2c::{self, ErrorType, I2c, Operation};

    // ── Mock I2C with register file ───────────────────────────────────

    #[derive(Debug)]
    struct MockError;

    impl i2c::Error for MockError {
        fn kind(&self) -> i2c::ErrorKind {
            i2c::ErrorKind::Other
        }
    }

    /// Mock I2C that maintains a register file and records writes.
    struct MockI2c {
        /// Register file: (address, value) pairs.
        regs: [(u16, u16); 128],
        reg_count: usize,
        /// Write log in chronological order.
        log: [(u16, u16); 128],
        log_count: usize,
    }

    impl MockI2c {
        fn new() -> Self {
            Self {
                regs: [(0, 0); 128],
                reg_count: 0,
                log: [(0, 0); 128],
                log_count: 0,
            }
        }

        /// Look up current register value, returning 0 if never written.
        fn read_reg(&self, addr: u16) -> u16 {
            for i in 0..self.reg_count {
                if self.regs[i].0 == addr {
                    return self.regs[i].1;
                }
            }
            0
        }

        /// Set a register value (update or insert).
        fn set_reg(&mut self, addr: u16, val: u16) {
            for i in 0..self.reg_count {
                if self.regs[i].0 == addr {
                    self.regs[i].1 = val;
                    return;
                }
            }
            self.regs[self.reg_count] = (addr, val);
            self.reg_count += 1;
        }

        /// Get the (register, value) of the nth write.
        fn write_at(&self, idx: usize) -> (u16, u16) {
            self.log[idx]
        }
    }

    impl ErrorType for MockI2c {
        type Error = MockError;
    }

    impl I2c for MockI2c {
        fn read(&mut self, _addr: u8, _buf: &mut [u8]) -> Result<(), Self::Error> {
            Ok(())
        }

        fn write(&mut self, _addr: u8, bytes: &[u8]) -> Result<(), Self::Error> {
            if bytes.len() == 4 {
                let reg = ((bytes[0] as u16) << 8) | bytes[1] as u16;
                let val = ((bytes[2] as u16) << 8) | bytes[3] as u16;
                self.set_reg(reg, val);
                self.log[self.log_count] = (reg, val);
                self.log_count += 1;
            }
            Ok(())
        }

        fn write_read(
            &mut self,
            _addr: u8,
            wr: &[u8],
            rd: &mut [u8],
        ) -> Result<(), Self::Error> {
            if wr.len() >= 2 && rd.len() >= 2 {
                let reg = ((wr[0] as u16) << 8) | wr[1] as u16;
                let val = self.read_reg(reg);
                rd[0] = (val >> 8) as u8;
                rd[1] = val as u8;
            }
            Ok(())
        }

        fn transaction(
            &mut self,
            _addr: u8,
            _ops: &mut [Operation<'_>],
        ) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    // ── Mock delay (no-op) ────────────────────────────────────────────

    struct MockDelay;

    impl DelayNs for MockDelay {
        fn delay_ns(&mut self, _ns: u32) {}
    }

    // ── Helpers ───────────────────────────────────────────────────────

    fn make_codec() -> Sgtl5000<MockI2c, MockDelay> {
        Sgtl5000::new(MockI2c::new(), MockDelay)
    }

    fn enabled_codec() -> Sgtl5000<MockI2c, MockDelay> {
        let mut c = make_codec();
        c.enable().unwrap();
        c
    }

    // ── Power-on tests ────────────────────────────────────────────────

    #[test]
    fn enable_writes_correct_sequence() {
        let mut codec = make_codec();
        codec.enable().unwrap();
        let (i2c, _) = codec.release();

        assert_eq!(i2c.log_count, 16);

        // Spot-check critical writes
        assert_eq!(i2c.write_at(0), (reg::CHIP_ANA_POWER, 0x4060));
        assert_eq!(i2c.write_at(1), (reg::CHIP_LINREG_CTRL, 0x006C));
        assert_eq!(i2c.write_at(5), (reg::CHIP_ANA_CTRL, 0x0137));
        assert_eq!(i2c.write_at(6), (reg::CHIP_ANA_POWER, 0x40FF));
        assert_eq!(i2c.write_at(7), (reg::CHIP_DIG_POWER, 0x0073));
        assert_eq!(i2c.write_at(8), (reg::CHIP_LINE_OUT_VOL, 0x1D1D));
        assert_eq!(i2c.write_at(9), (reg::CHIP_CLK_CTRL, 0x0004));
        assert_eq!(i2c.write_at(10), (reg::CHIP_I2S_CTRL, 0x0030));
        assert_eq!(i2c.write_at(14), (reg::CHIP_ANA_HP_CTRL, 0x7F7F));
        assert_eq!(i2c.write_at(15), (reg::CHIP_ANA_CTRL, 0x0036));
    }

    #[test]
    fn enable_caches_ana_ctrl() {
        let mut codec = make_codec();
        codec.enable().unwrap();
        // Last ANA_CTRL write is 0x0036
        assert_eq!(codec.ana_ctrl, 0x0036);
        assert!(codec.semi_automated);
    }

    // ── Volume tests ──────────────────────────────────────────────────

    #[test]
    fn volume_zero_mutes() {
        let mut codec = enabled_codec();
        codec.volume(0.0).unwrap();
        assert!(codec.muted);

        let (i2c, _) = codec.release();
        // Should write 0x7F7F to HP_CTRL, then mute bit to ANA_CTRL
        let last_hp = i2c.read_reg(reg::CHIP_ANA_HP_CTRL);
        assert_eq!(last_hp, 0x7F7F);
    }

    #[test]
    fn volume_full_scale() {
        let mut codec = enabled_codec();
        codec.volume(1.0).unwrap();
        assert!(!codec.muted);

        let (i2c, _) = codec.release();
        // n = (1.0 * 129 + 0.499) as u32 = 129; n > 0x80 → n = 0
        assert_eq!(i2c.read_reg(reg::CHIP_ANA_HP_CTRL), 0x0000);
    }

    #[test]
    fn volume_mid_range() {
        let mut codec = enabled_codec();
        codec.volume(0.5).unwrap();

        let (i2c, _) = codec.release();
        // n = (0.5 * 129 + 0.499) as u32 = 64; 0x80 - 64 = 64 = 0x40
        assert_eq!(i2c.read_reg(reg::CHIP_ANA_HP_CTRL), 0x4040);
    }

    #[test]
    fn volume_auto_unmutes() {
        let mut codec = enabled_codec();
        // After enable, muted=true
        assert!(codec.muted);
        codec.volume(0.7).unwrap();
        assert!(!codec.muted);
    }

    #[test]
    fn volume_lr_independent_channels() {
        let mut codec = enabled_codec();
        codec.volume_lr(1.0, 0.0).unwrap();

        let (i2c, _) = codec.release();
        let hp = i2c.read_reg(reg::CHIP_ANA_HP_CTRL);
        // Left = 0x7F - calc_vol(1.0, 0x7F) = 0x7F - 0x7F = 0x00
        // Right = 0x7F - calc_vol(0.0, 0x7F) = 0x7F - 0x00 = 0x7F
        assert_eq!(hp & 0x7F, 0x00); // left = max
        assert_eq!((hp >> 8) & 0x7F, 0x7F); // right = min
    }

    // ── Mute tests ────────────────────────────────────────────────────

    #[test]
    fn mute_unmute_headphone() {
        let mut codec = enabled_codec();
        // ana_ctrl after enable = 0x0036, bit 4 (MUTE_HP) is set
        codec.unmute_headphone().unwrap();
        // Should clear bit 4: 0x0036 & ~(1<<4) = 0x0026
        assert_eq!(codec.ana_ctrl, 0x0026);

        codec.mute_headphone().unwrap();
        // Should set bit 4: 0x0026 | (1<<4) = 0x0036
        assert_eq!(codec.ana_ctrl, 0x0036);
    }

    #[test]
    fn mute_unmute_lineout() {
        let mut codec = enabled_codec();
        // ana_ctrl = 0x0036, bit 8 = 0 (unmuted)
        codec.mute_lineout().unwrap();
        assert_eq!(codec.ana_ctrl & (1 << 8), 1 << 8);

        codec.unmute_lineout().unwrap();
        assert_eq!(codec.ana_ctrl & (1 << 8), 0);
    }

    // ── Input selection tests ─────────────────────────────────────────

    #[test]
    fn input_select_linein() {
        let mut codec = enabled_codec();
        codec.input_select(Input::LineIn).unwrap();

        let (i2c, _) = codec.release();
        // ADC gain for line-in
        assert_eq!(i2c.read_reg(reg::CHIP_ANA_ADC_CTRL), 0x055);
        // ANA_CTRL bit 2 set (SELECT_ADC = LINEIN)
        let ana = i2c.read_reg(reg::CHIP_ANA_CTRL);
        assert_ne!(ana & (1 << 2), 0);
    }

    #[test]
    fn input_select_mic() {
        let mut codec = enabled_codec();
        codec.input_select(Input::Mic).unwrap();

        let (i2c, _) = codec.release();
        assert_eq!(i2c.read_reg(reg::CHIP_MIC_CTRL), 0x0173);
        assert_eq!(i2c.read_reg(reg::CHIP_ANA_ADC_CTRL), 0x088);
        // ANA_CTRL bit 2 cleared (SELECT_ADC = Mic)
        let ana = i2c.read_reg(reg::CHIP_ANA_CTRL);
        assert_eq!(ana & (1 << 2), 0);
    }

    // ── Headphone select test ─────────────────────────────────────────

    #[test]
    fn headphone_select_toggles_bit() {
        let mut codec = enabled_codec();
        // After enable, ana_ctrl = 0x0036, bit 6 = 0
        codec.headphone_select(HeadphoneSource::Dac).unwrap();
        assert_ne!(codec.ana_ctrl & (1 << 6), 0);

        codec.headphone_select(HeadphoneSource::LineIn).unwrap();
        assert_eq!(codec.ana_ctrl & (1 << 6), 0);
    }

    // ── Line level tests ──────────────────────────────────────────────

    #[test]
    fn line_in_level_clamps_to_15() {
        let mut codec = enabled_codec();
        codec.line_in_level(20, 20).unwrap();

        let (i2c, _) = codec.release();
        // Clamped: (15 << 4) | 15 = 0xFF
        assert_eq!(i2c.read_reg(reg::CHIP_ANA_ADC_CTRL), 0xFF);
    }

    #[test]
    fn line_out_level_clamps_range() {
        let mut codec = enabled_codec();
        codec.line_out_level(5, 40).unwrap();

        let (i2c, _) = codec.release();
        let vol = i2c.read_reg(reg::CHIP_LINE_OUT_VOL);
        let left = vol & 0x1F;
        let right = (vol >> 8) & 0x1F;
        assert_eq!(left, 13); // clamped up from 5
        assert_eq!(right, 31); // clamped down from 40
    }

    // ── Mic gain tests ────────────────────────────────────────────────

    #[test]
    fn mic_gain_40db() {
        let mut codec = enabled_codec();
        codec.mic_gain(40).unwrap();

        let (i2c, _) = codec.release();
        // preamp = 3 (+40 dB), remaining = 0, input_gain = 0
        assert_eq!(i2c.read_reg(reg::CHIP_MIC_CTRL), 0x0170 | 3);
        assert_eq!(i2c.read_reg(reg::CHIP_ANA_ADC_CTRL), 0x000);
    }

    #[test]
    fn mic_gain_63db() {
        let mut codec = enabled_codec();
        codec.mic_gain(63).unwrap();

        let (i2c, _) = codec.release();
        // preamp = 3 (+40 dB), remaining = 23, input_gain = (23*2)/3 = 15
        assert_eq!(i2c.read_reg(reg::CHIP_MIC_CTRL), 0x0170 | 3);
        assert_eq!(i2c.read_reg(reg::CHIP_ANA_ADC_CTRL), (15 << 4) | 15);
    }

    #[test]
    fn mic_gain_25db() {
        let mut codec = enabled_codec();
        codec.mic_gain(25).unwrap();

        let (i2c, _) = codec.release();
        // preamp = 1 (+20 dB), remaining = 5, input_gain = (5*2)/3 = 3
        assert_eq!(i2c.read_reg(reg::CHIP_MIC_CTRL), 0x0170 | 1);
        assert_eq!(i2c.read_reg(reg::CHIP_ANA_ADC_CTRL), (3 << 4) | 3);
    }

    // ── DAC volume ramp tests ─────────────────────────────────────────

    #[test]
    fn dac_volume_ramp_modes() {
        let mut codec = enabled_codec();

        // Exponential ramp: bits 9:8 = 0b11
        codec.dac_volume_ramp().unwrap();
        let (i2c, _) = codec.release();
        assert_eq!(i2c.read_reg(reg::CHIP_ADCDAC_CTRL) & 0x300, 0x300);

        let mut codec = enabled_codec();
        // Linear ramp: bits 9:8 = 0b10
        codec.dac_volume_ramp_linear().unwrap();
        let (i2c, _) = codec.release();
        assert_eq!(i2c.read_reg(reg::CHIP_ADCDAC_CTRL) & 0x300, 0x200);

        let mut codec = enabled_codec();
        // Disabled: bits 9:8 = 0b00
        codec.dac_volume_ramp_disable().unwrap();
        let (i2c, _) = codec.release();
        assert_eq!(i2c.read_reg(reg::CHIP_ADCDAC_CTRL) & 0x300, 0x000);
    }

    // ── Modify (read-modify-write) test ───────────────────────────────

    #[test]
    fn modify_preserves_unmasked_bits() {
        let mut codec = enabled_codec();
        // CHIP_ADCDAC_CTRL was written as 0x0000 by enable()
        // Set bits 9:8 to 0x300 without touching other bits
        codec.dac_volume_ramp().unwrap();
        // Now set bit 0 (ADC_HPF_BYPASS) without clearing bits 9:8
        codec.adc_high_pass_filter_disable().unwrap();

        let (i2c, _) = codec.release();
        let val = i2c.read_reg(reg::CHIP_ADCDAC_CTRL);
        assert_eq!(val & 0x300, 0x300); // ramp bits preserved
        assert_eq!(val & 0x3, 1); // HPF bypass set
    }

    // ── DAP routing tests ─────────────────────────────────────────────

    #[test]
    fn audio_processor_routing() {
        let mut codec = enabled_codec();

        codec.audio_pre_processor_enable().unwrap();
        let (i2c, _) = codec.release();
        assert_eq!(i2c.read_reg(reg::DAP_CONTROL), 1);
        assert_eq!(i2c.read_reg(reg::CHIP_SSS_CTRL), 0x0013);

        let mut codec = enabled_codec();
        codec.audio_post_processor_enable().unwrap();
        let (i2c, _) = codec.release();
        assert_eq!(i2c.read_reg(reg::DAP_CONTROL), 1);
        assert_eq!(i2c.read_reg(reg::CHIP_SSS_CTRL), 0x0070);

        let mut codec = enabled_codec();
        codec.audio_post_processor_enable().unwrap();
        codec.audio_processor_disable().unwrap();
        let (i2c, _) = codec.release();
        assert_eq!(i2c.read_reg(reg::DAP_CONTROL), 0);
        assert_eq!(i2c.read_reg(reg::CHIP_SSS_CTRL), 0x0010);
    }

    // ── calc_vol helper test ──────────────────────────────────────────

    #[test]
    fn calc_vol_boundaries() {
        assert_eq!(Sgtl5000::<MockI2c, MockDelay>::calc_vol(0.0, 0x7F), 0);
        assert_eq!(Sgtl5000::<MockI2c, MockDelay>::calc_vol(1.0, 0x7F), 0x7F);
        // Mid-range: 0.5 * 127 + 0.499 = 63.999 → 63
        assert_eq!(Sgtl5000::<MockI2c, MockDelay>::calc_vol(0.5, 0x7F), 63);
    }

    // ── AudioControl trait test ───────────────────────────────────────

    #[test]
    fn audio_control_trait_delegation() {
        let mut codec = make_codec();

        // Call through the trait
        AudioControl::enable(&mut codec).unwrap();
        assert!(codec.semi_automated);

        AudioControl::volume(&mut codec, 0.8).unwrap();
        assert!(!codec.muted);

        AudioControl::disable(&mut codec).unwrap(); // no-op
    }

    // ── Address configuration test ────────────────────────────────────

    #[test]
    fn custom_address() {
        let codec = Sgtl5000::new_with_address(MockI2c::new(), MockDelay, 0x2A);
        assert_eq!(codec.address, Sgtl5000::<MockI2c, MockDelay>::ALT_ADDRESS);
    }

    // ── Release test ──────────────────────────────────────────────────

    #[test]
    fn release_returns_peripherals() {
        let codec = make_codec();
        let (_i2c, _delay) = codec.release();
        // Just verify it compiles and doesn't panic
    }
}
