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

## Roadmap

- [ ] HAL integration (DMA-driven I²S on i.MX RT1062)
- [ ] Additional waveforms (square, sawtooth, triangle, noise)
- [ ] FIR / biquad filters
- [ ] FFT analysis nodes
- [ ] USB audio class support
- [ ] `defmt` logging support

## License

See [LICENSE](LICENSE).
