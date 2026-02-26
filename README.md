# TeensyAudio-rs

A `no_std`, zero-allocation audio processing framework for the
[Teensy 4.x](https://www.pjrc.com/teensy/) (i.MX RT1062, Cortex-M7) written in
pure Rust. It mirrors the node-graph programming model of the
[PJRC Teensy Audio Library](https://www.pjrc.com/teensy/td_libs_Audio.html) while
leveraging Rust's type system for compile-time wiring validation.

## Features

- **Fixed-size block pool** — 32 blocks of 128 × `i16` samples; refcounted
  `AudioBlockRef` / exclusive `AudioBlockMut` handles (no heap allocation)
- **`AudioNode` trait** — uniform `update(inputs, outputs)` interface,
  const-generic input/output counts
- **`AudioControl` trait** — enable / disable / volume for hardware peripherals
- **Declarative graph macro** — `audio_graph!` wires nodes at compile time
- **I/O drivers** — `AudioOutputI2S`, `AudioInputI2S`, `AudioPlayQueue`,
  `AudioRecordQueue` stubs ready for HAL integration
- **SGTL5000 codec driver** — register-level I²C driver (feature-gated)
- **DSP nodes** — sine oscillator, DC source, amplifier, mixer, envelope, fade,
  peak & RMS analysis

## Architecture

```text
┌────────────────────────────────────────────────────────────┐
│  audio_graph! macro  (graph)                               │
│  Declarative wiring, update_all() processing               │
├──────────────┬──────────────┬──────────────┬───────────────┤
│  Synthesis   │  Effects     │  Analysis    │  I/O          │
│  (nodes)     │  (nodes)     │  (nodes)     │  (io)         │
├──────────────┴──────────────┴──────────────┴───────────────┤
│  AudioNode / AudioControl traits  (node, control)          │
├────────────────────────────────────────────────────────────┤
│  Block pool + refcounted handles  (block)                  │
│  constants (AUDIO_BLOCK_SAMPLES, AUDIO_SAMPLE_RATE, …)     │
└────────────────────────────────────────────────────────────┘
```

## Quick start

Add the dependency (from a local path during development):

```toml
[dependencies]
teensy-audio = { path = "../TeensyAudio-rs/teensy-audio" }
```

Declare and run an audio graph:

```rust
use teensy_audio::audio_graph;
use teensy_audio::nodes::*;

audio_graph! {
    pub struct MyGraph {
        sine:  AudioSynthSine       {},
        amp:   AudioAmplifier       { (sine, 0) },
        mixer: AudioMixer<4>        { (amp, 0), _, _, _ },
        peak:  AudioAnalyzePeak     { (mixer, 0) },
    }
}

fn main() {
    let mut g = MyGraph::new();

    // Configure nodes
    g.sine.frequency(440.0);
    g.sine.amplitude(1.0);
    g.amp.gain(0.5);
    g.mixer.gain(0, 1.0);

    // Process one block cycle (call from ISR / timer)
    g.update_all();

    if g.peak.available() {
        let level = g.peak.read(); // 0.0–1.0
    }
}
```

## Modules

| Module | Description |
|--------|-------------|
| `block` | Fixed-pool audio block allocator, `AudioBlockMut`, `AudioBlockRef` |
| `node` | `AudioNode` trait (per-node `update()` contract) |
| `control` | `AudioControl` trait (hardware enable/disable/volume) |
| `io` | I²S I/O, play/record queues, SPSC ring buffer |
| `codec` | SGTL5000 register-level I²C driver *(feature `sgtl5000`)* |
| `dsp` | Fixed-point math utilities *(feature `dsp`)* |
| `nodes` | Synthesis, effects & analysis nodes *(feature `dsp`)* |
| `graph` | `audio_graph!` macro for declarative wiring |

## Available nodes

| Category | Node | Description |
|----------|------|-------------|
| Synthesis | `AudioSynthSine` | Sine-wave oscillator (DDS, 128-entry wavetable) |
| Synthesis | `AudioSynthWaveformDc` | Constant DC level source |
| Effects | `AudioAmplifier` | Fixed-gain multiplier |
| Effects | `AudioMixer<N>` | N-input mixer with per-channel gain |
| Effects | `AudioEffectFade` | Linear fade in / fade out |
| Effects | `AudioEffectEnvelope` | ADSR envelope generator |
| Analysis | `AudioAnalyzePeak` | Peak absolute amplitude |
| Analysis | `AudioAnalyzeRms` | RMS level measurement |

## Cargo features

| Feature | Default | Description |
|---------|---------|-------------|
| `dsp` | ✅ | DSP math, synthesis/effect/analysis nodes |
| `sgtl5000` | ✅ | SGTL5000 codec driver (`embedded-hal` dependency) |

## Building

```sh
# Host tests (uses built-in test pool)
cargo test --lib -p teensy-audio -- --test-threads=1

# Cross-compile for Teensy 4.x
cargo check --target thumbv7em-none-eabihf -p teensy-audio
```

> **Note:** tests must run single-threaded (`--test-threads=1`) because the
> global block pool is shared mutable state.

## Audio parameters

| Parameter | Value |
|-----------|-------|
| Block size | 128 samples |
| Sample rate | 44 117.647 Hz |
| Sample format | `i16` (signed 16-bit) |
| Block pool | 32 blocks |

## Hardware examples

The `examples/` workspace crate contains three RTIC firmware binaries for
**Teensy 4.1 + Audio Shield** (SGTL5000):

| Example | Description |
|---------|-------------|
| `sine_tone` | 440 Hz sine wave → headphone output |
| `line_in_passthrough` | Line-in stereo → headphones (two DMA channels, shared RTIC resource) |
| `graph_synth` | Sine → amplifier → mixer → output with software tremolo envelope |

### Prerequisites

The examples depend on sibling repositories via relative paths. Your directory
layout must look like this:

```
parent/
├── TeensyAudio-rs/    ← this repo
├── teensy4-rs/        ← https://github.com/pjrc-rs/teensy4-rs
├── imxrt-hal/         ← https://github.com/imxrt-rs/imxrt-hal
└── imxrt-ral/         ← https://github.com/imxrt-rs/imxrt-ral
```

You also need the `thumbv7em-none-eabihf` target installed:

```sh
rustup target add thumbv7em-none-eabihf
```

### Building

To build all three examples:

```sh
cargo build -p teensy-audio-examples --target thumbv7em-none-eabihf --release
```

To build a single example:

```sh
cargo build -p teensy-audio-examples --bin sine_tone --target thumbv7em-none-eabihf --release
```

> **Tip:** Use `--release` for size-optimised builds. Debug builds may fail to
> link due to large text/data regions on the Cortex-M7.

Artifacts are placed in `target/thumbv7em-none-eabihf/release/`.

### Flashing

You will need:

- [`cargo-binutils`](https://github.com/rust-embedded/cargo-binutils) (provides
  `rust-objcopy`)
- Either [`teensy_loader_cli`](https://github.com/PaulStoffregen/teensy_loader_cli)
  or the [Teensy Loader Application](https://www.pjrc.com/teensy/loader.html)
  (ships with Teensyduino)

Convert the ELF to Intel HEX, then flash:

```sh
rust-objcopy -O ihex target/thumbv7em-none-eabihf/release/sine_tone sine_tone.hex
teensy_loader_cli --mcu=TEENSY41 -v -w sine_tone.hex
```

Repeat for whichever example binary you want to run.

## Known test flakiness

When running `cargo test` **without** `--test-threads=1`, three tests may
intermittently fail:

- `block::pool::tests::alloc_exhaustion`
- `graph::verification_tests::tests::verify_dsp_adsr_shapes_tone`
- `graph::verification_tests::tests::verify_fade_out_decreases_output`

### Root cause

All audio nodes allocate blocks from a **single global static pool**
(`block::pool::POOL`): a 32-slot bitmap-based allocator using atomics. Many
tests call `POOL.reset()` at their start to ensure a clean slate. When Rust's
test harness runs tests in parallel (the default), multiple tests race on this
shared global:

1. **Test A** calls `POOL.reset()`, clearing the bitmap.
2. **Test B** is mid-way through allocating blocks — its blocks are now silently
   freed under it.
3. **Test A** allocates its expected number of blocks, but Test B's concurrent
   allocations (or unexpected frees from step 2) leave the pool in a state
   neither test expected.

This manifests as:
- `alloc_exhaustion` — the pool appears to have free slots when it shouldn't,
  because another test reset it.
- Verification tests — node outputs come back as `None` (pool exhausted by a
  concurrent test) or have unexpected amplitude (blocks were zeroed mid-use).

### Workaround

Always run tests single-threaded:

```sh
cargo test --lib -p teensy-audio -- --test-threads=1
```

This is **not a bug in the pool allocator itself** — the atomic bitmap is
correct for single-core `no_std` use (ISR + main thread). The flakiness is
purely a test-harness artifact: the global pool was designed for a single
firmware application, not for dozens of independent tests sharing a process.

On real hardware only one firmware image runs at a time, so the pool is never
contested this way.

## Roadmap

- [x] HAL integration examples (DMA-driven I²S on i.MX RT1062)
- [ ] Additional waveforms (square, sawtooth, triangle, noise)
- [ ] FIR / biquad filters
- [ ] FFT analysis nodes
- [ ] USB audio class support
- [ ] `defmt` logging support

## License

See [LICENSE](LICENSE).
