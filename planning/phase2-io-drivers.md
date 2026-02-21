# Phase 2: I/O Drivers

This phase implements DMA-driven I2S input/output and user-facing queue buffers. These are the bridge between the audio processing graph and the SAI hardware.

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

RTIC example that:
1. Initializes SAI1 with DMA (using Phase 0 HAL extensions)
2. Configures SGTL5000 codec via I2C (using the inline driver from `rtic_sai_sgtl5000.rs` as a temporary stand-in)
3. Generates a 440Hz sine wave via `AudioPlayQueue`
4. Plays it through `AudioOutputI2S` into headphones on the Teensy Audio Shield

### Success criteria
- Clean 440Hz tone in headphones
- No audio glitches or underruns
- CPU usage low (DMA handles transfer, CPU only fills buffers once per block period)
