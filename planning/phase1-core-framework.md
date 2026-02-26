# Phase 1: Core Audio Framework (`teensy-audio` crate) — COMPLETE

This phase builds the foundational abstractions that all audio nodes share: the block allocator, the node trait, and DSP utilities.

**Status: Complete**

## 1.1 Create the crate skeleton — DONE

- Workspace `Cargo.toml` at repo root with `members = ["teensy-audio"]`, `resolver = "2"`
- `teensy-audio/Cargo.toml`: `no_std`, edition 2021, MIT license
  - No external dependencies for Phase 1 (atomics from `core`)
  - Feature flags: `default = ["dsp"]`, `sgtl5000`, `i2s`, `dma`, `dsp`
- `lib.rs` with `#![no_std]`, module declarations, `dsp` gated on feature flag
- `constants.rs`: `AUDIO_BLOCK_SAMPLES = 128`, `POOL_SIZE = 32`, `AUDIO_SAMPLE_RATE_EXACT = 44_117.647`
- `.gitignore` updated with `/target`

### File structure
```
TeensyAudio-rs/
  Cargo.toml                      # workspace root
  teensy-audio/
    Cargo.toml                    # library crate (no_std)
    src/
      lib.rs                      # #![no_std], feature flags, re-exports
      constants.rs                # AUDIO_BLOCK_SAMPLES, POOL_SIZE, SAMPLE_RATE
      block/
        mod.rs                    # re-exports AudioBlockRef, AudioBlockMut, pool
        pool.rs                   # AudioBlockPool: static pool, atomic bitmap, alloc/dealloc
        ref_types.rs              # AudioBlockRef (shared), AudioBlockMut (exclusive), Deref/Drop
      node.rs                     # AudioNode trait
      control.rs                  # AudioControl trait
      dsp/
        mod.rs                    # re-exports all DSP submodules
        intrinsics.rs             # ARM DSP asm wrappers + pure-Rust fallbacks
        helpers.rs                # block_multiply, block_accumulate, Q15 helpers
        wavetables.rs             # SINE_TABLE[257], FADER_TABLE[257]
```

## 1.2 `AudioBlock` — the fundamental data unit — DONE

### Structure
- `AudioBlockData`: `#[repr(C, align(4))]` wrapping `[i16; 128]`
- Reference-counted via `AudioBlockPool` — static pool allocator
- Fixed-size pool of 32 blocks in a `static` using `UnsafeCell<[MaybeUninit<AudioBlockData>; 32]>`

### Pool allocator design (implemented in `block/pool.rs`)
- **Atomic bitmap** (`AtomicU32`) — bit N=1 means slot N is allocated
- **Per-slot ref counts** (`[AtomicU8; 32]`)
- `alloc()`: find first zero bit via `(!bitmap).trailing_zeros()`, CAS to set it, set refcount=1, zero the block, return slot index
- `dec_ref(slot)`: `fetch_sub(1)` on refcount; if was 1 (now 0), clear bitmap bit
- `inc_ref(slot)`: `fetch_add(1)` for clone
- Lock-free: no critical sections needed, safe in ISR context
- `#[cfg(test)] reset()` to clear pool state between tests

### Smart pointer types (implemented in `block/ref_types.rs`)
- **`AudioBlockMut`** — stores `slot: u8`, implements `Deref<Target=[i16;128]>`, `DerefMut`, `Drop`
  - `into_shared(self) -> AudioBlockRef` — zero-cost conversion (forget self, return Ref with same slot)
  - `alloc() -> Option<Self>` — convenience wrapper around `POOL.alloc()`
- **`AudioBlockRef`** — stores `slot: u8`, implements `Deref<Target=[i16;128]>`, `Clone` (increments refcount), `Drop`
  - `into_mut(self) -> Option<AudioBlockMut>` — clone-on-write: if refcount==1, convert in-place; if >1, alloc new block and copy

### C++ equivalence
| C++ | Rust |
|-----|------|
| `receiveReadOnly(channel)` | Returns `Option<AudioBlockRef>` |
| `receiveWritable(channel)` | Returns `Option<AudioBlockMut>` (clone-on-write via `into_mut()`) |
| `allocate()` | `AudioBlockMut::alloc() -> Option<AudioBlockMut>` |
| `release(block)` | Automatic via `Drop` on `AudioBlockRef` / `AudioBlockMut` |
| `transmit(block, channel)` | Place into output slot: `outputs[ch] = Some(block.into_shared())` |

## 1.3 `AudioNode` trait — DONE

```rust
pub trait AudioNode {
    const NUM_INPUTS: usize;
    const NUM_OUTPUTS: usize;
    fn update(
        &mut self,
        inputs: &[Option<AudioBlockRef>],
        outputs: &mut [Option<AudioBlockMut>],
    );
}
```

- Uses slices (not const-generic arrays) for stable Rust compatibility
- No global linked list — nodes will be owned by the audio graph struct (Phase 5)

## 1.4 `AudioControl` trait — DONE

```rust
pub trait AudioControl {
    type Error;
    fn enable(&mut self) -> Result<(), Self::Error>;
    fn disable(&mut self) -> Result<(), Self::Error>;
    fn volume(&mut self, level: f32) -> Result<(), Self::Error>;
}
```

Codec-specific methods (e.g., `input_select()`, `mic_gain()`) are inherent methods on the concrete type, not part of the trait.

## 1.5 DSP utility module — DONE

