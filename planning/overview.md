# TeensyAudio-rs — Project Overview

A Rust reimplementation of the TeensyAudio C++ library's core architecture and key components, targeting Teensy 4.1 (i.MX RT 1062). Uses idiomatic Rust with static dispatch via traits, RTIC v2 for concurrency, and builds on the existing imxrt-hal/imxrt-ral ecosystem.

## Scope

**Initial target:** Core framework + I2S I/O + SGTL5000 codec + Mixer + a few effects/synths.

The C++ library has ~84 `AudioStream` subclasses across ~170 source files. This plan covers the foundational architecture and a representative subset of components sufficient to build real audio applications on the Teensy Audio Shield.

## Phase Summary

| Phase | Title | Document |
|-------|-------|----------|
| 0 | Fork & Extend teensy4-rs (HAL Prerequisites) | [phase0-hal-prerequisites.md](phase0-hal-prerequisites.md) |
| 1 | Core Audio Framework (`teensy-audio` crate) | [phase1-core-framework.md](phase1-core-framework.md) |
| 2 | I/O Drivers | [phase2-io-drivers.md](phase2-io-drivers.md) |
| 3 | SGTL5000 Codec Driver | [phase3-sgtl5000-driver.md](phase3-sgtl5000-driver.md) |
| 4 | DSP Nodes (Initial Set) | [phase4-dsp-nodes.md](phase4-dsp-nodes.md) |
| 5 | Integration & Polish | [phase5-integration.md](phase5-integration.md) |

## Key Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| **Dispatch model** | Static dispatch via `AudioNode` trait + const generics + compile-time graph | Zero-cost abstractions, no heap, no global linked lists |
| **Concurrency** | RTIC v2 | Matches existing imxrt-hal examples; hardware-priority concurrency, zero-cost |
| **HAL strategy** | Fork teensy4-rs, update to imxrt-hal v0.6 | Cleaner than `[patch]` overrides; SAI driver only exists in v0.6 |
| **I/O model** | DMA (not interrupt polling) | Frees CPU for DSP during transfers; essential for real audio workloads |
| **Block size** | 128 samples (i16/Q15) | Matches C++ library exactly; compatible FFT sizes, delays, timing |
| **Sample format** | `i16` (Q15 fixed-point) | Matches C++ library; f32 processing is a future option (Cortex-M7 has FPU) |

## Dependency Graph

```
teensy-audio (this crate)
  ├── teensy4-rs (forked, with SAI + PLL4 + DMA support)
  │     ├── imxrt-hal v0.6 (with SAI DMA additions)
  │     │     └── imxrt-ral v0.6 (imxrt1062 feature)
  │     └── teensy4-pins
  ├── cortex-m v0.7
  ├── rtic v2 (optional feature)
  ├── heapless (for lock-free queues)
  └── embedded-hal v1.0 (for codec I2C generics)
```

## Test Hardware

- Teensy 4.1 (i.MX RT 1062, Cortex-M7 @ 600MHz)
- Teensy Audio Shield (Rev D) with SGTL5000 codec

## Reference Locations

### Workspace Repositories

| Repository | Path | Purpose |
|-----------|------|---------|
| **TeensyAudio (C++)** | `TeensyAudio/` | Original C++ library — reference implementation for all audio components |
| **imxrt-hal** | `imxrt-hal/` | i.MX RT Hardware Abstraction Layer (local v0.6 with SAI driver) |
| **imxrt-ral** | `imxrt-ral/` | i.MX RT Register Access Layer (low-level register definitions) |
| **teensy4-rs** | `teensy4-rs/` | Teensy 4.x Rust BSP (to be forked and extended) |

### Key Source Files — C++ Reference Implementation

| File | Purpose |
|------|---------|
| `TeensyAudio/Audio.h` | Master include header — lists all components |
| `TeensyAudio/output_i2s.cpp` | I2S output with DMA double-buffering — primary I/O reference |
| `TeensyAudio/input_i2s.cpp` | I2S input with DMA — mirror of output |
| `TeensyAudio/control_sgtl5000.h` | SGTL5000 register map (~50 registers) |
| `TeensyAudio/control_sgtl5000.cpp` | SGTL5000 codec driver (1075 lines) — full feature set |
| `TeensyAudio/mixer.h` / `mixer.cpp` | `AudioMixer4` and `AudioAmplifier` |
| `TeensyAudio/synth_sine.cpp` | Sine oscillator — wavetable + phase accumulator |
| `TeensyAudio/synth_dc.cpp` | DC source — simplest possible node |
| `TeensyAudio/effect_fade.cpp` | Fade effect — simple volume control |
| `TeensyAudio/effect_envelope.cpp` | ADSR envelope — state machine pattern |
| `TeensyAudio/analyze_peak.cpp` | Peak detector — simplest analyzer |
| `TeensyAudio/analyze_rms.cpp` | RMS meter — accumulator pattern |
| `TeensyAudio/utility/dspinst.h` | ARM Cortex-M DSP intrinsics (SSAT, SMUL, QADD, etc.) |
| `TeensyAudio/utility/imxrt_hw.cpp` | Audio PLL (PLL4) clock configuration for i.MX RT |
| `TeensyAudio/data_waveforms.c` | Sine wavetable (257 entries) |
| `TeensyAudio/memcpy_audio.S` | Assembly-optimized interleave/deinterleave routines |

### Key Source Files — Rust HAL Ecosystem

