//! DMA-driven I2S stereo output.
//!
//! [`AudioOutputI2S`] accepts left and right audio channel inputs and interleaves
//! them into a DMA buffer for transmission via SAI1. The DMA engine reads from
//! this buffer and feeds the SAI TX FIFO automatically.
//!
//! ## Architecture
//!
//! ```text
//! Audio Graph                   DMA Buffer (DMAMEM)                 SAI1 TX
//! ┌──────────┐               ┌───────────────────────┐          ┌─────────┐
//! │ left  [0]├──interleave──►│ 256 × u32 (128 frames)│───DMA──►│  TDR[0] │
//! │ right [1]├──────────────►│ L R L R L R ...       │          │         │
//! └──────────┘               └───────────────────────┘          └─────────┘
//! ```
//!
//! ## DMA Buffer Layout
//!
//! - `[u32; AUDIO_BLOCK_SAMPLES * 2]` — 128 stereo frames, 2 words each
//! - Each frame: `[left_sample_msb_aligned, right_sample_msb_aligned]`
//! - DMA runs in one-shot mode: the ISR fills the buffer and re-arms DMA
//! - One ISR call per audio block (128 samples)
//!
//! ## Usage with RTIC
//!
//! ```ignore
//! // In RTIC init: configure SAI1 + DMA, create the output node
//! let output = AudioOutputI2S::new(true);
//!
//! // In DMA ISR: fill the buffer and re-arm
//! let should_update = output.isr(&mut DMA_TX_BUFFER);
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

/// DMA buffer size in `u32` words: 2 words per stereo frame.
pub const DMA_BUFFER_WORDS: usize = AUDIO_BLOCK_SAMPLES * 2;

