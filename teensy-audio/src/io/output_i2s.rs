//! DMA-driven I2S stereo output.
//!
//! [`AudioOutputI2S`] accepts left and right audio channel inputs and interleaves
//! them into a DMA buffer for transmission via SAI1. The DMA engine reads from
//! this buffer and feeds the SAI TX FIFO automatically.
//!
//! ## Architecture
//!
//! ```text
//! Audio Graph                    DMA Buffer (DMAMEM)              SAI1 TX
//! ┌──────────┐               ┌──────────┬──────────┐         ┌─────────┐
//! │ left  [0]├──interleave──►│  Half A  │  Half B  │───DMA──►│  TDR[0] │
//! │ right [1]├──────────────►│  64×u32  │  64×u32  │         │         │
//! └──────────┘               └──────────┴──────────┘         └─────────┘
//! ```
//!
//! ## DMA Buffer Layout
//!
//! - `[u32; 128]` — 128 interleaved stereo frames
//! - Each `u32` = packed stereo sample (left in lower 16 bits, right in upper 16)
//! - DMA runs in circular mode with half-complete and complete interrupts
//! - Two ISR calls per audio block cycle (64 frames each)
//!
//! ## Usage with RTIC
//!
//! ```ignore
//! // In RTIC init: configure SAI1 + DMA, create the output node
//! let output = AudioOutputI2S::new(true);
//!
//! // In DMA ISR: determine which half completed and fill the other
//! let half = if dma_in_first_half { DmaHalf::First } else { DmaHalf::Second };
//! let should_update = output.isr(&mut DMA_TX_BUFFER, half);
//! if should_update { /* trigger audio graph update */ }
//!
//! // In audio update task: pass blocks from the graph
//! output.update(&[left_block, right_block], &mut []);
//! ```
//!
//! ## Reference
//!
//! Ported from `TeensyAudio/output_i2s.cpp`.

use crate::block::{AudioBlockMut, AudioBlockRef};
use crate::constants::AUDIO_BLOCK_SAMPLES;
use crate::node::AudioNode;

use super::interleave::{interleave_l, interleave_lr, interleave_r};

/// Indicates which half of the DMA buffer the DMA engine is currently operating on.
///
/// Used by both output (TX) and input (RX) ISR handlers:
/// - **Output ISR:** DMA is *reading* this half, so fill the *other* half.
/// - **Input ISR:** DMA is *writing* this half, so read from the *other* half.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmaHalf {
    /// DMA is operating on the first half of the buffer (indices `0..N/2`).
    First,
    /// DMA is operating on the second half of the buffer (indices `N/2..N`).
    Second,
}

/// DMA-driven I2S stereo output node.
///
/// Implements [`AudioNode`] with 2 inputs (left, right) and 0 outputs.
///
/// This node uses double-buffering: [`update()`](AudioNode::update) queues audio blocks from the
/// graph, and the DMA ISR (via [`isr()`](Self::isr)) interleaves them into the DMA buffer.
/// Each block (128 samples) is consumed across two ISR calls (64 frames each).
pub struct AudioOutputI2S {
    /// First block being actively transmitted (left channel).
    block_left_1st: Option<AudioBlockRef>,
    /// Second block queued for transmission (left channel).
    block_left_2nd: Option<AudioBlockRef>,
    /// First block being actively transmitted (right channel).
    block_right_1st: Option<AudioBlockRef>,
    /// Second block queued for transmission (right channel).
    block_right_2nd: Option<AudioBlockRef>,
    /// Current sample offset into `block_left_1st` (0 or `AUDIO_BLOCK_SAMPLES / 2`).
    block_left_offset: usize,
    /// Current sample offset into `block_right_1st` (0 or `AUDIO_BLOCK_SAMPLES / 2`).
    block_right_offset: usize,
    /// If `true`, this node's ISR triggers the audio graph update cycle.
    update_responsibility: bool,
}

