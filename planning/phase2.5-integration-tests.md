# Phase 2.5: Software Integration Tests

**Status: Complete**

After Phase 2 delivered unit-tested I/O components, this mini-phase adds integration tests that exercise the full audio data pipeline in software — no hardware required.

## Motivation

Each Phase 2 component was unit-tested in isolation. But bugs often hide at component boundaries:
- Does `AudioPlayQueue` produce blocks that `AudioOutputI2S` can consume?
- Does the interleaved DMA buffer produced by the output ISR round-trip correctly through the input ISR?
- Are audio blocks properly reference-counted through the full pipeline (no leaks, no use-after-free)?
- Does the system degrade gracefully under pool pressure?

A software loopback test exercises the **entire I/O data path** without needing hardware:

```
PlayQueue → OutputI2S.update → OutputI2S.isr (interleave)
    → [DMA buffer] →
InputI2S.isr (deinterleave) → InputI2S.update → RecordQueue
```

## Test Cases

### 2.5.1 Full Loopback — Stereo Sine Wave
Generate a stereo sine wave, push through the full pipeline, verify output matches input sample-for-sample.

**Data flow:**
1. Generate 128-sample sine block for left and right channels (different frequencies)
2. `PlayQueue.play()` both blocks
3. `PlayQueue.update()` → produces output blocks
4. Feed into `OutputI2S.update()` as left/right inputs
5. Call `OutputI2S.isr()` twice (two halves) → fills DMA TX buffer
6. Copy TX buffer to RX buffer (simulated loopback)
7. Call `InputI2S.isr()` twice → de-interleaves into working blocks
8. `InputI2S.update()` → produces output blocks
9. Feed into `RecordQueue.update()`
10. `RecordQueue.read()` → verify samples match originals

**Verifies:** Complete data integrity through all 6 components.

### 2.5.2 Multi-Block Streaming
Stream 4 consecutive blocks through the pipeline to verify block rotation, double-buffering, and queue FIFO ordering.

**Verifies:** Steady-state streaming works, not just single-block.

### 2.5.3 Left-Only and Right-Only
Send audio on only one channel. Verify the other channel is silent (zeros).

**Verifies:** Per-channel handling in interleave/deinterleave, no cross-talk.

### 2.5.4 Pool Accounting
Run a full loopback and verify the block pool returns to zero allocations afterward.

**Verifies:** No block leaks through the pipeline.

### 2.5.5 Empty Pipeline (Silence)
Run ISR cycles with no audio queued. Verify DMA buffer contains silence.

**Verifies:** Graceful handling of underrun conditions.

## Implementation

All tests live in `teensy-audio/src/io/integration_tests.rs` and run via `cargo test`.

## Verification

```
cargo test -- --test-threads=1 io::integration_tests
```

All 5 integration tests passing.