### ARM intrinsics (`dsp/intrinsics.rs`)
Each function has an ARM inline asm path (`#[cfg(all(target_arch = "arm", target_feature = "dsp"))]`) and a pure-Rust fallback. 17 functions implemented:

| Rust function | ARM instruction | Description |
|--------------|-----------------|-------------|
| `signed_saturate_rshift::<BITS, RSHIFT>(val)` | `SSAT` | Saturate with right shift (const generics for immediates) |
| `saturate16(val)` | `SSAT #16` | Saturate i32 to i16 range |
| `mul_32x16b(a, b)` | `SMULWB` | `(a * b[15:0]) >> 16` |
| `mul_32x16t(a, b)` | `SMULWT` | `(a * b[31:16]) >> 16` |
| `mul_32x32_rshift32(a, b)` | `SMMUL` | `(a * b) >> 32` |
| `mul_32x32_rshift32_rounded(a, b)` | `SMMULR` | `(a * b + 0x80000000) >> 32` |
| `multiply_accumulate_32x32_rshift32_rounded(sum, a, b)` | `SMMLAR` | `sum + (a*b+0x80000000)>>32` |
| `multiply_subtract_32x32_rshift32_rounded(sum, a, b)` | `SMMLSR` | `sum - (a*b+0x80000000)>>32` |
| `pack_16b_16b(a, b)` | `PKHBT` | `(a[15:0] << 16) \| b[15:0]` |
| `pack_16t_16b(a, b)` | `PKHTB` | `a[31:16] \| b[15:0]` |
| `pack_16t_16t(a, b)` | `PKHTB ASR #16` | `a[31:16] \| (b >> 16)` |
| `qadd16(a, b)` | `QADD16` | Saturating dual 16-bit add |
| `qsub16(a, b)` | `QSUB16` | Saturating dual 16-bit subtract |
| `mul_16bx16b(a, b)` | `SMULBB` | `a[15:0] * b[15:0]` |
| `mul_16bx16t(a, b)` | `SMULBT` | `a[15:0] * b[31:16]` |
| `mul_16tx16b(a, b)` | `SMULTB` | `a[31:16] * b[15:0]` |
| `mul_16tx16t(a, b)` | `SMULTT` | `a[31:16] * b[31:16]` |
| `multiply_accumulate_32x16b(sum, a, b)` | `SMLAWB` | `sum + (a * b[15:0]) >> 16` |
| `multiply_accumulate_32x16t(sum, a, b)` | `SMLAWT` | `sum + (a * b[31:16]) >> 16` |

### Higher-level helpers (`dsp/helpers.rs`)
- `saturating_multiply_q15(a: i16, b: i16) -> i16` — `(a * b) >> 15`, saturated
- `saturating_add_q15(a: i16, b: i16) -> i16` — saturating i16 addition
- `block_multiply(block: &mut [i16; 128], gain: i32)` — multiply entire block by Q15 gain
- `block_accumulate(dst: &mut [i16; 128], src: &[i16; 128])` — saturating add

### Wavetable data (`dsp/wavetables.rs`)
- `SINE_TABLE: [i16; 257]` — verbatim from `TeensyAudio/data_waveforms.c`
- `FADER_TABLE: [i16; 257]` — verbatim from `TeensyAudio/data_waveforms.c`
- 256 entries cover one full period; entry 256 = entry 0 for interpolation wrap-around

## Key Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Pool allocator | Atomic bitmap + per-slot AtomicU8 refcounts | Lock-free, O(1) alloc via trailing_zeros(), ISR-safe |
| Smart pointer size | `slot: u8` (1 byte) | POOL_SIZE ≤ 32, avoids raw pointers and lifetime issues |
| DSP cfg gate | `target_feature = "dsp"` | Correctly targets thumbv7em (Cortex-M7), falls back on host/M0 |
| SSAT operands | Const generics `<const BITS: u32, const RSHIFT: u32>` | ARM SSAT requires immediates; callers always use literals |
| AudioNode slices | `&[Option<AudioBlockRef>]` not const-generic arrays | Stable Rust — no `generic_const_exprs` feature needed |
| Test pool isolation | `#[cfg(test)] reset()` method | Simplest way to prevent test interference on shared global |
| No external deps | Phase 1 uses only `core` | Minimizes dependency tree; atomics are in `core::sync::atomic` |

## Verification — PASS

- **`cargo check`** — clean, zero warnings
- **`cargo test`** — **42 tests pass**:
  - Pool: alloc, dealloc, exhaustion, unique slots, zeroed data, refcount lifecycle (6 tests)
  - Smart pointers: alloc/drop, read/write, into_shared, clone/drop, into_mut sole owner, into_mut clone-on-write (6 tests)
  - DSP intrinsics: saturate16, signed_saturate_rshift, mul_32x32, mul_32x16, pack, qadd16, qsub16, mul_16x16 variants, multiply-accumulate/subtract (18 tests)
  - DSP helpers: saturating_multiply_q15, saturating_add_q15, block_multiply, block_accumulate (4 tests)
  - Wavetables: lengths, endpoints, sine peak/symmetry, fader monotonicity/midpoint (8 tests)
- **`cargo check --target thumbv7em-none-eabihf`** — ARM cross-compilation succeeds (clean)

## Deferred to Later Phases

- Audio graph struct and `update_all()` wiring (Phase 5)
- `audio_graph!` macro with compile-time topological sort (Phase 5)
- External dependencies (`imxrt-hal`, `cortex-m`, etc.) will be added when I/O drivers need them (Phase 2)
