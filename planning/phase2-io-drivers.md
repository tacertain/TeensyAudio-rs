# Phase 2: I/O Drivers

**Status: Complete**

This phase implements DMA-driven I2S input/output and user-facing queue buffers. These are the bridge between the audio processing graph and the SAI hardware.

## Implementation Summary

| Component | File | Tests | Description |
|-----------|------|-------|-------------|
| `SpscQueue<T, N>` | `io/spsc.rs` | 7 | Lock-free SPSC ring buffer using atomic indices; const-constructible, ISR-safe |
| Interleave utilities | `io/interleave.rs` | 8 | `interleave_lr`, `interleave_l`, `interleave_r`, `deinterleave`, `silence` |
| `AudioOutputI2S` | `io/output_i2s.rs` | 10 | DMA-driven I2S stereo output with double-buffered block management |
| `AudioInputI2S` | `io/input_i2s.rs` | 8 | DMA-driven I2S stereo input with working-block allocation |
| `AudioPlayQueue` | `io/play_queue.rs` | 5 | User → graph queue via SPSC buffer |
| `AudioRecordQueue` | `io/record_queue.rs` | 8 | Graph → user queue with start/stop recording |
| Module root | `io/mod.rs` | — | Re-exports all public types |

**Total: 46 new tests (88 cumulative)**

### Key design decisions made during implementation

- **Custom SPSC instead of `heapless`**: Built a zero-dependency `SpscQueue<T, N>` with const generics to avoid adding `heapless` as a dependency. Uses Lamport queue algorithm (one sentinel slot) with atomic load/store ordering.
- **Hardware-agnostic ISR API**: The `isr()` methods on `AudioOutputI2S` and `AudioInputI2S` accept `&mut [u32; 128]` + `DmaHalf` enum rather than HAL-specific DMA channel types. This decouples the audio logic from hardware setup and lets the RTIC ISR handler own the DMA buffer and determine which half completed.
- **`DmaHalf` enum**: Shared between input and output drivers — the ISR determines which half the DMA is operating on and passes it in.
- **`AudioBlockMut` over `AudioBlockRef` in queues**: `AudioPlayQueue` stores `AudioBlockMut` (exclusive ownership in the queue), while `AudioRecordQueue` stores `AudioBlockRef` (shared, since the graph may still reference the block).
- **`Debug` derives added**: `AudioBlockMut` and `AudioBlockRef` now derive `Debug` to support `.unwrap()` on `Result<(), T>` in user code.

## 2.1 `AudioOutputI2S` — DMA-driven I2S output

The output driver is the most critical component — it owns the DMA transfer and drives the audio update cycle.

### Architecture

```
Audio Graph                    DMA Buffer (DMAMEM)              SAI1 TX
┌──────────┐                  ┌─────────┬─────────┐           ┌─────────┐
│ left  [0]├──interleave──►│  Half A  │  Half B  │──DMA──►│  TDR[0]  │
│ right [1]├──────────────►│ 64×u32   │ 64×u32   │          │          │
└──────────┘                  └─────────┴─────────┘           └─────────┘
                                 ▲ ISR fills    ▲ ISR fills
                                 inactive half  inactive half
```

### DMA buffer
- Static `[u32; 128]` in DMAMEM — 128 interleaved stereo frames
- Each `u32` = two packed `i16` samples (left in lower 16 bits, right in upper 16 bits)
- DMA runs in circular mode with major loop count = 128
- Two interrupts per cycle: half-complete (after 64 words) and complete (after 128 words)

### DMA Transfer Control Descriptor (TCD) setup
- Source: memory buffer (`i2s_tx_buffer`)
- Destination: SAI1 TDR[0] register (fixed address)
- Minor loop: 4 bytes (one `u32` per DMA request)
- Major loop: 128 iterations (one full buffer cycle)
- Interrupts: `DMA_TCD_CSR_INTHALF | DMA_TCD_CSR_INTMAJOR`
- Circular: link major loop back to start

