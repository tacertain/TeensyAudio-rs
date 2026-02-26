# Phase 4: DSP Nodes (Initial Set) — **Complete**

This phase implements the first batch of audio processing nodes — enough to build a simple synthesizer or effects chain. Each node implements the `AudioNode` trait from Phase 1.

### Implementation Summary

All 8 nodes implemented in `teensy-audio/src/nodes/`:
- `AudioMixer<N>` — const-generic N-channel mixer (§4.1)
- `AudioAmplifier` — single-channel gain (§4.2)
- `AudioSynthSine` — sine wave oscillator (§4.3)
- `AudioSynthWaveformDc` — DC level source (§4.4)
- `AudioEffectFade` — volume fade (§4.5)
- `AudioAnalyzePeak` — peak level detector (§4.6)
- `AudioAnalyzeRms` — RMS level meter (§4.7)
- `AudioEffectEnvelope` — ADSR envelope (§4.8)

51 new unit tests added (169 total). ARM cross-compilation verified. Added `libm` dependency for `sqrt` in RMS analyzer.

## 4.1 `AudioMixer<N>` — N-channel mixer

Mixes N input channels into a single mono output with per-channel gain.

### Port from
- `TeensyAudio/mixer.h` / `TeensyAudio/mixer.cpp`

### Design
```rust
pub struct AudioMixer<const N: usize> {
    gains: [i32; N],  // Q15 fixed-point gains (default = 1.0 = 0x7FFF scaled to i32)
}

impl<const N: usize> AudioNode for AudioMixer<N> {
    const NUM_INPUTS: usize = N;
    const NUM_OUTPUTS: usize = 1;
    fn update(&mut self, inputs: &[Option<AudioBlockRef>], outputs: &mut [Option<AudioBlockMut>]) { ... }
}
```

### Methods
- `gain(channel: usize, level: f32)` — set per-channel gain (0.0–32767.0 mapped to Q15 i32)

### Processing
For each sample index 0..128:
1. Accumulate: `sum += input[ch][i] * gain[ch]` (32-bit accumulator, Q15 multiply)
2. Saturate to i16 range
3. Store in output block

### Improvement over C++
Uses const generic `N` instead of hardcoded 4 channels. `AudioMixer<4>` matches C++ `AudioMixer4`, but `AudioMixer<8>` is also possible.

## 4.2 `AudioAmplifier` — single-channel gain

Simple volume control: one input, one output.

### Port from
- `AudioAmplifier` in `TeensyAudio/mixer.h`

### Design
```rust
pub struct AudioAmplifier {
    gain: i32,  // Q15 fixed-point
}

impl AudioNode for AudioAmplifier {
    const NUM_INPUTS: usize = 1;
    const NUM_OUTPUTS: usize = 1;
    fn update(...) { ... }
}
```

### Methods
- `gain(level: f32)` — set amplification (0.0 = silence, 1.0 = unity, >1.0 = boost)

## 4.3 `AudioSynthSine` — sine wave oscillator

Generates a sine wave using a phase accumulator and wavetable lookup.

### Port from
- `TeensyAudio/synth_sine.cpp`
- `TeensyAudio/data_waveforms.c` (sine wavetable)

### Design
```rust
pub struct AudioSynthSine {
    phase_accumulator: u32,
    phase_increment: u32,
    magnitude: i32,  // Q15
}

impl AudioNode for AudioSynthSine {
    const NUM_INPUTS: usize = 0;
    const NUM_OUTPUTS: usize = 1;
    fn update(...) { ... }
}
```

### Methods
- `frequency(hz: f32)` — sets phase increment: `(hz / AUDIO_SAMPLE_RATE) * 2^32`
- `amplitude(level: f32)` — sets output magnitude (0.0–1.0)
- `phase(angle: f32)` — sets phase offset in degrees

### Wavetable
- 257-entry `[i16; 257]` table (256 + 1 for wrap-around interpolation)
- Index from upper 8 bits of `phase_accumulator`
- Linear interpolation between adjacent entries using fractional bits

## 4.4 `AudioSynthWaveformDc` — DC level source

Simplest possible source node — fills the output block with a constant value.

### Port from
- `TeensyAudio/synth_dc.cpp`

### Design
```rust
pub struct AudioSynthWaveformDc {
    magnitude: i16,
}

impl AudioNode for AudioSynthWaveformDc {
    const NUM_INPUTS: usize = 0;
    const NUM_OUTPUTS: usize = 1;
    fn update(...) { ... }
}
```

### Methods
- `amplitude(level: f32)` — sets DC level (-1.0 to 1.0)

## 4.5 `AudioEffectFade` — volume fade

Smoothly fades audio volume up or down over time.

