//! DMA-driven I2S stereo input.
//!
//! [`AudioInputI2S`] reads interleaved stereo audio from the SAI1 RX DMA buffer,
//! de-interleaves it into separate left/right mono blocks, and provides them
//! as outputs to the audio processing graph.
//!
//! ## Architecture
//!
//! ```text
//! SAI1 RX              DMA Buffer (DMAMEM)                 Audio Graph
//! ┌─────────┐         ┌─────────┬─────────┐              ┌──────────┐
//! │  RDR[0]  │──DMA──►│  Half A  │  Half B  │──deinterl──►│ left  [0]│
//! │          │         │ 64×u32   │ 64×u32   │────────────►│ right [1]│
//! └─────────┘         └─────────┴─────────┘              └──────────┘
//! ```
//!
//! ## Synchronization
//!
//! SAI RX is configured with `sync_mode = RxFollowTx` — RX clocks derive
//! from TX. The RX DMA buffer fills in lockstep with TX DMA consumption.
//!
//! ## Usage with RTIC
//!
//! ```ignore
//! // In RTIC init: configure SAI1 RX + DMA, create the input node
//! let input = AudioInputI2S::new(false);
//!
//! // In DMA RX ISR:
//! let half = if dma_in_first_half { DmaHalf::First } else { DmaHalf::Second };
//! let should_update = input.isr(&DMA_RX_BUFFER, half);
//! if should_update { /* trigger audio graph update */ }
//!
//! // In audio update task:
//! let mut outputs = [None, None];
//! input.update(&[], &mut outputs);
//! // outputs[0] = left channel, outputs[1] = right channel
//! ```
//!
//! ## Reference
//!
//! Ported from `TeensyAudio/input_i2s.cpp`.

use crate::block::{AudioBlockMut, AudioBlockRef};
use crate::constants::AUDIO_BLOCK_SAMPLES;
use crate::node::AudioNode;

use super::interleave::deinterleave;
use super::output_i2s::DmaHalf;

/// DMA-driven I2S stereo input node.
///
/// Implements [`AudioNode`] with 0 inputs and 2 outputs (left, right).
///
/// The DMA engine fills a circular buffer with interleaved stereo data from
/// SAI1 RX. The ISR (via [`isr()`](Self::isr)) de-interleaves completed data
/// into separate left/right audio blocks. [`update()`](AudioNode::update)
/// provides the completed blocks as graph outputs and allocates fresh working
/// blocks for the next DMA cycle.
pub struct AudioInputI2S {
    /// Working block being filled by the ISR (left channel).
    block_left: Option<AudioBlockMut>,
    /// Working block being filled by the ISR (right channel).
    block_right: Option<AudioBlockMut>,
    /// Current sample offset into the working blocks (0, 64, or 128).
    block_offset: usize,
    /// If `true`, this node's ISR triggers the audio graph update cycle.
    update_responsibility: bool,
}

impl AudioInputI2S {
    /// Create a new I2S input node.
    ///
    /// # Arguments
    ///
    /// - `update_responsibility`: If `true`, this node's ISR will signal
    ///   that the audio graph should be updated.
    pub const fn new(update_responsibility: bool) -> Self {
        AudioInputI2S {
            block_left: None,
            block_right: None,
            block_offset: 0,
            update_responsibility,
        }
    }

    /// Handle DMA interrupt — de-interleave the completed half of the RX buffer.
    ///
    /// Call this from the DMA half-complete or complete ISR. It reads the
    /// DMA buffer half that has just been filled by hardware and splits the
    /// interleaved stereo data into the working left/right blocks.
    ///
    /// # Arguments
    ///
    /// - `dma_buffer`: The full DMA receive buffer (`[u32; AUDIO_BLOCK_SAMPLES]`).
    /// - `active_half`: Which half the DMA is currently writing to.
    ///
    /// # Returns
    ///
    /// `true` if the audio graph should be updated.
    pub fn isr(
        &mut self,
        dma_buffer: &[u32; AUDIO_BLOCK_SAMPLES],
        active_half: DmaHalf,
    ) -> bool {
        let half_len = AUDIO_BLOCK_SAMPLES / 2;

        // Read from the half that DMA has finished writing (opposite of active)
        let src = match active_half {
            DmaHalf::First => &dma_buffer[half_len..AUDIO_BLOCK_SAMPLES],
            DmaHalf::Second => &dma_buffer[..half_len],
        };

        let should_update =
            matches!(active_half, DmaHalf::First) && self.update_responsibility;

        // De-interleave into working blocks
        if let (Some(ref mut left), Some(ref mut right)) =
            (&mut self.block_left, &mut self.block_right)
        {
            let offset = self.block_offset;
            if offset + half_len <= AUDIO_BLOCK_SAMPLES {
                deinterleave(
                    src,
                    &mut left[offset..offset + half_len],
                    &mut right[offset..offset + half_len],
                );
                self.block_offset = offset + half_len;
            }
        }

        should_update
    }