impl AudioOutputI2S {
    /// Create a new I2S output node.
    ///
    /// # Arguments
    ///
    /// - `update_responsibility`: If `true`, this node's ISR will signal that
    ///   the audio graph should be updated. Typically only one output node
    ///   in the system has this responsibility.
    pub const fn new(update_responsibility: bool) -> Self {
        AudioOutputI2S {
            block_left_1st: None,
            block_left_2nd: None,
            block_right_1st: None,
            block_right_2nd: None,
            block_left_offset: 0,
            block_right_offset: 0,
            update_responsibility,
        }
    }

    /// Handle DMA interrupt — fill the inactive half of the DMA buffer.
    ///
    /// Call this from the DMA half-complete or complete ISR. It interleaves
    /// the current left/right audio blocks into the DMA buffer half that
    /// the DMA engine is NOT currently reading.
    ///
    /// # Arguments
    ///
    /// - `dma_buffer`: The full DMA transmit buffer (`[u32; AUDIO_BLOCK_SAMPLES]`).
    /// - `active_half`: Which half the DMA is currently transmitting.
    ///
    /// # Returns
    ///
    /// `true` if the audio graph should be updated (i.e., `update_all()`
    /// should be called). This happens once per block cycle on the
    /// half-complete interrupt, when `update_responsibility` is set.
    pub fn isr(
        &mut self,
        dma_buffer: &mut [u32; AUDIO_BLOCK_SAMPLES],
        active_half: DmaHalf,
    ) -> bool {
        let half_len = AUDIO_BLOCK_SAMPLES / 2;

        // Determine which half to fill (opposite of what DMA is reading)
        let dest = match active_half {
            DmaHalf::First => &mut dma_buffer[half_len..AUDIO_BLOCK_SAMPLES],
            DmaHalf::Second => &mut dma_buffer[..half_len],
        };

        // Signal update on first-half interrupt (half-complete)
        let should_update =
            matches!(active_half, DmaHalf::First) && self.update_responsibility;

        // Interleave audio data into the DMA buffer
        let offset_l = self.block_left_offset;
        let offset_r = self.block_right_offset;

        match (&self.block_left_1st, &self.block_right_1st) {
            (Some(left), Some(right)) => {
                interleave_lr(
                    dest,
                    &left[offset_l..offset_l + half_len],
                    &right[offset_r..offset_r + half_len],
                );
            }
            (Some(left), None) => {
                interleave_l(dest, &left[offset_l..offset_l + half_len]);
            }
            (None, Some(right)) => {
                interleave_r(dest, &right[offset_r..offset_r + half_len]);
            }
            (None, None) => {
                dest.fill(0);
            }
        }

        // Advance left channel offset and rotate blocks if needed
        let new_offset_l = offset_l + half_len;
        if new_offset_l < AUDIO_BLOCK_SAMPLES {
            self.block_left_offset = new_offset_l;
        } else {
            self.block_left_offset = 0;
            self.block_left_1st = self.block_left_2nd.take();
        }

        // Advance right channel offset and rotate blocks if needed
        let new_offset_r = offset_r + half_len;
        if new_offset_r < AUDIO_BLOCK_SAMPLES {
            self.block_right_offset = new_offset_r;
        } else {
            self.block_right_offset = 0;
            self.block_right_1st = self.block_right_2nd.take();
        }

        should_update
    }

    /// Whether this output is responsible for triggering graph updates.
    pub fn has_update_responsibility(&self) -> bool {
        self.update_responsibility
    }

    /// Check if the output has a left channel block queued.
    pub fn has_left_block(&self) -> bool {
        self.block_left_1st.is_some()
    }

    /// Check if the output has a right channel block queued.
    pub fn has_right_block(&self) -> bool {
        self.block_right_1st.is_some()
    }
}

impl AudioNode for AudioOutputI2S {
    const NUM_INPUTS: usize = 2;
    const NUM_OUTPUTS: usize = 0;

