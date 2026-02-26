//! User-to-graph audio queue.
//!
//! [`AudioPlayQueue`] allows user code (non-ISR context) to inject audio blocks
//! into the processing graph. This is useful for streaming pre-computed audio,
//! test tones, or data from external sources.
//!
//! ## Usage
//!
//! ```ignore
//! let play_queue = AudioPlayQueue::new();
//!
//! // In user code (e.g., RTIC idle or low-priority task):
//! let mut block = AudioBlockMut::alloc().unwrap();
//! // Fill block with audio data...
//! play_queue.play(block).unwrap();
//!
//! // In audio update task:
//! let mut outputs = [None];
//! play_queue.update(&[], &mut outputs);
//! // outputs[0] contains the dequeued block
//! ```

use crate::block::{AudioBlockMut, AudioBlockRef};
use crate::node::AudioNode;

use super::spsc::SpscQueue;

/// Queue capacity: 4 usable slots + 1 sentinel = 5 total.
const QUEUE_SIZE: usize = 5;

/// Allows user code to inject audio blocks into the processing graph.
///
/// Implements [`AudioNode`] with 0 inputs and 1 output.
///
/// Internally uses a lock-free SPSC ring buffer, so [`play()`](Self::play)
/// can be called from a different priority context than
/// [`update()`](AudioNode::update).
///
/// The producer (user code) calls `play()` to enqueue blocks.
/// The consumer (audio graph) calls `update()` to dequeue one block per cycle.
pub struct AudioPlayQueue {
    queue: SpscQueue<AudioBlockMut, QUEUE_SIZE>,
}

impl AudioPlayQueue {
    /// Create a new play queue.
    pub const fn new() -> Self {
        AudioPlayQueue {
            queue: SpscQueue::new(),
        }
    }

    /// Enqueue an audio block for playback.
    ///
    /// The block is transferred to the audio graph on the next `update()` call.
    /// Returns `Err(block)` if the queue is full (caller retains ownership).
    ///
    /// This method takes `&self` and is safe to call from a different priority
    /// context than `update()` (single-producer single-consumer guarantee).
    pub fn play(&self, block: AudioBlockMut) -> Result<(), AudioBlockMut> {
        self.queue.push(block)
    }

    /// Check if the queue has blocks waiting for playback.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Return the number of blocks currently queued.
    pub fn len(&self) -> usize {
        self.queue.len()
    }
}

impl AudioNode for AudioPlayQueue {
    const NUM_INPUTS: usize = 0;
    const NUM_OUTPUTS: usize = 1;

    fn update(
        &mut self,
        _inputs: &[Option<AudioBlockRef>],
        outputs: &mut [Option<AudioBlockMut>],
    ) {
        if let Some(block) = self.queue.pop() {
            outputs[0] = Some(block);
        }
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
    fn new_is_empty() {
        let q = AudioPlayQueue::new();
        assert!(q.is_empty());
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn play_and_update() {
        reset_pool();
        let mut q = AudioPlayQueue::new();

        let mut block = AudioBlockMut::alloc().unwrap();
        block[0] = 42;
        block[127] = -99;

        q.play(block).unwrap();
        assert_eq!(q.len(), 1);

        let mut outputs = [None];
        q.update(&[], &mut outputs);

        assert!(outputs[0].is_some());
        let out = outputs[0].as_ref().unwrap();
        assert_eq!(out[0], 42);
        assert_eq!(out[127], -99);
    }

    #[test]
    fn update_empty_produces_none() {
        let mut q = AudioPlayQueue::new();
        let mut outputs = [None];

        q.update(&[], &mut outputs);

        assert!(outputs[0].is_none());
    }

    #[test]
    fn fifo_ordering() {
        reset_pool();
        let mut q = AudioPlayQueue::new();

        let mut b1 = AudioBlockMut::alloc().unwrap();
        b1[0] = 1;
        let mut b2 = AudioBlockMut::alloc().unwrap();
        b2[0] = 2;
        let mut b3 = AudioBlockMut::alloc().unwrap();
        b3[0] = 3;

        q.play(b1).unwrap();
        q.play(b2).unwrap();
        q.play(b3).unwrap();
        assert_eq!(q.len(), 3);

        let mut outputs = [None];

        q.update(&[], &mut outputs);
        assert_eq!(outputs[0].as_ref().unwrap()[0], 1);
        outputs[0] = None;

        q.update(&[], &mut outputs);
        assert_eq!(outputs[0].as_ref().unwrap()[0], 2);
        outputs[0] = None;

        q.update(&[], &mut outputs);
        assert_eq!(outputs[0].as_ref().unwrap()[0], 3);
    }

    #[test]
    fn full_queue_rejects() {
        reset_pool();
        let q = AudioPlayQueue::new();

        // Fill all 4 usable slots
        for i in 0..4 {
            let mut block = AudioBlockMut::alloc().unwrap();
            block[0] = i;
            q.play(block).unwrap();
        }

        // 5th push should fail
        let mut block = AudioBlockMut::alloc().unwrap();
        block[0] = 99;
        let result = q.play(block);
        assert!(result.is_err());

        // Verify the rejected block is returned
        let rejected = result.unwrap_err();
        assert_eq!(rejected[0], 99);
    }
}