/// DMA-driven I2S stereo output node.
///
/// Implements [`AudioNode`] with 2 inputs (left, right) and 0 outputs.
///
/// This node uses double-buffering: [`update()`](AudioNode::update) queues audio blocks from the
/// graph, and the DMA ISR (via [`isr()`](Self::isr)) interleaves them into the DMA buffer.
/// Each ISR call consumes one full audio block (128 samples).
pub struct AudioOutputI2S {
    /// First block being actively transmitted (left channel).
    block_left_1st: Option<AudioBlockRef>,
    /// Second block queued for transmission (left channel).
    block_left_2nd: Option<AudioBlockRef>,
    /// First block being actively transmitted (right channel).
    block_right_1st: Option<AudioBlockRef>,
    /// Second block queued for transmission (right channel).
    block_right_2nd: Option<AudioBlockRef>,
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
            update_responsibility,
        }
    }

    /// Handle DMA interrupt — fill the entire DMA buffer with one audio block.
    ///
    /// Call this from the DMA completion ISR. It interleaves the current
    /// left/right audio blocks into the DMA buffer, then rotates the
    /// double-buffer queue.
    ///
    /// # Arguments
    ///
    /// - `dma_buffer`: The DMA transmit buffer (`[u32; AUDIO_BLOCK_SAMPLES * 2]`).
    ///
    /// # Returns
    ///
    /// `true` if the audio graph should be updated (i.e., `update_all()`
    /// should be called), when `update_responsibility` is set.
    pub fn isr(
        &mut self,
        dma_buffer: &mut [u32; AUDIO_BLOCK_SAMPLES * 2],
    ) -> bool {
        // Interleave audio data into the DMA buffer
        match (&self.block_left_1st, &self.block_right_1st) {
            (Some(left), Some(right)) => {
                interleave_lr(dma_buffer, &left[..], &right[..]);
            }
            (Some(left), None) => {
                interleave_l(dma_buffer, &left[..]);
            }
            (None, Some(right)) => {
                interleave_r(dma_buffer, &right[..]);
            }
            (None, None) => {
                dma_buffer.fill(0);
            }
        }

        // Rotate: consume 1st block, promote 2nd → 1st
        self.block_left_1st = self.block_left_2nd.take();
        self.block_right_1st = self.block_right_2nd.take();

        self.update_responsibility
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
            } else if self.block_left_2nd.is_none() {
                self.block_left_2nd = Some(block.clone());
            } else {
                // Both slots full — drop oldest, shift, add new
                self.block_left_1st = self.block_left_2nd.take();
                self.block_left_2nd = Some(block.clone());
            }
        }

        // Input 1 = right channel
        if let Some(ref block) = inputs[1] {
            if self.block_right_1st.is_none() {
                self.block_right_1st = Some(block.clone());
            } else if self.block_right_2nd.is_none() {
                self.block_right_2nd = Some(block.clone());
            } else {
                // Both slots full — drop oldest, shift, add new
                self.block_right_1st = self.block_right_2nd.take();
                self.block_right_2nd = Some(block.clone());
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
    }

    #[test]
    fn isr_silence_when_no_blocks() {
        let mut output = AudioOutputI2S::new(true);
        let mut dma_buf = [0xDEAD_BEEFu32; AUDIO_BLOCK_SAMPLES * 2];

        output.isr(&mut dma_buf);

        // Entire buffer should be zeroed (silence)
        for &sample in dma_buf.iter() {
            assert_eq!(sample, 0, "expected silence");
        }
    }

    #[test]
    fn isr_interleaves_both_channels() {
        reset_pool();
        let mut output = AudioOutputI2S::new(false);
        let left = make_block(100);
        let right = make_block(200);
        output.update(&[Some(left), Some(right)], &mut []);

        let mut dma_buf = [0u32; AUDIO_BLOCK_SAMPLES * 2];

        output.isr(&mut dma_buf);

        for i in 0..AUDIO_BLOCK_SAMPLES {
            assert_eq!(
                (dma_buf[i * 2] >> 16) as i16,
                100,
                "left mismatch at frame {i}"
            );
            assert_eq!(
                (dma_buf[i * 2 + 1] >> 16) as i16,
                200,
                "right mismatch at frame {i}"
            );
        }
    }

    #[test]
    fn isr_left_only_zeroes_right() {
        reset_pool();
        let mut output = AudioOutputI2S::new(false);
        let left = make_block(500);
        output.update(&[Some(left), None], &mut []);

        let mut dma_buf = [0u32; AUDIO_BLOCK_SAMPLES * 2];
        output.isr(&mut dma_buf);

        for i in 0..AUDIO_BLOCK_SAMPLES {
            assert_eq!((dma_buf[i * 2] >> 16) as i16, 500);
            assert_eq!(dma_buf[i * 2 + 1], 0);
        }
    }

    #[test]
    fn isr_rotates_blocks_after_consumption() {
        reset_pool();
        let mut output = AudioOutputI2S::new(false);
        let left1 = make_block(10);
        let left2 = make_block(20);
        output.update(&[Some(left1), None], &mut []);
        output.update(&[Some(left2), None], &mut []);

        let mut dma_buf = [0u32; AUDIO_BLOCK_SAMPLES * 2];

        // One ISR call consumes the first block entirely
        output.isr(&mut dma_buf);

        // Verify first block was used (value 10)
        assert_eq!((dma_buf[0] >> 16) as i16, 10);

        // Second block is now first
        assert!(output.block_left_1st.is_some());
        assert!(output.block_left_2nd.is_none());

        // Second ISR call uses the second block (value 20)
        output.isr(&mut dma_buf);
        assert_eq!((dma_buf[0] >> 16) as i16, 20);
        assert!(output.block_left_1st.is_none());
    }

    #[test]
    fn isr_signals_update_correctly() {
        let mut output_responsible = AudioOutputI2S::new(true);
        let mut output_not = AudioOutputI2S::new(false);
        let mut dma_buf = [0u32; AUDIO_BLOCK_SAMPLES * 2];

        assert!(output_responsible.isr(&mut dma_buf));
        assert!(!output_not.isr(&mut dma_buf));
    }

    #[test]
    fn isr_with_ramp_data() {
        reset_pool();
        let mut output = AudioOutputI2S::new(false);
        let left = make_ramp_block(0);
        let right = make_ramp_block(1000);
        output.update(&[Some(left), Some(right)], &mut []);

        let mut dma_buf = [0u32; AUDIO_BLOCK_SAMPLES * 2];

        output.isr(&mut dma_buf);

        for i in 0..AUDIO_BLOCK_SAMPLES {
            let expected_l = i as i16;
            let expected_r = 1000i16.wrapping_add(i as i16);
            assert_eq!(
                (dma_buf[i * 2] >> 16) as i16,
                expected_l,
                "left mismatch at frame {i}"
            );
            assert_eq!(
                (dma_buf[i * 2 + 1] >> 16) as i16,
                expected_r,
                "right mismatch at frame {i}"
            );
        }
    }
}
