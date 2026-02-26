# Phase 0: Fork & Extend teensy4-rs (HAL Prerequisites)

**Status: COMPLETE** — all steps implemented and compiling cleanly.

Before building the audio framework, we need a working SAI (I2S) + DMA foundation. The local imxrt-hal v0.6 has a SAI driver, but it lacks DMA support and isn't wired into teensy4-rs. This phase closes those gaps.

## 0.1 Fork teensy4-rs and update dependencies

- Fork `teensy4-rs/Cargo.toml` to point at imxrt-hal v0.6 and imxrt-ral v0.6 (local path or git deps)
- Update any breaking API changes between imxrt-hal 0.5 → 0.6
- Verify the existing examples still compile

### Key files
- `teensy4-rs/Cargo.toml` — dependency declarations (currently imxrt-hal 0.5.3)

## 0.2 Enable SAI clock gates in teensy4-rs

- Add `clock_gate::sai::<1>()`, `<2>()`, `<3>()` to the `CLOCK_GATES` array in `clock_power.rs`
- These constants already exist in imxrt-hal at `imxrt1060.rs` as `SAI_CLOCK_GATES`

### Key files
- `teensy4-rs/src/clock_power.rs` — `CLOCK_GATES` array (currently missing SAI)
- `imxrt-hal/src/chip/imxrt1060.rs` — `SAI_CLOCK_GATES` constant (already defined)
- `imxrt-hal/src/chip/drivers/ccm_10xx/clock_gate.rs` — `sai::<N>()` locator: CCGR5 CG9/CG10/CG11

## 0.3 Expose SAI peripherals in teensy4-rs BSP

- Add `sai1: ral::sai::SAI1`, `sai2`, `sai3` fields to the `Resources` struct in `board.rs`
- Wire them through `prepare_resources()`
- Add SAI1 pin type aliases and a helper function (following the `lpi2c()` / `lpspi()` pattern)

### Key files
- `teensy4-rs/src/board.rs` — `Resources` struct (line ~205), `prepare_resources()` (line ~600)
- `imxrt-hal/board/src/teensy4.rs` — SAI1 pin type definitions (MCLK=pin 23, TX sync=p27, TX bclk=p26, TX data=p7, RX sync=p20, RX bclk=p21, RX data=p8)

## 0.4 Add Audio PLL (PLL4) clock configuration

- Add a `setup_audio_pll()` function to `clock_power.rs` that configures PLL4 for 44.1kHz-family rates
- Set the SAI clock root mux to PLL4 and configure the divider
- Set `IOMUXC_GPR.GPR1.SAI1_MCLK_DIR = 1` to output MCLK on pin 23

### Reference
- `TeensyAudio/utility/imxrt_hw.cpp` — C++ `set_audioClock()` implementation: configures `CCM_ANALOG_PLL_AUDIO` with loop divider, numerator, denominator, and post-divider for exact 44117.647 Hz sample rate
- `imxrt-hal/board/src/teensy4.rs` — line ~148 sets `SAI1_MCLK_DIR`

### PLL4 configuration values (actual, from C++ runtime)
```
PLL loop divider:  28
Numerator:         2348
Denominator:       10000
Post-divider:      /1 → PLL4 ≈ 677.6 MHz
SAI clock prediv:  4, podf: 15
→ MCLK ≈ 11,293,920 Hz
BCLK divider:      4 → BCLK ≈ 2,823,480 Hz = 64×Fs
→ Fs = BCLK / 64 ≈ 44,117 Hz  (32-bit words × 2 channels)
```

## 0.5 Add SAI DMA support to imxrt-hal

### 0.5a Add DMA enable/disable and register accessors to sai.rs

Add to `Tx`:
- `enable_dma_transmit()` — set `TCSR.FWDE = 1` (FIFO Write DMA Enable)
- `disable_dma_transmit()` — set `TCSR.FWDE = 0`
- `tdr(&self) -> *const u32` — pointer to TDR[0] register (DMA destination address)

Add to `Rx`:
- `enable_dma_receive()` — set `RCSR.FRDE = 1` (FIFO Read DMA Enable)
- `disable_dma_receive()` — set `RCSR.FRDE = 0`
- `rdr(&self) -> *const u32` — pointer to RDR[0] register (DMA source address)

### Key file
- `imxrt-hal/src/chip/drivers/sai.rs` — `split()` currently sets `FRDE: 0, FWDE: 0` explicitly (line ~737)

### 0.5b Implement DMA peripheral traits in dma.rs

Implement `peripheral::Destination<u32>` for `sai::Tx` and `peripheral::Source<u32>` for `sai::Rx`, following the LPSPI pattern in `dma.rs`:

```rust
// SAI TX — DMA writes audio data TO the SAI TDR register
unsafe impl<const N: u8, ...> peripheral::Destination<u32> for sai::Tx<N, ...> {
    fn destination_signal(&self) -> u32 { SAI_DMA_TX_MAPPING[N - 1] }
    fn destination_address(&self) -> *const u32 { self.tdr() }
    fn enable_destination(&mut self) { self.enable_dma_transmit() }
    fn disable_destination(&mut self) { self.disable_dma_transmit() }
}

// SAI RX — DMA reads audio data FROM the SAI RDR register
unsafe impl<const N: u8, ...> peripheral::Source<u32> for sai::Rx<N, ...> {
    fn source_signal(&self) -> u32 { SAI_DMA_RX_MAPPING[N - 1] }
    fn source_address(&self) -> *const u32 { self.rdr() }
    fn enable_source(&mut self) { self.enable_dma_receive() }
    fn disable_source(&mut self) { self.disable_dma_receive() }
}
```

