# Debug No-Audio Output — sine_tone.rs

## Symptom

`sine_tone.hex` loads onto Teensy 4.1 + Audio Shield Rev D, LED heartbeat blinks, but **no audio** comes out of the headphone jack.

## Reference

The known-working example is [rtic_sai_dma_tone.rs](../teensy4-rs/examples/rtic_sai_dma_tone.rs) in `teensy4-rs`. It plays a 440 Hz triangle tone through the same hardware and produces audible output. All differences below are relative to that reference.

---

## Issues Found (priority order)

### Issue 1 — Interleave format completely wrong (CRITICAL)

**File:** [teensy-audio/src/io/interleave.rs](../teensy-audio/src/io/interleave.rs)

**Problem:** `interleave_lr` packs two 16-bit samples into a single `u32`:

```rust
dest[i] = (left[i] as u16 as u32) | ((right[i] as u16 as u32) << 16);
```

This produces **128 words** for 128 frames (1 word per frame). But SAI1 in 32-bit word / 2-channel mode expects **2 u32 per frame** — one word per channel — totalling **256 words** for 128 frames.

**Reference format:** The working example writes:

```rust
buf[i * 2]     = sample32; // Left
buf[i * 2 + 1] = sample32; // Right
```

**Fix:** Rewrite `interleave_lr` (and `_l`, `_r`, `deinterleave`) to emit 2 words per frame. The output buffer must be `[u32; AUDIO_BLOCK_SAMPLES * 2]`.

### Issue 2 — Samples not MSB-aligned (CRITICAL)

**File:** [teensy-audio/src/io/interleave.rs](../teensy-audio/src/io/interleave.rs)

**Problem:** Samples are placed in the lower 16 bits of each u32. The SAI sends MSB-first in 32-bit word mode, so a 16-bit sample must be shifted left by 16 to land in the upper half-word:

```rust
// Wrong (current):
dest[i] = left[i] as u16 as u32;

// Correct (reference):
let sample32 = (sample as u16 as u32) << 16;
```

Without `<< 16`, the codec receives near-zero amplitude data.

**Fix:** Add `<< 16` to every interleave path.

### Issue 3 — DMA buffer size is half what it should be

