//! Lock-free single-producer single-consumer (SPSC) ring buffer.
//!
//! Designed for ISR-safe communication between the audio update task
//! and user code. Uses atomic indices for lock-free synchronization.
//!
//! # Safety Contract
//!
//! - Only ONE thread/context may call [`push()`](SpscQueue::push) (the "producer").
//! - Only ONE thread/context may call [`pop()`](SpscQueue::pop) (the "consumer").
//! - These may be different threads/ISR contexts running concurrently.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicUsize, Ordering};

/// A lock-free single-producer single-consumer (SPSC) queue.
///
/// The usable capacity is `N - 1` (one slot is reserved for full/empty
/// disambiguation via the Lamport queue algorithm).
///
/// # Type Parameters
///
/// - `T`: The element type. Must be `Send` for cross-context safety.
/// - `N`: Total number of slots. Usable capacity is `N - 1`. Must be ≥ 2.
pub struct SpscQueue<T, const N: usize> {
    buffer: [UnsafeCell<MaybeUninit<T>>; N],
    /// Write position (only modified by the producer).
    head: AtomicUsize,
    /// Read position (only modified by the consumer).
    tail: AtomicUsize,
}

// SAFETY: T: Send is required because values cross thread/ISR boundaries.
// The SPSC contract (single producer, single consumer) ensures that
// head and tail are only modified by their respective sides, and
// atomic ordering guarantees visibility of buffer writes.
unsafe impl<T: Send, const N: usize> Sync for SpscQueue<T, N> {}
unsafe impl<T: Send, const N: usize> Send for SpscQueue<T, N> {}

impl<T, const N: usize> SpscQueue<T, N> {
    /// Create a new empty queue.
    ///
    /// # Panics
    ///
    /// Compile-time assertion: `N` must be at least 2 (usable capacity is `N - 1`).
    pub const fn new() -> Self {
        assert!(N >= 2, "SPSC queue must have at least 2 slots (1 usable)");

        SpscQueue {
            // SAFETY: An array of uninitialized MaybeUninit<T> is always valid.
            // UnsafeCell is a transparent wrapper that doesn't affect validity.
            buffer: unsafe {
                MaybeUninit::<[UnsafeCell<MaybeUninit<T>>; N]>::uninit().assume_init()
            },
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Push a value into the queue (producer side).
    ///
    /// Returns `Err(val)` if the queue is full, returning ownership to the caller.
    pub fn push(&self, val: T) -> Result<(), T> {
        let head = self.head.load(Ordering::Relaxed);
        let next_head = (head + 1) % N;

        if next_head == self.tail.load(Ordering::Acquire) {
            return Err(val); // Queue is full
        }

        // SAFETY: We are the sole producer  and `head` is only advanced by us.
        // `next_head != tail` guarantees this slot is not occupied by the consumer.
        unsafe {
            (*self.buffer[head].get()).write(val);
        }

        // Release ordering ensures the buffer write is visible before head advances.
        self.head.store(next_head, Ordering::Release);
        Ok(())
    }

    /// Pop a value from the queue (consumer side).
    ///
    /// Returns `None` if the queue is empty.
    pub fn pop(&self) -> Option<T> {
        let tail = self.tail.load(Ordering::Relaxed);

        if tail == self.head.load(Ordering::Acquire) {
            return None; // Queue is empty
        }

        // SAFETY: We are the sole consumer and `tail` is only advanced by us.
        // `tail != head` guarantees this slot contains a valid value.
        let val = unsafe { (*self.buffer[tail].get()).assume_init_read() };

        // Release ordering ensures the read completes before tail advances,
        // freeing the slot for the producer.
        self.tail.store((tail + 1) % N, Ordering::Release);
        Some(val)
    }

    /// Check if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.tail.load(Ordering::Acquire) == self.head.load(Ordering::Acquire)
    }

    /// Check if the queue is full.
    pub fn is_full(&self) -> bool {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        (head + 1) % N == tail
    }

    /// Return the number of items currently in the queue.
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        (head + N - tail) % N
    }
}

impl<T, const N: usize> Drop for SpscQueue<T, N> {
    fn drop(&mut self) {
        // Drop any remaining items to avoid leaks.
        while self.pop().is_some() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_pop() {
        let q: SpscQueue<i32, 4> = SpscQueue::new(); // capacity 3
        assert!(q.is_empty());
        assert_eq!(q.len(), 0);

        q.push(10).unwrap();
        assert_eq!(q.len(), 1);
        assert!(!q.is_empty());

        q.push(20).unwrap();
        q.push(30).unwrap();
        assert_eq!(q.len(), 3);
        assert!(q.is_full());

        // Queue is full — push should fail
        assert_eq!(q.push(40), Err(40));

        assert_eq!(q.pop(), Some(10));
        assert_eq!(q.pop(), Some(20));
        assert_eq!(q.pop(), Some(30));
        assert_eq!(q.pop(), None);
        assert!(q.is_empty());
    }

    #[test]
    fn empty_pop_returns_none() {
        let q: SpscQueue<u8, 3> = SpscQueue::new();
        assert_eq!(q.pop(), None);
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn single_slot_queue() {
        // N=2 means 1 usable slot
        let q: SpscQueue<i32, 2> = SpscQueue::new();
        q.push(42).unwrap();
        assert!(q.is_full());
        assert_eq!(q.push(99), Err(99));
        assert_eq!(q.pop(), Some(42));
        assert!(q.is_empty());
    }

    #[test]
    fn wraparound() {
        let q: SpscQueue<i32, 3> = SpscQueue::new(); // capacity 2

        // Fill and drain multiple times to wrap indices around
        for round in 0..10 {
            let base = round * 100;
            q.push(base + 1).unwrap();
            q.push(base + 2).unwrap();
            assert!(q.is_full());

            assert_eq!(q.pop(), Some(base + 1));
            assert_eq!(q.pop(), Some(base + 2));
            assert!(q.is_empty());
        }
    }

    #[test]
    fn interleaved_push_pop() {
        let q: SpscQueue<i32, 4> = SpscQueue::new(); // capacity 3

        q.push(1).unwrap();
        q.push(2).unwrap();
        assert_eq!(q.pop(), Some(1));

        q.push(3).unwrap();
        q.push(4).unwrap();
        assert_eq!(q.pop(), Some(2));
        assert_eq!(q.pop(), Some(3));
        assert_eq!(q.pop(), Some(4));
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn len_tracks_correctly() {
        let q: SpscQueue<i32, 5> = SpscQueue::new(); // capacity 4
        assert_eq!(q.len(), 0);

        q.push(1).unwrap();
        assert_eq!(q.len(), 1);

        q.push(2).unwrap();
        assert_eq!(q.len(), 2);

        q.pop();
        assert_eq!(q.len(), 1);

        q.pop();
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn drop_cleans_up_remaining() {
        use core::sync::atomic::{AtomicUsize, Ordering};

        static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

        #[derive(Debug)]
        struct Trackable;
        impl Drop for Trackable {
            fn drop(&mut self) {
                DROP_COUNT.fetch_add(1, Ordering::Relaxed);
            }
        }

        DROP_COUNT.store(0, Ordering::Relaxed);
        {
            let q: SpscQueue<Trackable, 4> = SpscQueue::new();
            q.push(Trackable).unwrap();
            q.push(Trackable).unwrap();
            // Drop q with 2 items still inside
        }
        assert_eq!(DROP_COUNT.load(Ordering::Relaxed), 2);
    }
}