### ISR flow
1. DMA half-complete or complete interrupt fires
2. Determine which half of the buffer is inactive (not being read by DMA)
3. Interleave the current left/right `AudioBlock` data into the inactive half
4. Trigger `update_all()` on the audio graph (via software interrupt or direct call)
5. Swap the "current" and "next" block pointers (double-buffering at the block level)

### AudioNode implementation
- `NUM_INPUTS = 2` (left channel, right channel)
- `NUM_OUTPUTS = 0`
- `update()` stores the input blocks for the next DMA ISR to consume

### Update responsibility
This node triggers the audio graph update cycle, matching the C++ pattern where `AudioOutputI2S::isr()` calls `AudioStream::update_all()`.

### Reference
- `TeensyAudio/output_i2s.cpp` — C++ DMA setup, ISR, interleave logic

## 2.2 `AudioInputI2S` — DMA-driven I2S input

Mirror of the output driver, reading audio from the SAI RX FIFO.

### Architecture

```
SAI1 RX              DMA Buffer (DMAMEM)                 Audio Graph
┌─────────┐         ┌─────────┬─────────┐              ┌──────────┐
│  RDR[0]  │──DMA──►│  Half A  │  Half B  │──deinterl──►│ left  [0]│
│          │         │ 64×u32   │ 64×u32   │────────────►│ right [1]│
└─────────┘         └─────────┴─────────┘              └──────────┘
```

### AudioNode implementation
- `NUM_INPUTS = 0`
- `NUM_OUTPUTS = 2` (left channel, right channel)
- `update()` provides the most recently de-interleaved blocks as outputs

### Synchronization
- SAI RX configured with `sync_mode = RxFollowTx` — RX clocks derive from TX
- RX DMA buffer filled in lockstep with TX DMA consumption
- De-interleave happens in the DMA ISR: split each `u32` into left/right `i16` samples

### Reference
- `TeensyAudio/input_i2s.cpp` — C++ DMA setup, ISR, de-interleave logic

## 2.3 `AudioPlayQueue` — user-to-ISR buffer

Allows user code (non-ISR context) to inject audio blocks into the processing graph.

### Design
- Lock-free SPSC ring buffer (e.g., `heapless::spsc::Queue<AudioBlockRef, 4>`)
- User calls `play(block)` to enqueue
- `update()` dequeues one block and transmits it

### AudioNode implementation
- `NUM_INPUTS = 0`
- `NUM_OUTPUTS = 1`

## 2.4 `AudioRecordQueue` — ISR-to-user buffer

Allows user code to read audio blocks captured by the processing graph.

### Design
- Lock-free SPSC ring buffer (e.g., `heapless::spsc::Queue<AudioBlockRef, 4>`)
- `update()` receives an input block and enqueues it
- User calls `read()` to dequeue

### AudioNode implementation
- `NUM_INPUTS = 1`
- `NUM_OUTPUTS = 0`

## Verification

### Unit tests (complete)

All 46 Phase 2 tests pass (`cargo test -- --test-threads=1`):

- `io::spsc` — push/pop, wraparound, full queue rejection, FIFO ordering, drop cleanup
- `io::interleave` — LR/L-only/R-only interleave, deinterleave, roundtrip, extreme values, silence
- `io::output_i2s` — silence on empty, interleave both/left-only channels, block rotation after consumption, update responsibility signaling, ramp data verification
- `io::input_i2s` — working block allocation, ISR fill, de-interleaved output, cycle rotation, pool exhaustion handling
- `io::play_queue` — enqueue/dequeue, FIFO ordering, full queue rejection
- `io::record_queue` — start/stop, record/discard behavior, FIFO ordering, full queue silent drop, read-after-stop

### Hardware integration test (deferred to Phase 5)

RTIC example that:
1. Initializes SAI1 with DMA (using Phase 0 HAL extensions)
2. Configures SGTL5000 codec via I2C (using the inline driver from `rtic_sai_sgtl5000.rs` as a temporary stand-in)
3. Generates a 440Hz sine wave via `AudioPlayQueue`
4. Plays it through `AudioOutputI2S` into headphones on the Teensy Audio Shield

### Success criteria
- Clean 440Hz tone in headphones
- No audio glitches or underruns
- CPU usage low (DMA handles transfer, CPU only fills buffers once per block period)
