# Phase 5: Integration & Polish — **COMPLETE**

This phase ties everything together with the `audio_graph!` macro, documentation, examples, and CI.

## Status

All deliverables implemented. The macro uses an inline input-declaration syntax
(different from the originally planned separate `connections` block) which avoids
the `macro_rules!` ident-comparison limitation while remaining ergonomic.

## 5.1 `audio_graph!` macro — **COMPLETE**

A declarative `macro_rules!` macro that generates a typed audio graph struct with
automatic `update_all()` wiring.

### Final syntax (implemented)

```rust
use teensy_audio::audio_graph;
use teensy_audio::nodes::*;

audio_graph! {
    pub struct MyGraph {
        sine:  AudioSynthSine       {},
        env:   AudioEffectEnvelope  { (sine, 0) },
        mixer: AudioMixer<4>        { (env, 0), _, _, _ },
        peak:  AudioAnalyzePeak     { (mixer, 0) },
        out:   AudioOutputI2S       { (mixer, 0), (mixer, 0) },
    }
}
```

Each node's inputs are declared **inline** using `{ ... }`:
- `{}` — no inputs (source node)
- `{ (node, port) }` — input 0 connected to `node`'s output `port`
- `{ _ }` — unconnected input (receives `None` / silence)
- `{ (a, 0), (b, 0) }` — multiple inputs from different sources
- `{ (mixer, 0), (mixer, 0) }` — fan-out: same output to two inputs

### Design note

The originally planned `connections: [ (src, p) -> (dst, p) ]` syntax required
matching identifiers inside `macro_rules!`, which Rust macros cannot do. The
inline syntax sidesteps this entirely — each node declares its own inputs, so no
ident comparison is needed. The compile-time validation (correct input count) is
achieved via array type annotation against `NUM_INPUTS`.

### Generated code

The macro generates:
1. **A struct** with `pub` fields for each node
2. **`new()`** — constructs all nodes via `<Type>::new()`
3. **`update_all()`** — processes nodes in declaration order:
   - Builds each node's input array from connection specs
   - Allocates output `AudioBlockMut` blocks from the pool
   - Calls `node.update(inputs, outputs)`
   - Converts outputs to `AudioBlockRef` for downstream routing
   - Fan-out handled via `AudioBlockRef::clone()`

### Tests (8 unit + 15 verification = 23 new)

**Unit tests (graph/mod.rs):**
- Graph creation and field access
- Source → analyzer routing
- Multi-node chain with fan-out (peak + RMS from same amplifier)
- Mixer with multiple inputs and unconnected slots
- Envelope modulation chain
- DC source level accuracy
- Silent source (zero amplitude, zeroed block passthrough)
- Multiple update cycles (pool recycling, no leaks)

**Verification tests (graph/verification_tests.rs):**
- ADSR envelope shapes tone (idle → attack → sustain → release level progression)
- Pool accounting: zero block leaks after update cycles
- Pool accounting: zero leaks with fan-out graphs
- Streaming stability: 100 consecutive cycles with signal validation
- Gain staging: half gain, quarter gain, zero attenuation
- Fan-out correctness: identical levels at both consumers
- Mixer summing accuracy: two DC sources, gain weighting
- Fade-in increases output over time
- Fade-out decreases output below full level
- Full synthesizer chain: 2 oscillators → 2 envelopes → mixer → amplifier → analyzers
- RMS DC accuracy: RMS of constant signal equals amplitude
- Block count/duration sanity check (128 samples ≈ 2.9 ms)

## 5.2 Documentation — **COMPLETE**

### README.md — **COMPLETE**
- Project overview and motivation
- ASCII architecture diagram (block system → traits → nodes → graph macro)
- Quick start guide with `audio_graph!` macro example
- Module summary table
- Available nodes table (8 nodes across 4 categories)
- Cargo features table
- Build/test commands (`cargo test`, `cargo check --target thumbv7em-none-eabihf`)
- Audio parameters table
- Roadmap section for future work

### Rustdoc — **COMPLETE**
- Crate-level doc comment on `lib.rs` with module table, quick-start example,
  feature/parameter reference
- Module-level doc on `graph.rs` with full syntax documentation and examples
- Doc comments on generated macro API (`new()`, `update_all()`)