### Port from
- `TeensyAudio/effect_fade.cpp`

### Design
```rust
pub struct AudioEffectFade {
    position: u32,   // 0 = silent, 0xFFFFFFFF = full volume
    rate: i32,       // increment per sample (positive = fade in, negative = fade out)
}

impl AudioNode for AudioEffectFade {
    const NUM_INPUTS: usize = 1;
    const NUM_OUTPUTS: usize = 1;
    fn update(...) { ... }
}
```

### Methods
- `fade_in(milliseconds: u32)` — compute rate for fade-in over given duration
- `fade_out(milliseconds: u32)` — compute rate for fade-out

### Processing
For each sample: multiply input by `position >> 16` (upper 16 bits as Q15 gain), then advance `position += rate` with clamping at 0 and 0xFFFFFFFF.

## 4.6 `AudioAnalyzePeak` — peak level detector

Tracks the maximum absolute sample value over one or more block periods.

### Port from
- `TeensyAudio/analyze_peak.cpp`

### Design
```rust
pub struct AudioAnalyzePeak {
    min_val: i16,
    max_val: i16,
    new_output: bool,
}

impl AudioNode for AudioAnalyzePeak {
    const NUM_INPUTS: usize = 1;
    const NUM_OUTPUTS: usize = 0;  // analyzer, no audio output
    fn update(...) { ... }
}
```

### Methods
- `available() -> bool` — returns true if new data has been accumulated since last read
- `read() -> f32` — returns peak level (0.0–1.0) and resets accumulator
- `read_peak_to_peak() -> f32` — returns peak-to-peak level

### Processing
Scan all 128 samples, track min and max. Set `new_output = true`.

## 4.7 `AudioAnalyzeRms` — RMS level meter

Computes root-mean-square level over one or more block periods.

### Port from
- `TeensyAudio/analyze_rms.cpp`

### Design
```rust
pub struct AudioAnalyzeRms {
    accum: u64,       // sum of squares
    count: u32,       // number of samples accumulated
    new_output: bool,
}

impl AudioNode for AudioAnalyzeRms {
    const NUM_INPUTS: usize = 1;
    const NUM_OUTPUTS: usize = 0;
    fn update(...) { ... }
}
```

### Methods
- `available() -> bool` — returns true if new data available
- `read() -> f32` — returns RMS level (0.0–1.0) and resets

### Processing
For each sample: `accum += (sample as i32) * (sample as i32)`. On `read()`: `sqrt(accum / count) / 32768.0`.

## 4.8 `AudioEffectEnvelope` — ADSR envelope

Applies an Attack-Decay-Sustain-Release envelope to audio input.

### Port from
- `TeensyAudio/effect_envelope.cpp`

### Design
```rust
pub enum EnvelopeState {
    Idle,
    Delay,
    Attack,
    Hold,
    Decay,
    Sustain,
    Release,
    Forced,
}

pub struct AudioEffectEnvelope {
    state: EnvelopeState,
    level: i32,          // current envelope level (0 to max)
    attack_rate: i32,
    decay_rate: i32,
    release_rate: i32,
    sustain_level: i32,
    delay_count: u32,
    hold_count: u32,
    // ... timing counters
}

impl AudioNode for AudioEffectEnvelope {
    const NUM_INPUTS: usize = 1;
    const NUM_OUTPUTS: usize = 1;
    fn update(...) { ... }
}
```

### Methods
- `note_on()` — trigger envelope (start delay → attack → hold → decay → sustain)
- `note_off()` — trigger release phase
- `attack(milliseconds: f32)` — set attack time
- `hold(milliseconds: f32)` — set hold time
- `decay(milliseconds: f32)` — set decay time
- `sustain(level: f32)` — set sustain level (0.0–1.0)
- `release(milliseconds: f32)` — set release time
- `delay(milliseconds: f32)` — set initial delay before attack

### Processing
Per-sample state machine: advance `level` according to current state and rate, multiply input sample by `level`, output result. Transition states when level reaches target.

## Verification

RTIC example combining the Phase 2 I/O drivers with these DSP nodes:

```
AudioSynthSine ──► AudioEffectEnvelope ──► AudioMixer<4> ──► AudioOutputI2S
                                                              (left + right)
                                           AudioAnalyzePeak ◄─┘
```

- Pressing a button (or periodic timer) calls `envelope.note_on()` / `envelope.note_off()`
- Peak analyzer reads level and logs to USB serial
- SGTL5000 codec configured via Phase 3 driver

### Success criteria
- Audible sine tone with ADSR envelope shape
- Mixer gain control audibly affects volume
- Peak analyzer reports levels matching expected output
- No glitches, pops, or underruns