| File | Purpose |
|------|---------|
| `imxrt-hal/src/chip/drivers/sai.rs` | SAI driver (763 lines) — I2S config, Tx/Rx, interrupts (no DMA yet) |
| `imxrt-hal/src/chip/drivers/dma.rs` | DMA driver — channel API, `Source`/`Destination` traits |
| `imxrt-hal/src/common/lpi2c.rs` | I2C driver (1226 lines) — needed for SGTL5000 control |
| `imxrt-hal/src/chip/drivers/ccm_10xx/clock_gate.rs` | Clock gate definitions including `sai::<N>()` |
| `imxrt-hal/src/chip/imxrt1060.rs` | Chip-specific config — `SAI_CLOCK_GATES` constant |
| `imxrt-hal/board/src/teensy4.rs` | Teensy 4 board config — SAI1 pin types, MCLK/TX/RX definitions |
| `imxrt-hal/examples/rtic_sai_sgtl5000.rs` | Working RTIC SAI + SGTL5000 example (interrupt-driven, ~90-line inline codec driver) |
| `imxrt-ral/src/blocks/imxrt1015/sai.rs` | SAI register block definitions (shared by imxrt1062) |
| `teensy4-rs/Cargo.toml` | BSP dependency declarations (currently imxrt-hal 0.5.3) |
| `teensy4-rs/src/board.rs` | BSP `Resources` struct — needs SAI fields added |
| `teensy4-rs/src/clock_power.rs` | Clock gate enable list — needs SAI gates added |

### External Documentation

| Document | URL / Location |
|----------|---------------|
| i.MX RT 1060 Reference Manual | NXP IMXRT1060RM — Chapter 36 (SAI), Chapter 5 (DMA MUX Table 4-3) |
| SGTL5000 Datasheet | NXP SGTL5000 — register map, power-up sequence, I2C protocol |
| Teensy Audio Library Design Page | https://www.pjrc.com/teensy/td_libs_Audio.html |
| Teensy Audio System Design Tool | https://www.pjrc.com/teensy/gui/ |
| RTIC v2 Documentation | https://rtic.rs/2/book/en/ |
| embedded-hal traits | https://docs.rs/embedded-hal/1.0/ |

## Verification (End-to-End)

| Test | Description |
|------|-------------|
| **Smoke test** | Sine → I2S out on Audio Shield with SGTL5000 |
| **Passthrough test** | I2S in → I2S out (line-in to headphones) |
| **DSP test** | Sine → envelope → mixer → I2S out, with peak analyzer confirming signal levels |
| **Latency measurement** | Verify ~2.9ms block latency (128 samples / 44.1kHz) matches C++ behavior |

## C++ Library Scope Reference

The full C++ library contains ~84 `AudioStream` subclasses. The table below shows what's in scope for this plan vs. future work.

| Category | Total in C++ | In Scope (Phase 0–5) | Future |
|----------|-------------|----------------------|--------|
| **Core framework** | 1 (AudioStream) | ✅ AudioNode trait, block system, graph | — |
| **Inputs** | ~18 | ✅ I2S, PlayQueue | TDM, PDM, SPDIF, ADC, SD playback |
| **Outputs** | ~18 | ✅ I2S, RecordQueue | TDM, SPDIF, DAC, PWM, MQS, ADAT |
| **Effects** | ~16 | ✅ Fade, Envelope | Delay, Reverb, Chorus, Flange, Bitcrusher, etc. |
| **Filters** | ~4 | — | Biquad, FIR, StateVariable, Ladder |
| **Mixers** | 2 | ✅ Mixer, Amplifier | — |
| **Analyzers** | 7 | ✅ Peak, RMS | FFT256, FFT1024, ToneDetect, NoteFreq |
| **Synthesizers** | ~12 | ✅ Sine, DC | Waveform, Noise, PWM, KarplusStrong, Wavetable |
| **Controls** | ~7 | ✅ SGTL5000 | WM8731, AK4558, CS4272, CS42448, TLV320 |
| **DSP utilities** | ~10 files | ✅ ARM intrinsics, sine table | Resampler, Quantizer, FFT windows |

## Key Constants

| Constant | Value | Source |
|----------|-------|--------|
| `AUDIO_BLOCK_SAMPLES` | 128 | Teensy cores |
| `AUDIO_SAMPLE_RATE_EXACT` | ~44117.647 Hz | Teensy cores (PLL-derived) |
| Block duration | ~2.9 ms | 128 / 44100 |
| Sample format | `i16` (Q15) | Throughout |
| DMA buffer | `[u32; 128]` | Interleaved stereo (256 × i16) |
| Block pool size | 32 blocks (8 KB) | Configurable |
| SAI1 TX DMA MUX | 20 | i.MX RT 1060 RM Table 4-3 |
| SAI1 RX DMA MUX | 19 | i.MX RT 1060 RM Table 4-3 |
| SGTL5000 I2C address | 0x0A | `control_sgtl5000.h` |
| SAI1 clock gates | CCGR5 CG9 | imxrt-hal `clock_gate::sai::<1>()` |

## HAL Gaps to Fill

| Gap | Location | Work Required |
|-----|----------|---------------|
| SAI DMA trait impls | `imxrt-hal/src/chip/drivers/dma.rs` | Implement `Source<u32>` for `Rx`, `Destination<u32>` for `Tx` |
| SAI DMA enable/disable | `imxrt-hal/src/chip/drivers/sai.rs` | Add `enable_dma_transmit()`, `enable_dma_receive()`, `tdr()`, `rdr()` |
| SAI clock gates in BSP | `teensy4-rs/src/clock_power.rs` | Add `clock_gate::sai::<1..3>()` to `CLOCK_GATES` |
| SAI in Resources | `teensy4-rs/src/board.rs` | Add `sai1`/`sai2`/`sai3` fields |
| Audio PLL (PLL4) | `teensy4-rs/src/clock_power.rs` | New `setup_audio_pll()` function |
| MCLK direction | `teensy4-rs/src/board.rs` | Set `IOMUXC_GPR.GPR1.SAI1_MCLK_DIR = 1` |