### DMA MUX source numbers (i.MX RT 1060 RM Table 4-3)

| Peripheral | TX Source | RX Source |
|-----------|----------|----------|
| SAI1 | 20 | 19 |
| SAI2 | 22 | 21 |
| SAI3 | 84 | 83 |

### Key files
- `imxrt-hal/src/chip/drivers/dma.rs` — existing `Source`/`Destination` impls for LPSPI (line ~150)

## Verification

Two RTIC examples that play a 440 Hz tone through SAI1 on the Teensy Audio Shield (SGTL5000):
- `rtic_sai_poll_tone.rs` — interrupt-driven FIFO polling (minimal, no DMA)
- `rtic_sai_dma_tone.rs` — DMA-driven transfers (no per-sample CPU intervention)

### Success criteria
- Clean build on `thumbv7em-none-eabihf` — **DONE**
- Existing teensy4-rs examples still compile — **DONE**
- 440 Hz tone audible on Audio Shield headphone output — **DONE** (both examples verified on hardware)

## Implementation Notes

### Branches
- **teensy4-rs**: `audio-support` branch
- **imxrt-hal**: `sai-dma` branch

### SAI configuration (final working values)
```
MCLK source:  MclkSource::Select1 (MSEL=0b01 → MCLK1 → PLL4, ~11.3 MHz)
BCLK div:     bclk_div(4) → BCLK ≈ 2,823,480 Hz = 64×Fs
Word size:    32-bit (split::<32, 2, PackingNone>)
Sync mode:    TxFollowRx (RX generates clocks on p20/p21, TX follows)
Sample fmt:   16-bit i16 → upper 16 bits of 32-bit word (MSB-aligned)
Codec I2S:    SCLKFREQ=0 (64×Fs), DLEN=16-bit, slave mode
```

### Breaking changes encountered
- **LPSPI Pins**: imxrt-hal v0.6 removed PCS0 from `lpspi::Pins<SDO, SDI, SCK>` (was 4 generic params in v0.5). Fixed all type aliases, helper functions, and examples.
- **imxrt-log**: The `imxrt-log v0.1.2` crate depends on `imxrt-hal v0.5.x` which is incompatible with the v0.6 RAL. Removed from dev-dependencies on the `audio-support` branch. The `rtic_lpspi` and `rtic_usb_log` examples that used it will need updating when `imxrt-log` is updated for HAL v0.6.

### Bugs discovered during bring-up
Eight bugs were found and fixed while getting the first tone to play. See `docs/debug-plan-no-audio.md` (in the teensy4-rs repo, since removed) for the full debugging log.

| # | Bug | Fix |
|---|-----|-----|
| 1 | `SyncMode::RxFollowTx` — wrong sync direction | Changed to `TxFollowRx` |
| 2 | RX side dropped before `set_enable(true)` — no BCLK/LRCLK | Keep `sai_rx`, call `set_enable(true)` |
| 3 | `MclkSource::Sysclk` (IPG 150 MHz) — clocks 13× too fast | Changed to `MclkSource::Select1` (PLL4) |
| 4 | `MclkSource::Select3` — dead clock (not configured) | Confirmed Select1 is correct |
| 5 | TX enabled 400 ms before ISR armed — FIFO underrun | Deferred TX enable until after ISR setup |
| 6 | ISR wrote 16 frames (32 words) to 16-slot FIFO — overflow | Reduced to 8 frames (16 words) |
| 7 | SGTL5000 SCLKFREQ mismatch with BCLK | Matched to 64×Fs (32-bit words) |
| 8 | 16-bit SAI words / bclk_div(8) vs Teensyduino's 32-bit / bclk_div(4) | Changed to 32-bit words, bclk_div(4) |
| — | SGTL5000 init: missing 400ms delay, wrong HP volume, MUTE_HP=1 in unmute, silent I2C errors | Fixed all codec init issues |

### Files modified

#### teensy4-rs (`audio-support` branch)
- `Cargo.toml` — path deps, patches, example registration, removed imxrt-log
- `src/board.rs` — LPSPI PCS0 removal, added SAI1/2/3 to Resources
- `src/clock_power.rs` — SAI clock gates, Audio PLL, SAI1 clock root
- `examples/rtic_lpspi.rs` — PCS0 fix
- `examples/rtic_sai_poll_tone.rs` — NEW: polled FIFO verification example
- `examples/rtic_sai_dma_tone.rs` — NEW: DMA verification example

#### imxrt-hal (`sai-dma` branch)
- `src/chip/drivers/sai.rs` — channel field, DMA enable/disable, TDR/RDR accessors
- `src/chip/drivers/dma.rs` — SAI DMA MUX mappings, Destination/Source impls
