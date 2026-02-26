//! Graph-to-user audio queue.
//!
//! [`AudioRecordQueue`] allows user code to read audio blocks captured by
//! the processing graph. This is useful for recording, analysis, streaming
//! to external storage, or any case where user code needs to inspect the
//! audio data produced by the graph.
//!
//! ## Usage
//!
//! ```ignore
//! let mut record_queue = AudioRecordQueue::new();
//! record_queue.start(); // Begin recording
//!
//! // In audio update task:
//! record_queue.update(&[Some(input_block)], &mut []);
//!
//! // In user code (e.g., RTIC idle or low-priority task):
//! while let Some(block) = record_queue.read() {
//!     // Process the captured block...
//! }
//!
//! record_queue.stop(); // Stop recording
//! ```

use crate::block::{AudioBlockMut, AudioBlockRef};
use crate::node::AudioNode;

use super::spsc::SpscQueue;

/// Queue capacity: 4 usable slots + 1 sentinel = 5 total.
const QUEUE_SIZE: usize = 5;

/// Allows user code to read audio blocks captured by the processing graph.
///
/// Implements [`AudioNode`] with 1 input and 0 outputs.
///
/// Internally uses a lock-free SPSC ring buffer, so [`read()`](Self::read)
/// can be called from a different priority context than
/// [`update()`](AudioNode::update).
///
/// Recording must be explicitly started with [`start()`](Self::start).
/// When not recording, incoming blocks are silently discarded.
pub struct AudioRecordQueue {
    queue: SpscQueue<AudioBlockRef, QUEUE_SIZE>,
    recording: bool,
}

impl AudioRecordQueue {
    /// Create a new record queue (recording is initially stopped).
    pub const fn new() -> Self {
        AudioRecordQueue {
            queue: SpscQueue::new(),
            recording: false,
        }
    }

    /// Start recording. Incoming blocks will be enqueued until [`stop()`](Self::stop).
    pub fn start(&mut self) {
        self.recording = true;
    }

    /// Stop recording. No more blocks will be enqueued.
    ///
    /// Blocks already in the queue can still be read with [`read()`](Self::read).
    pub fn stop(&mut self) {
        self.recording = false;
    }

    /// Whether recording is currently active.
    pub fn is_recording(&self) -> bool {
        self.recording
    }

    /// Read a captured audio block from the queue.
    ///
    /// Returns `None` if the queue is empty.
    ///
    /// This method takes `&self` and is safe to call from a different priority
    /// context than `update()` (single-producer single-consumer guarantee).
    pub fn read(&self) -> Option<AudioBlockRef> {
        self.queue.pop()
    }

    /// Check if there are captured blocks waiting to be read.
    pub fn available(&self) -> bool {
        !self.queue.is_empty()
    }

    /// Return the number of captured blocks waiting to be read.
    pub fn len(&self) -> usize {
        self.queue.len()
    }
}

impl AudioNode for AudioRecordQueue {
    const NUM_INPUTS: usize = 1;
    const NUM_OUTPUTS: usize = 0;

    fn update(
        &mut self,
        inputs: &[Option<AudioBlockRef>],
        _outputs: &mut [Option<AudioBlockMut>],
    ) {
        if !self.recording {
            return;
        }
        if let Some(ref block) = inputs[0] {
            // Enqueue the block. If the queue is full, the block is silently dropped.
            let _ = self.queue.push(block.clone());
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

    fn make_block(value: i16) -> AudioBlockRef {
        let mut block = AudioBlockMut::alloc().unwrap();
        block.fill(value);
        block.into_shared()
    }

    #[test]
    fn new_is_stopped_and_empty() {
        let q = AudioRecordQueue::new();
        assert!(!q.is_recording());
        assert!(!q.available());
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn start_stop() {
        let mut q = AudioRecordQueue::new();
        q.start();
        assert!(q.is_recording());
        q.stop();
        assert!(!q.is_recording());
    }

    #[test]
    fn discards_when_not_recording() {
        reset_pool();
        let mut q = AudioRecordQueue::new();
        let block = make_block(42);

        q.update(&[Some(block)], &mut []);
        assert!(q.read().is_none());
    }

    #[test]
    fn records_when_active() {
        reset_pool();
        let mut q = AudioRecordQueue::new();
        q.start();

        let block = make_block(77);
        q.update(&[Some(block)], &mut []);

        assert!(q.available());
        assert_eq!(q.len(), 1);

        let recorded = q.read().unwrap();
        assert_eq!(recorded[0], 77);
        assert_eq!(recorded[127], 77);
    }

    #[test]
    fn fifo_ordering() {
        reset_pool();
        let mut q = AudioRecordQueue::new();
        q.start();

        let b1 = make_block(1);
        let b2 = make_block(2);
        let b3 = make_block(3);

        q.update(&[Some(b1)], &mut []);
        q.update(&[Some(b2)], &mut []);
        q.update(&[Some(b3)], &mut []);
        assert_eq!(q.len(), 3);

        assert_eq!(q.read().unwrap()[0], 1);
        assert_eq!(q.read().unwrap()[0], 2);
        assert_eq!(q.read().unwrap()[0], 3);
        assert!(q.read().is_none());
    }

    #[test]
    fn full_queue_drops_silently() {
        reset_pool();
        let mut q = AudioRecordQueue::new();
        q.start();

        // Fill all 4 usable slots
        for i in 0..4 {
            let block = make_block(i);
            q.update(&[Some(block)], &mut []);
        }
        assert_eq!(q.len(), 4);

        // 5th block should be silently dropped
        let block = make_block(99);
        q.update(&[Some(block)], &mut []);
        assert_eq!(q.len(), 4);

        // Verify only the first 4 are present
        for i in 0..4 {
            assert_eq!(q.read().unwrap()[0], i);
        }
        assert!(q.read().is_none());
    }

    #[test]
    fn read_after_stop_returns_remaining() {
        reset_pool();
        let mut q = AudioRecordQueue::new();
        q.start();

        let b1 = make_block(10);
        let b2 = make_block(20);
        q.update(&[Some(b1)], &mut []);
        q.update(&[Some(b2)], &mut []);

        q.stop();

        // Blocks already enqueued should still be readable
        assert_eq!(q.read().unwrap()[0], 10);
        assert_eq!(q.read().unwrap()[0], 20);
        assert!(q.read().is_none());
    }

    #[test]
    fn none_input_ignored() {
        let mut q = AudioRecordQueue::new();
        q.start();

        q.update(&[None], &mut []);
        assert!(!q.available());
    }
}