    fn update(
        &mut self,
        inputs: &[Option<AudioBlockRef>],
        _outputs: &mut [Option<AudioBlockMut>],
    ) {
        // Input 0 = left channel
        if let Some(ref block) = inputs[0] {
            if self.block_left_1st.is_none() {
                self.block_left_1st = Some(block.clone());
                self.block_left_offset = 0;
            } else if self.block_left_2nd.is_none() {
                self.block_left_2nd = Some(block.clone());
            } else {
                // Both slots full — drop oldest, shift, add new
                self.block_left_1st = self.block_left_2nd.take();
                self.block_left_2nd = Some(block.clone());
                self.block_left_offset = 0;
            }
        }

        // Input 1 = right channel
        if let Some(ref block) = inputs[1] {
            if self.block_right_1st.is_none() {
                self.block_right_1st = Some(block.clone());
                self.block_right_offset = 0;
            } else if self.block_right_2nd.is_none() {
                self.block_right_2nd = Some(block.clone());
            } else {
                // Both slots full — drop oldest, shift, add new
                self.block_right_1st = self.block_right_2nd.take();
                self.block_right_2nd = Some(block.clone());
                self.block_right_offset = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::pool::POOL;
    use crate::block::AudioBlockMut;

    fn reset_pool() {
        POOL.reset();
    }

    /// Helper: allocate a block filled with a constant value.
    fn make_block(value: i16) -> AudioBlockRef {
        let mut block = AudioBlockMut::alloc().unwrap();
        block.fill(value);
        block.into_shared()
    }

    /// Helper: allocate a block with a linear ramp.
    fn make_ramp_block(start: i16) -> AudioBlockRef {
        let mut block = AudioBlockMut::alloc().unwrap();
        for (i, sample) in block.iter_mut().enumerate() {
            *sample = start.wrapping_add(i as i16);
        }
        block.into_shared()
    }

    #[test]
    fn new_has_no_blocks() {
        let output = AudioOutputI2S::new(true);
        assert!(!output.has_left_block());
        assert!(!output.has_right_block());
        assert!(output.has_update_responsibility());
    }

    #[test]
    fn update_queues_blocks() {
        reset_pool();
        let mut output = AudioOutputI2S::new(false);
        let left = make_block(100);
        let right = make_block(200);

        output.update(&[Some(left), Some(right)], &mut []);

        assert!(output.has_left_block());
        assert!(output.has_right_block());
    }

    #[test]
    fn update_double_buffer() {
        reset_pool();
        let mut output = AudioOutputI2S::new(false);

        let left1 = make_block(10);
        let right1 = make_block(20);
        output.update(&[Some(left1), Some(right1)], &mut []);
        assert!(output.block_left_1st.is_some());
        assert!(output.block_left_2nd.is_none());

        let left2 = make_block(30);
        let right2 = make_block(40);
        output.update(&[Some(left2), Some(right2)], &mut []);
        assert!(output.block_left_1st.is_some());
        assert!(output.block_left_2nd.is_some());
    }

    #[test]
    fn update_overflow_rotates() {
        reset_pool();
        let mut output = AudioOutputI2S::new(false);

        let left1 = make_block(10);
        let left2 = make_block(20);
        let left3 = make_block(30);

        output.update(&[Some(left1), None], &mut []);
        output.update(&[Some(left2), None], &mut []);
        output.update(&[Some(left3), None], &mut []);

        // After overflow: 1st = left2, 2nd = left3
        assert!(output.block_left_1st.is_some());
        assert!(output.block_left_2nd.is_some());
        assert_eq!(output.block_left_offset, 0);
    }

    #[test]
    fn isr_silence_when_no_blocks() {
        let mut output = AudioOutputI2S::new(true);
        let mut dma_buf = [0xDEAD_BEEFu32; AUDIO_BLOCK_SAMPLES];

        output.isr(&mut dma_buf, DmaHalf::First);

        // Second half should be zeroed (silence)
        let half_len = AUDIO_BLOCK_SAMPLES / 2;
        for &sample in &dma_buf[half_len..] {
            assert_eq!(sample, 0, "expected silence in second half");
        }
    }

    #[test]
    fn isr_interleaves_both_channels() {
        reset_pool();
        let mut output = AudioOutputI2S::new(false);
        let left = make_block(100);
        let right = make_block(200);
        output.update(&[Some(left), Some(right)], &mut []);

        let mut dma_buf = [0u32; AUDIO_BLOCK_SAMPLES];

        // First ISR call: fill second half (DMA reading first half)
        output.isr(&mut dma_buf, DmaHalf::First);

        let half_len = AUDIO_BLOCK_SAMPLES / 2;
        for i in half_len..AUDIO_BLOCK_SAMPLES {
            assert_eq!(dma_buf[i] as i16, 100, "left mismatch at {i}");
            assert_eq!((dma_buf[i] >> 16) as i16, 200, "right mismatch at {i}");
        }
    }

    #[test]
    fn isr_left_only_zeroes_right() {
        reset_pool();
        let mut output = AudioOutputI2S::new(false);
        let left = make_block(500);
        output.update(&[Some(left), None], &mut []);

        let mut dma_buf = [0u32; AUDIO_BLOCK_SAMPLES];
        output.isr(&mut dma_buf, DmaHalf::Second);

        // First half should have left data, right zero
        let half_len = AUDIO_BLOCK_SAMPLES / 2;
        for i in 0..half_len {
            assert_eq!(dma_buf[i] as i16, 500);
            assert_eq!((dma_buf[i] >> 16) as i16, 0);
        }
    }

    #[test]
    fn isr_rotates_blocks_after_full_consumption() {
        reset_pool();
        let mut output = AudioOutputI2S::new(false);
        let left1 = make_block(10);
        let left2 = make_block(20);
        output.update(&[Some(left1), None], &mut []);
        output.update(&[Some(left2), None], &mut []);

        let mut dma_buf = [0u32; AUDIO_BLOCK_SAMPLES];

        // Two ISR calls consume the first block (2 × 64 = 128 samples)
        output.isr(&mut dma_buf, DmaHalf::First);
        assert_eq!(output.block_left_offset, AUDIO_BLOCK_SAMPLES / 2);

        output.isr(&mut dma_buf, DmaHalf::Second);
        // First block consumed, second block becomes first
        assert_eq!(output.block_left_offset, 0);
        assert!(output.block_left_1st.is_some()); // left2 is now first
        assert!(output.block_left_2nd.is_none());
    }

    #[test]
    fn isr_signals_update_correctly() {
        let mut output_responsible = AudioOutputI2S::new(true);
        let mut output_not = AudioOutputI2S::new(false);
        let mut dma_buf = [0u32; AUDIO_BLOCK_SAMPLES];

        // First half active → should signal update (half-complete interrupt)
        assert!(output_responsible.isr(&mut dma_buf, DmaHalf::First));
        assert!(!output_responsible.isr(&mut dma_buf, DmaHalf::Second));

        assert!(!output_not.isr(&mut dma_buf, DmaHalf::First));
        assert!(!output_not.isr(&mut dma_buf, DmaHalf::Second));
    }

    #[test]
    fn isr_with_ramp_data() {
        reset_pool();
        let mut output = AudioOutputI2S::new(false);
        let left = make_ramp_block(0);
        let right = make_ramp_block(1000);
        output.update(&[Some(left), Some(right)], &mut []);

        let mut dma_buf = [0u32; AUDIO_BLOCK_SAMPLES];
        let half_len = AUDIO_BLOCK_SAMPLES / 2;

        // First ISR: fill second half with samples 0..63
        output.isr(&mut dma_buf, DmaHalf::First);
        for i in 0..half_len {
            let expected_l = i as i16;
            let expected_r = 1000i16.wrapping_add(i as i16);
            assert_eq!(dma_buf[half_len + i] as i16, expected_l);
            assert_eq!((dma_buf[half_len + i] >> 16) as i16, expected_r);
        }

        // Second ISR: fill first half with samples 64..127
        output.isr(&mut dma_buf, DmaHalf::Second);
        for i in 0..half_len {
            let expected_l = (half_len + i) as i16;
            let expected_r = 1000i16.wrapping_add((half_len + i) as i16);
            assert_eq!(dma_buf[i] as i16, expected_l);
            assert_eq!((dma_buf[i] >> 16) as i16, expected_r);
        }
    }
}