    /// Whether this input is responsible for triggering graph updates.
    pub fn has_update_responsibility(&self) -> bool {
        self.update_responsibility
    }

    /// Whether the input currently has working blocks allocated.
    pub fn has_working_blocks(&self) -> bool {
        self.block_left.is_some() && self.block_right.is_some()
    }

    /// Current fill offset into working blocks.
    pub fn block_offset(&self) -> usize {
        self.block_offset
    }
}

impl AudioNode for AudioInputI2S {
    const NUM_INPUTS: usize = 0;
    const NUM_OUTPUTS: usize = 2;

    fn update(
        &mut self,
        _inputs: &[Option<AudioBlockRef>],
        outputs: &mut [Option<AudioBlockMut>],
    ) {
        // Try to allocate new working blocks (need both or neither)
        let new_left = AudioBlockMut::alloc();
        let new_right = if new_left.is_some() {
            AudioBlockMut::alloc()
        } else {
            None
        };

        if self.block_offset >= AUDIO_BLOCK_SAMPLES {
            // Working blocks are full — provide them as outputs
            if let Some(left) = self.block_left.take() {
                outputs[0] = Some(left);
            }
            if let Some(right) = self.block_right.take() {
                outputs[1] = Some(right);
            }

            // Install new working blocks for the next DMA cycle
            if let (Some(nl), Some(nr)) = (new_left, new_right) {
                self.block_left = Some(nl);
                self.block_right = Some(nr);
                self.block_offset = 0;
            }
        } else if let (Some(nl), Some(nr)) = (new_left, new_right) {
            // Working blocks aren't full yet
            if self.block_left.is_none() {
                // No working blocks exist — install these new ones
                self.block_left = Some(nl);
                self.block_right = Some(nr);
                self.block_offset = 0;
            }
            // else: already have working blocks, discard the new ones (they drop here)
        }
        // else: couldn't allocate — nothing we can do
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::pool::POOL;

    fn reset_pool() {
        POOL.reset();
    }

    #[test]
    fn new_has_no_blocks() {
        let input = AudioInputI2S::new(false);
        assert!(!input.has_working_blocks());
        assert_eq!(input.block_offset(), 0);
        assert!(!input.has_update_responsibility());
    }

    #[test]
    fn update_allocates_working_blocks() {
        reset_pool();
        let mut input = AudioInputI2S::new(false);
        let mut outputs = [None, None];

        input.update(&[], &mut outputs);

        assert!(input.has_working_blocks());
        assert_eq!(input.block_offset(), 0);
        // No outputs yet (blocks just allocated, not yet filled)
        assert!(outputs[0].is_none());
        assert!(outputs[1].is_none());
    }

    #[test]
    fn isr_fills_working_blocks() {
        reset_pool();
        let mut input = AudioInputI2S::new(false);
        let mut outputs = [None, None];
        input.update(&[], &mut outputs); // allocate working blocks

        // Create a DMA buffer with known data
        let half_len = AUDIO_BLOCK_SAMPLES / 2;
        let mut dma_buf = [0u32; AUDIO_BLOCK_SAMPLES];
        for i in 0..AUDIO_BLOCK_SAMPLES {
            let left = (i * 10) as i16;
            let right = (i * 10 + 5) as i16;
            dma_buf[i] = (left as u16 as u32) | ((right as u16 as u32) << 16);
        }

        // First ISR: DMA writing first half, read from second half
        input.isr(&dma_buf, DmaHalf::First);
        assert_eq!(input.block_offset(), half_len);

        // Second ISR: DMA writing second half, read from first half
        input.isr(&dma_buf, DmaHalf::Second);
        assert_eq!(input.block_offset(), AUDIO_BLOCK_SAMPLES);
    }

    #[test]
    fn update_provides_filled_blocks() {
        reset_pool();
        let mut input = AudioInputI2S::new(false);
        let mut outputs = [None, None];
        input.update(&[], &mut outputs); // allocate working blocks

        let mut dma_buf = [0u32; AUDIO_BLOCK_SAMPLES];

        // Fill with known pattern: left=100, right=200
        for i in 0..AUDIO_BLOCK_SAMPLES {
            dma_buf[i] = (100u16 as u32) | ((200u16 as u32) << 16);
        }

        // Two ISR calls to fill the block completely
        input.isr(&dma_buf, DmaHalf::First);
        input.isr(&dma_buf, DmaHalf::Second);

        // Now update should provide the filled blocks
        let mut outputs = [None, None];
        input.update(&[], &mut outputs);

        assert!(outputs[0].is_some(), "expected left output");
        assert!(outputs[1].is_some(), "expected right output");

        let left = outputs[0].as_ref().unwrap();
        let right = outputs[1].as_ref().unwrap();

        // Verify the de-interleaved data
        // ISR with DmaHalf::First reads from second half (indices 64..128)
        // ISR with DmaHalf::Second reads from first half (indices 0..64)
        // First ISR fills block[0..64], second fills block[64..128]
        for i in 0..AUDIO_BLOCK_SAMPLES {
            assert_eq!(left[i], 100, "left mismatch at {i}");
            assert_eq!(right[i], 200, "right mismatch at {i}");
        }
    }

    #[test]
    fn isr_without_working_blocks_is_safe() {
        let mut input = AudioInputI2S::new(false);
        let dma_buf = [0u32; AUDIO_BLOCK_SAMPLES];

        // Should not panic
        input.isr(&dma_buf, DmaHalf::First);
        input.isr(&dma_buf, DmaHalf::Second);
        assert_eq!(input.block_offset(), 0);
    }

    #[test]
    fn isr_signals_update_correctly() {
        let mut input_responsible = AudioInputI2S::new(true);
        let mut input_not = AudioInputI2S::new(false);
        let dma_buf = [0u32; AUDIO_BLOCK_SAMPLES];

        // First half active → should signal update
        assert!(input_responsible.isr(&dma_buf, DmaHalf::First));
        assert!(!input_responsible.isr(&dma_buf, DmaHalf::Second));

        assert!(!input_not.isr(&dma_buf, DmaHalf::First));
        assert!(!input_not.isr(&dma_buf, DmaHalf::Second));
    }

    #[test]
    fn update_cycle_rotation() {
        reset_pool();
        let mut input = AudioInputI2S::new(false);
        let dma_buf = [0u32; AUDIO_BLOCK_SAMPLES];

        // Cycle 1: allocate and fill
        let mut outputs = [None, None];
        input.update(&[], &mut outputs);
        assert!(input.has_working_blocks());

        input.isr(&dma_buf, DmaHalf::First);
        input.isr(&dma_buf, DmaHalf::Second);
        assert_eq!(input.block_offset(), AUDIO_BLOCK_SAMPLES);

        // Cycle 2: output filled blocks, allocate new ones
        let mut outputs = [None, None];
        input.update(&[], &mut outputs);
        assert!(outputs[0].is_some());
        assert!(outputs[1].is_some());
        assert!(input.has_working_blocks()); // new blocks allocated
        assert_eq!(input.block_offset(), 0);
    }

    #[test]
    fn pool_exhaustion_handled_gracefully() {
        reset_pool();
        let mut input = AudioInputI2S::new(false);

        // Exhaust the pool (32 blocks = POOL_SIZE)
        let mut _blocks = [const { None }; 32];
        for slot in _blocks.iter_mut() {
            *slot = Some(AudioBlockMut::alloc().unwrap());
        }

        // update with no pool space should not panic
        let mut outputs = [None, None];
        input.update(&[], &mut outputs);
        assert!(!input.has_working_blocks());
        assert!(outputs[0].is_none());
        assert!(outputs[1].is_none());
    }
}
