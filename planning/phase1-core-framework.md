# Phase 1: Core Audio Framework (`teensy-audio` crate)

This phase builds the foundational abstractions that all audio nodes share: the block allocator, the node trait, the connection graph, and DSP utilities.

## 1.1 Create the crate skeleton

- Initialize `TeensyAudio-rs/` as a `no_std` Cargo workspace with a `teensy-audio` library crate
- Dependencies: `imxrt-hal` (forked), `imxrt-ral`, `cortex-m`, `rtic` (optional feature)
- Feature flags: `sgtl5000`, `i2s`, `dma`, `dsp`

## 1.2 Define `AudioBlock` — the fundamental data unit

The `AudioBlock` is the atomic unit of audio data, matching the C++ `audio_block_t`.

### Structure
- A struct wrapping `[i16; 128]` (matching C++ `AUDIO_BLOCK_SAMPLES = 128`)
- Reference-counted via a static pool allocator (`AudioBlockPool`)
- Use a fixed-size pool of `AudioBlock` (e.g., 32 blocks = 8KB) allocated in a static `MaybeUninit` array

### Smart pointer types
- `AudioBlockRef` — shared, immutable reference (like `Rc`). `Clone` increments ref count.
- `AudioBlockMut` — exclusive, mutable reference. Cannot be cloned.
- Implement `Deref<Target=[i16; 128]>` for both; `DerefMut` for `AudioBlockMut` only.
- Conversion: `AudioBlockRef` → `AudioBlockMut` via clone-on-write (copies data if ref_count > 1, otherwise converts in-place).

### C++ equivalence
| C++ | Rust |
|-----|------|
| `receiveReadOnly(channel)` | Returns `Option<AudioBlockRef>` |
| `receiveWritable(channel)` | Returns `Option<AudioBlockMut>` (clone-on-write) |
| `allocate()` | `AudioBlockPool::alloc() -> Option<AudioBlockMut>` |
| `release(block)` | Automatic via `Drop` on `AudioBlockRef` / `AudioBlockMut` |
| `transmit(block, channel)` | Place into output slot: `outputs[ch] = Some(block)` |

### Pool allocator design
- Static array: `static POOL: MaybeUninit<[AudioBlockInner; POOL_SIZE]>`
- Free list: atomic bitmap or linked free list
- `alloc()` and `dealloc()` must be interrupt-safe (use critical sections or atomic ops)
- `POOL_SIZE` configurable (default 32, same as C++ `AudioMemory(N)`)

## 1.3 Define the `AudioNode` trait — replaces C++ `AudioStream`

```rust
pub trait AudioNode {
    /// Number of input channels this node accepts.
    const NUM_INPUTS: usize;
    /// Number of output channels this node produces.
    const NUM_OUTPUTS: usize;

    /// Process one block period (~2.9ms / 128 samples).
    /// Read from `inputs`, write to `outputs`.
    fn update(
        &mut self,
        inputs: &[Option<AudioBlockRef>],
        outputs: &mut [Option<AudioBlockMut>],
    );
}
```

### Design notes
- No global linked list — nodes are owned by the audio graph struct
- No virtual dispatch overhead — the `audio_graph!` macro calls each node's `update()` directly
- `inputs` and `outputs` slices are sized by the associated constants at compile time
- Nodes that produce no output (analyzers) have `NUM_OUTPUTS = 0`
- Nodes that consume no input (synths) have `NUM_INPUTS = 0`

## 1.4 Define the audio graph — replaces C++ `AudioConnection`

The C++ library uses `AudioConnection` objects to wire outputs to inputs at runtime. We replace this with compile-time graph construction.

### Initial approach (v1 — manual ordering)
For the first version, users manually construct nodes and wire them in a struct. The `update_all()` method calls nodes in declaration order (user is responsible for topological correctness):

```rust
struct MyAudioGraph {
    sine: AudioSynthSine,
    mixer: AudioMixer<4>,
    out: AudioOutputI2S,
}

impl MyAudioGraph {
    fn update_all(&mut self) {
        let mut sine_out = [None; 1];
        self.sine.update(&[], &mut sine_out);

        let mixer_in = [sine_out[0].as_ref().map(|b| b.as_shared()), None, None, None];
        let mut mixer_out = [None; 1];
        self.mixer.update(&mixer_in, &mut mixer_out);

        let out_in = [mixer_out[0].as_ref().map(|b| b.as_shared()), /* right */ None];
        self.out.update(&out_in, &mut []);
    }
}
```

### Future approach (v2 — `audio_graph!` macro)
The macro (Phase 5) will generate the struct and `update_all()` automatically from a connection declaration, with compile-time topological sort.

## 1.5 Define `AudioControl` trait — for codec drivers

```rust
pub trait AudioControl {
    type Error;
    fn enable(&mut self) -> Result<(), Self::Error>;
    fn disable(&mut self) -> Result<(), Self::Error>;
    fn volume(&mut self, level: f32) -> Result<(), Self::Error>;
}
```

Codec-specific methods (e.g., `input_select()`, `mic_gain()`) are inherent methods on the concrete type, not part of the trait.

## 1.6 DSP utility module

### ARM intrinsics
Port key inline functions from `TeensyAudio/utility/dspinst.h` using Rust `core::arch::arm`:

| C++ intrinsic | Rust function | ARM instruction |
|--------------|---------------|-----------------|
| `signed_saturate_rshift(val, bits, count)` | `saturate_rshift_i32(val, bits, count)` | `SSAT` |
| `signed_multiply_32x16b(a, b)` | `mul_32x16b(a, b)` | `SMULWB` |
| `signed_multiply_32x16t(a, b)` | `mul_32x16t(a, b)` | `SMULWT` |
| `multiply_32x32_rshift32(a, b)` | `mul_32x32_rshift32(a, b)` | `SMMUL` |
| `pack_16b_16b(a, b)` | `pack_16b_16b(a, b)` | `PKHBT` |
| `signed_add_16_and_16(a, b)` | `qadd16(a, b)` | `QADD16` |
| `multiply_16tx16t(a, b)` | `mul_16tx16t(a, b)` | `SMULTT` |
| `multiply_16bx16b(a, b)` | `mul_16bx16b(a, b)` | `SMULBB` |

### Higher-level helpers
- `saturating_multiply_q15(a: i16, b: i16) -> i16`
- `saturating_add_q15(a: i16, b: i16) -> i16`
- `block_multiply(block: &mut [i16; 128], gain: i16)` — multiply entire block by Q15 gain
- `block_accumulate(dst: &mut [i16; 128], src: &[i16; 128])` — saturating add

### Wavetable data
- Port the 257-entry sine wavetable from `TeensyAudio/data_waveforms.c`
- Store as `static SINE_TABLE: [i16; 257]`
- 256 entries cover one full period; entry 256 = entry 0 for interpolation wrap-around

## Verification

- Unit tests (run on host with `#[cfg(test)]`):
  - `AudioBlockPool`: alloc, dealloc, ref counting, pool exhaustion
  - `AudioBlockRef` / `AudioBlockMut`: clone, drop, clone-on-write
  - DSP intrinsics: known-value tests against pure-Rust fallbacks
- Integration test: construct a trivial passthrough `AudioNode`, feed a known block, verify output matches input