**File:** [examples/src/sine_tone.rs](../examples/src/sine_tone.rs#L59)

**Problem:** `DMA_BUF_LEN = AUDIO_BLOCK_SAMPLES` (128). With 2 words per stereo frame this should be `AUDIO_BLOCK_SAMPLES * 2` (256), matching the reference:

```rust
const DMA_BUF_LEN: usize = AUDIO_BLOCK_SAMPLES * 2;
```

The static buffer, DMA transfer iterations, and re-arm all use this constant, so only the constant needs changing — but the `isr()` signature and `AudioOutputI2S` internals must also accept the larger buffer.

### Issue 4 — SAI RX handle dropped → clocks may stop (HIGH)

**File:** [examples/src/sine_tone.rs](../examples/src/sine_tone.rs#L118)

**Problem:** The current code does:

```rust
drop(sai_rx); // RX is enabled at split; we keep TX handle.
```

In `TxFollowRx` sync mode, the TX derives BCLK and LRCLK from the RX block. If the `Rx` type's `Drop` impl disables the receiver, all clocks stop and the codec receives nothing.

**Reference:** Keeps `sai_rx` alive and explicitly enables it:

```rust
sai_rx.set_enable(true);
```

**Fix:** Keep `sai_rx` alive (store in RTIC local resource or `core::mem::forget`). Call `sai_rx.set_enable(true)` before codec init so MCLK/BCLK/LRCLK are present on pins.

### Issue 5 — Codec headphones may remain muted (MEDIUM)

**File:** [teensy-audio/src/codec/sgtl5000.rs](../teensy-audio/src/codec/sgtl5000.rs)

**Problem:** `enable()` ends with:

```rust
self.write_register(CHIP_ANA_CTRL, 0x0036)?;  // MUTE_HP bit (bit 4) = 1 → muted
```

Then `volume(0.5)` calls `volume_integer(65)` which calls `unmute_headphone()`. That writes:

```rust
self.ana_ctrl & !(1 << 4)  // should clear MUTE_HP
```

But `ana_ctrl` was cached as `0x0036` (the last write to `CHIP_ANA_CTRL` in `enable()`), so unmute writes `0x0026`. In theory this is correct — bit 4 cleared.

**However:** The reference writes `CHIP_ANA_HP_CTRL = 0x1818` (~0 dB) and `CHIP_ANA_CTRL = 0x0126`. Our driver writes HP vol to `0x7F7F` first (minimum), then the volume integer sets it to the computed level. The register value `0x0026` vs `0x0126` differs in bit 8 (`MUTE_LO`). Not critical for headphone output, but the overall ANA_CTRL state diverges from the working reference.

**Verification step:** Read back `CHIP_ANA_CTRL` after `volume()` to confirm bit 4 is clear and check that the I2C writes succeed (they require MCLK to be running — see Issue 4).

### Issue 6 — DMA half-transfer model mismatches one-shot config (MEDIUM)

**Files:** [output_i2s.rs](../teensy-audio/src/io/output_i2s.rs) and [sine_tone.rs](../examples/src/sine_tone.rs)

**Problem:** `AudioOutputI2S::isr()` takes a `DmaHalf` argument and fills the "other" half of a 128-word buffer (64 words each). This implies a circular DMA with half-transfer interrupts.

But the DMA is configured as one-shot (`set_disable_on_completion(true)`), so there are no half-transfer interrupts — only a single completion interrupt per buffer. The `toggle` counter in the ISR synthetically alternates halves, but the DMA always reads the full buffer from the start.

**Reference:** Uses a single full-buffer one-shot DMA transfer and fills the entire 256-word buffer before re-arming. No half-transfer logic.

**Fix (Option A — match reference):** Remove `DmaHalf` from the ISR path for now. Write the full buffer each ISR. This means `output_i2s.rs` needs a method that fills an entire `[u32; AUDIO_BLOCK_SAMPLES * 2]` buffer at once, or the example bypasses the half-transfer logic.

**Fix (Option B — switch to circular DMA):** Configure half-transfer interrupt and use the existing logic. This is more complex and can be deferred.

---

## Recommended Debugging Sequence

### Step 0 — Minimal bypass test

Create a stripped-down ISR that writes a phase-accumulator tone directly into the DMA buffer using the reference's exact format (2 u32/frame, `<< 16`, 256-word buffer). Keep SAI RX alive. If this produces audio, the issue is confirmed as our audio pipeline (interleave + DMA model).

### Step 1 — Fix SAI RX lifetime (Issue 4)

- Store `sai_rx` in an RTIC local resource or use `core::mem::forget`.
- Call `sai_rx.set_enable(true)` before codec I2C init.
- Rebuild, flash, test. If audio appears with the bypass tone, this was the blocker.

### Step 2 — Fix interleave format (Issues 1 + 2)

In `interleave.rs`, rewrite all functions:

```rust
pub fn interleave_lr(dest: &mut [u32], left: &[i16], right: &[i16]) {
    debug_assert_eq!(dest.len(), left.len() * 2);
    for i in 0..left.len() {
        dest[i * 2]     = (left[i]  as u16 as u32) << 16;
        dest[i * 2 + 1] = (right[i] as u16 as u32) << 16;
    }
}
```

Update `interleave_l`, `interleave_r`, `deinterleave` similarly. Update all unit tests.

### Step 3 — Fix buffer size (Issue 3)

- Change `DMA_BUF_LEN` to `AUDIO_BLOCK_SAMPLES * 2` in all examples.
- Update `AudioOutputI2S::isr()` signature to accept `[u32; AUDIO_BLOCK_SAMPLES * 2]`.
- Either remove `DmaHalf` (Option A) or implement circular DMA (Option B).

### Step 4 — Verify codec unmute (Issue 5)

- Add a temporary read-back of `CHIP_ANA_CTRL` after `codec.volume(0.5)`.
- Confirm bit 4 (`MUTE_HP`) is clear.
- If I2C reads fail/hang, the MCLK wasn't running during codec init (Issue 4).

### Step 5 — End-to-end test

With all fixes applied, the full sine_tone pipeline should produce audible output. Verify with headphones / oscilloscope.

---

## Files to Modify

| File | Changes |
|------|---------|
| `teensy-audio/src/io/interleave.rs` | 2 words/frame, `<< 16` alignment, update all fns + tests |
| `teensy-audio/src/io/output_i2s.rs` | Buffer type `[u32; AUDIO_BLOCK_SAMPLES * 2]`, remove or adapt `DmaHalf` |
| `examples/src/sine_tone.rs` | `DMA_BUF_LEN = AUDIO_BLOCK_SAMPLES * 2`, keep `sai_rx` alive, fix ISR |
| `examples/src/line_in_passthrough.rs` | Same DMA + SAI RX fixes |
| `examples/src/graph_synth.rs` | Same DMA + SAI RX fixes |
| `teensy-audio/src/io/input_i2s.rs` | Mirror interleave fixes for deinterleave path |