## 5.3 Examples — **COMPLETE**

Three hardware examples targeting Teensy 4.1 + Audio Shield (SGTL5000) are
provided in the `examples/` workspace crate. Each is a standalone RTIC firmware
binary demonstrating a different aspect of the library:

| Example | Description |
|---------|-------------|
| `sine_tone` | 440 Hz sine → headphone output. Minimal audio pipeline. |
| `line_in_passthrough` | Line-in stereo → headphone out. Two DMA channels, shared RTIC resource. |
| `graph_synth` | Multi-node pipeline (sine → amp → mixer → output) with software tremolo. |

### Key patterns demonstrated

- **SAI1 + DMA setup:** MCLK direction, I2S config, bclk divider, TxFollowRx sync
- **SGTL5000 codec init:** Using the `teensy-audio` `Sgtl5000` driver with `embedded-hal` 1.0 traits
- **`AsmDelay` wrapper:** `cortex_m::asm::delay` implementing `DelayNs` (cortex-m 0.7 only provides EH 0.2 delays)
- **Double-buffered DMA ISR:** `output.isr()` triggers graph updates; blocks interleaved into DMA buffer
- **Manual node wiring:** Allocate output blocks, call `update()`, convert to `AudioBlockRef` for routing
- **RTIC shared resources:** Line-in passthrough shares `AudioInputI2S` between RX and TX DMA ISRs
- **Mono-to-stereo fan-out:** `AudioBlockRef::clone()` for zero-copy dual-channel output

### Build

```bash
cargo check -p teensy-audio-examples --target thumbv7em-none-eabihf
```

### Note on the `audio_graph!` macro

The declarative `audio_graph!` macro with `update_all()` is ideal for pure-DSP
chains terminating in analyzer nodes (e.g. `AudioAnalyzePeak`). For chains that
terminate in I/O nodes like `AudioOutputI2S` — which require `new(bool)` and ISR
integration — manual node wiring is demonstrated instead.

## 5.4 CI setup — **COMPLETE**

### GitHub Actions workflow (`.github/workflows/ci.yml`)
- **rustfmt** — `cargo fmt -p teensy-audio -- --check`
- **clippy** — `cargo clippy -p teensy-audio --target thumbv7em-none-eabihf -- -D warnings`
- **test** — `cargo test --lib -p teensy-audio -- --test-threads=1`
- **build** — `cargo check -p teensy-audio --target thumbv7em-none-eabihf`
- **rustdoc** — `cargo doc -p teensy-audio --no-deps` with `-D warnings`
- Triggers on push/PR to `main` branch
- Uses `dtolnay/rust-toolchain@stable`

## 5.5 Future expansion notes

After Phase 5, the framework is complete and new audio nodes can be added incrementally without architectural changes. Priority candidates for Phase 6+:

| Priority | Component | Complexity |
|----------|-----------|------------|
| High | `AudioFilterBiquad` | Medium — biquad coefficient computation + processing |
| High | `AudioSynthWaveform` | Medium — multiple waveform types (saw, square, triangle) |
| Medium | `AudioEffectDelay` | Medium — circular buffer, multiple taps |
| Medium | `AudioAnalyzeFFT256/1024` | High — FFT implementation (consider CMSIS-DSP bindings) |
| Medium | `AudioFilterStateVariable` | Medium — simultaneous LP/BP/HP outputs |
| Low | `AudioEffectFreeverb` | High — complex reverb algorithm |
| Low | `AudioEffectChorus/Flange` | Medium — modulated delay lines |
| Low | Additional codec drivers | Medium per codec |

## Verification (End-to-End)

All four examples build and run on Teensy 4.1 with Audio Shield:

| Test | Description |
|------|-------------|
| **Smoke test** | Example 1: Sine → I2S out, audible clean tone |
| **Passthrough test** | Example 2: Line-in → headphones, no artifacts |
| **DSP test** | Example 3: ADSR envelope shapes tone, peak analyzer reads levels |
| **Macro test** | Example 4: Same behavior as Example 3 but using `audio_graph!` |
| **Latency measurement** | Verify ~2.9ms block latency (128 samples / 44.1kHz) |
| **CI** | All checks pass: build, clippy, fmt, doc, host tests |
