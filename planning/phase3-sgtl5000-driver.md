# Phase 3: SGTL5000 Codec Driver

The SGTL5000 is the audio codec on the Teensy Audio Shield. This phase builds a proper Rust driver, replacing the ~90-line inline demo in the imxrt-hal examples with a full-featured module ported from the 1075-line C++ driver.

## 3.1 Register map

Port all ~50 register addresses and their bit field definitions from `TeensyAudio/control_sgtl5000.h` as Rust constants.

### Register categories

| Category | Registers | Purpose |
|----------|-----------|---------|
| **Chip ID** | `CHIP_ID` (0x0000) | Read-only identification |
| **Power** | `CHIP_ANA_POWER` (0x0030), `CHIP_DIG_POWER` (0x0002), `CHIP_LINREG_CTRL` (0x0026) | Power rail sequencing |
| **Clocking** | `CHIP_CLK_CTRL` (0x0004), `CHIP_CLK_TOP_CTRL` (0x0034), `CHIP_PLL_CTRL` (0x0032) | Sample rate, PLL |
| **I2S** | `CHIP_I2S_CTRL` (0x0006) | Word length, mode |
| **Routing** | `CHIP_SSS_CTRL` (0x000A) | Source select for DAC, I2S out |
| **DAC** | `CHIP_ADCDAC_CTRL` (0x000E), `CHIP_DAC_VOL` (0x0010) | DAC volume, mute |
| **ADC** | `CHIP_ANA_ADC_CTRL` (0x0020), `CHIP_MIC_CTRL` (0x002A) | ADC gain, mic bias |
| **Headphone** | `CHIP_ANA_HP_CTRL` (0x0022) | HP volume |
| **Line out** | `CHIP_LINE_OUT_CTRL` (0x002C), `CHIP_LINE_OUT_VOL` (0x002E) | Line output level |
| **Analog** | `CHIP_ANA_CTRL` (0x0024), `CHIP_REF_CTRL` (0x0028) | Mute, input select, reference |
| **Protection** | `CHIP_SHORT_CTRL` (0x003C) | Short circuit detection |
| **DAP** | `DAP_CONTROL` (0x0100), `DAP_*` (0x01xx) | Digital Audio Processor (EQ, surround, bass, AVC) |

## 3.2 Driver struct

```rust
pub struct Sgtl5000<I2C> {
    i2c: I2C,
    // Cached state for read-modify-write operations
    ana_ctrl: u16,
}

impl<I2C: embedded_hal::i2c::I2c> Sgtl5000<I2C> {
    pub fn new(i2c: I2C) -> Self { ... }
}
```

Generic over any `embedded_hal::i2c::I2c` implementation.

I2C address: `0x0A` (fixed by SGTL5000 hardware).

### I2C protocol
- 16-bit register addresses, 16-bit register values
- Write: `[addr_hi, addr_lo, val_hi, val_lo]`
- Read: write `[addr_hi, addr_lo]`, then read `[val_hi, val_lo]`

## 3.3 Core methods — AudioControl trait

| Method | Description |
|--------|-------------|
| `enable()` | Full power-on sequence (see below) |
| `disable()` | Power down codec |
| `volume(level: f32)` | Set headphone volume (0.0–1.0) using the C++ lookup table |

## 3.4 Extended methods — inherent impl

| Method | C++ equivalent | Description |
|--------|---------------|-------------|
| `input_select(input: Input)` | `inputSelect()` | Select LINEIN or MIC input |
| `line_in_level(level: u8)` | `lineInLevel()` | Set line-in gain (0–15) |
| `line_out_level(left: u8, right: u8)` | `lineOutLevel()` | Set line-out level (13–31) |
| `mic_gain(db: u8)` | `micGain()` | Set mic preamp gain (0–63 dB) |
| `dac_volume(left: f32, right: f32)` | `dacVolume()` | Set DAC volume with optional ramp |
| `adc_volume(left: f32, right: f32)` | `adcHighPassFilter...()` | ADC volume control |
| `headphone_select(source: HpSource)` | `headphoneSelect()` | Route DAC or LINEIN to headphones |
| `mute()` / `unmute()` | Read-modify-write `ANA_CTRL` bit 4 | Mute/unmute headphone output |

### Future / lower priority
- DAP (Digital Audio Processor): EQ bands, surround, bass enhance, auto volume control
- Master mode with PLL configuration
- Multiple sample rates

## 3.5 Power-on sequence

The C++ `enable()` performs a carefully ordered register write sequence with a 400ms delay. Critical to replicate exactly:

1. Set `ANA_POWER` — enable internal VDDD regulator, reference bias
2. Set `LINREG_CTRL` — configure charge pump and linear regulator
3. Set `REF_CTRL` — reference voltage ramp, bias current
4. Set `LINE_OUT_CTRL` — line output reference and bias
5. Set `SHORT_CTRL` — short circuit protection thresholds
6. Set `ANA_CTRL` — initial mute state, input selection
7. Set `ANA_POWER` — enable all analog blocks (HP amp, ADC, DAC, etc.)
8. Set `DIG_POWER` — enable digital blocks (I2S, DAP, DAC, ADC)
9. **Wait 400ms** for analog power ramp
10. Set `LINE_OUT_VOL` — line output volume
11. Set `CLK_CTRL` — sample rate divider (FS_RATE = 44.1kHz, MCLK_FREQ = 256*Fs)
12. Set `I2S_CTRL` — 16-bit, I2S mode
13. Set `SSS_CTRL` — route ADC to I2S out, route I2S in to DAC
14. Set `ADCDAC_CTRL` — unmute DAC
15. Set `DAC_VOL` — set DAC volume
16. Set `ANA_HP_CTRL` — headphone volume
17. Set `ANA_ADC_CTRL` — ADC volume
18. Set `ANA_CTRL` — unmute, select input, enable zero-cross detection

### Difference from existing Rust example
The inline driver in `rtic_sai_sgtl5000.rs` hardcodes 48kHz (`CLK_CTRL=0x0008`) and skips the 400ms delay. The proper driver defaults to 44.1kHz (`CLK_CTRL=0x0004`) and includes the delay.

## 3.6 Volume lookup table

The C++ driver uses a 128-entry lookup table mapping linear 0.0–1.0 to the SGTL5000's headphone volume register values (logarithmic). Port this table as a `static` array.

## Reference files
- `TeensyAudio/control_sgtl5000.h` — register addresses, class declaration
- `TeensyAudio/control_sgtl5000.cpp` — full implementation (1075 lines)
- `imxrt-hal/examples/rtic_sai_sgtl5000.rs` — minimal inline Rust driver (~90 lines, starting point)
- SGTL5000 Datasheet (NXP) — register map, power-up timing, I2C protocol

## Verification

RTIC example: line-in passthrough with volume control on the Teensy Audio Shield.

```
AudioInputI2S → AudioOutputI2S
                  + SGTL5000 codec control (volume, input select)
```

### Success criteria
- Audio passes from line-in to headphone-out cleanly
- `volume()` method audibly changes output level
- `input_select()` switches between LINEIN and MIC
- No audible pops or glitches during codec configuration
- 400ms power-on delay properly handled
