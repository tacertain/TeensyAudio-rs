# Phase 5: Integration & Polish

This phase ties everything together with the `audio_graph!` macro, documentation, examples, and CI.

## 5.1 `audio_graph!` macro

A declarative macro that generates a typed audio graph struct with automatic `update_all()` wiring.

### Syntax

```rust
audio_graph! {
    sine: AudioSynthSine,
    env: AudioEffectEnvelope,
    mixer: AudioMixer<4>,
    peak: AudioAnalyzePeak,
    out: AudioOutputI2S,
    connections: [
        (sine, 0) -> (env, 0),
        (env, 0) -> (mixer, 0),
        (mixer, 0) -> (out, 0),   // left
        (mixer, 0) -> (out, 1),   // right
        (mixer, 0) -> (peak, 0),  // tap for analysis
    ]
}
```

### Generated code

The macro generates:

1. **A struct** with all nodes as named fields:
   ```rust
   struct AudioGraph {
       pub sine: AudioSynthSine,
       pub env: AudioEffectEnvelope,
       pub mixer: AudioMixer<4>,
       pub peak: AudioAnalyzePeak,
       pub out: AudioOutputI2S,
   }
   ```

2. **An `update_all()` method** that calls nodes in topological order and passes blocks between connected ports:
   ```rust
   impl AudioGraph {
       pub fn update_all(&mut self) {
           // 1. Sources first (no inputs)
           let mut sine_out = [None; 1];
           self.sine.update(&[], &mut sine_out);

           // 2. Then nodes that depend on sources
           let env_in = [sine_out[0].take().map(|b| b.as_shared())];
           let mut env_out = [None; 1];
           self.env.update(&env_in, &mut env_out);

           // 3. Continue in topological order...
           let mixer_in = [env_out[0].take().map(|b| b.as_shared()), None, None, None];
           let mut mixer_out = [None; 1];
           self.mixer.update(&mixer_in, &mut mixer_out);

           // 4. Fan-out: clone shared ref for multiple consumers
           let mixer_shared = mixer_out[0].take().map(|b| b.as_shared());
           let out_in = [mixer_shared.clone(), mixer_shared.clone()];
           self.out.update(&out_in, &mut []);

           let peak_in = [mixer_shared];
           self.peak.update(&peak_in, &mut []);
       }
   }
   ```

3. **Compile-time validation**:
   - Output index < `NUM_OUTPUTS` for source node
   - Input index < `NUM_INPUTS` for destination node
   - No cycles in the connection graph
   - Warning/error if an input is unconnected (receives `None`)

### Implementation approach
- Use `macro_rules!` for the initial version
- The topological sort can be done by ordering nodes: sources first (NUM_INPUTS=0), then nodes whose inputs are all defined, etc.
- Fan-out (one output → multiple inputs) handled via `AudioBlockRef::clone()`

## 5.2 Documentation

### README.md
- Project overview and motivation
- Architecture diagram (block system, node trait, graph macro)
- Getting started guide:
  1. Hardware requirements (Teensy 4.1 + Audio Shield)
  2. Toolchain setup (`thumbv7em-none-eabihf`, `cargo-objcopy`)
  3. First example walkthrough
- API overview with links to docs.rs

### Rustdoc
- Module-level docs on `teensy-audio` crate root
- Doc comments on all public types and methods
- Examples in doc comments for key APIs (`AudioNode`, `AudioBlock`, `AudioMixer`, etc.)

## 5.3 Examples

### Example 1: Sine tone playback
Simplest possible example — generate a tone and play it.
```
AudioSynthSine → AudioOutputI2S (both channels)
+ SGTL5000 enable + volume
```

### Example 2: Line-in passthrough
Audio passthrough from line-in to headphones with volume control.
```
AudioInputI2S → AudioOutputI2S
+ SGTL5000 with input_select(LINEIN), volume control
```

### Example 3: Synthesizer with envelope
Button-triggered tone with ADSR envelope and mixing.
```
AudioSynthSine → AudioEffectEnvelope → AudioMixer<4> → AudioOutputI2S
+ peak analyzer logging to USB serial
```

### Example 4: Audio graph macro demo
Same as Example 3 but using the `audio_graph!` macro for wiring.

## 5.4 CI setup

### GitHub Actions workflow
- **Target**: `thumbv7em-none-eabihf`
- **Steps**:
  1. Install Rust toolchain + target
  2. `cargo check` — compile check
  3. `cargo clippy -- -D warnings` — lint
  4. `cargo fmt --check` — formatting
  5. `cargo doc --no-deps` — documentation generation
  6. `cargo test --target x86_64-*` — run host-side unit tests (block allocator, DSP math)

### Dependency caching
- Cache `~/.cargo/registry` and `target/` directories

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
