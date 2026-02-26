use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};

use crate::constants::{AUDIO_BLOCK_SAMPLES, POOL_SIZE};

/// Raw audio block storage: 128 signed 16-bit samples, 4-byte aligned.
#[repr(C, align(4))]
pub struct AudioBlockData {
    pub samples: [i16; AUDIO_BLOCK_SAMPLES],
}

impl AudioBlockData {
    /// Create a zeroed audio block.
    const fn zeroed() -> Self {
        AudioBlockData {
            samples: [0i16; AUDIO_BLOCK_SAMPLES],
        }
    }
}

/// Global lock-free pool allocator for audio blocks.
///
/// Uses an atomic bitmap to track which slots are allocated, and per-slot
/// atomic reference counts for shared ownership. All operations are lock-free
/// and ISR-safe.
pub struct AudioBlockPool {
    /// Bitmap: bit N = 1 means slot N is allocated.
    bitmap: AtomicU32,
    /// Per-slot reference counts.
    refcounts: [AtomicU8; POOL_SIZE],
    /// Block storage.
    storage: UnsafeCell<[MaybeUninit<AudioBlockData>; POOL_SIZE]>,
}

// SAFETY: The pool uses atomic operations for all shared state.
// The UnsafeCell<storage> is only accessed through slot indices that are
// exclusively owned (via bitmap allocation) or shared (via refcount).
unsafe impl Sync for AudioBlockPool {}

impl AudioBlockPool {
    /// Create a new pool. All slots start unallocated.
    #[allow(clippy::declare_interior_mut_const)]
    const fn new() -> Self {
        const ZERO_REFCOUNT: AtomicU8 = AtomicU8::new(0);
        AudioBlockPool {
            bitmap: AtomicU32::new(0),
            refcounts: [ZERO_REFCOUNT; POOL_SIZE],
            storage: UnsafeCell::new(unsafe {
                MaybeUninit::<[MaybeUninit<AudioBlockData>; POOL_SIZE]>::zeroed().assume_init()
            }),
        }
    }

    /// Allocate a block from the pool. Returns the slot index, or `None` if full.
    ///
    /// The returned slot has refcount = 1 and its data is zeroed.
    pub fn alloc(&self) -> Option<u8> {
        loop {
            let bitmap = self.bitmap.load(Ordering::Acquire);
            let free = !bitmap;
            if free == 0 {
                return None; // all slots allocated
            }
            let slot = free.trailing_zeros();
            if slot >= POOL_SIZE as u32 {
                return None;
            }
            let bit = 1u32 << slot;
            // Try to claim this slot
            match self.bitmap.compare_exchange_weak(
                bitmap,
                bitmap | bit,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    // Slot claimed — initialize it
                    self.refcounts[slot as usize].store(1, Ordering::Release);
                    // Zero the block data
                    let storage = self.storage.get();
                    // SAFETY: We just exclusively claimed this slot via the bitmap CAS.
                    unsafe {
                        let block_ptr =
                            (*storage)[slot as usize].as_mut_ptr();
                        (*block_ptr) = AudioBlockData::zeroed();
                    }
                    return Some(slot as u8);
                }
                Err(_) => continue, // another core/ISR raced us, retry
            }
        }
    }

    /// Increment the reference count for a slot (used by `AudioBlockRef::clone`).
    ///
    /// # Panics
    /// Debug-asserts that the slot is currently allocated and refcount won't overflow.
    pub fn inc_ref(&self, slot: u8) {
        debug_assert!((slot as usize) < POOL_SIZE);
        let old = self.refcounts[slot as usize].fetch_add(1, Ordering::AcqRel);
        debug_assert!(old > 0, "inc_ref on unallocated slot");
        debug_assert!(old < 255, "refcount overflow");
    }

    /// Decrement the reference count for a slot. If it reaches zero, the slot
    /// is deallocated (bitmap bit cleared).
    pub fn dec_ref(&self, slot: u8) {
        debug_assert!((slot as usize) < POOL_SIZE);
        let old = self.refcounts[slot as usize].fetch_sub(1, Ordering::AcqRel);
        debug_assert!(old > 0, "dec_ref on slot with refcount 0");
        if old == 1 {
            // Refcount went from 1 to 0 — deallocate
            let bit = 1u32 << (slot as u32);
            self.bitmap.fetch_and(!bit, Ordering::Release);
        }
    }

    /// Get the current reference count for a slot.
    pub fn refcount(&self, slot: u8) -> u8 {
        self.refcounts[slot as usize].load(Ordering::Acquire)
    }

    /// Get a pointer to the block data for a given slot.
    ///
    /// # Safety
    /// Caller must ensure the slot is currently allocated.
    pub unsafe fn data_ptr(&self, slot: u8) -> *mut AudioBlockData {
        let storage = self.storage.get();
        unsafe { (*storage)[slot as usize].as_mut_ptr() }
    }

    /// Return the number of currently allocated blocks.
    pub fn allocated_count(&self) -> u32 {
        self.bitmap.load(Ordering::Acquire).count_ones()
    }

    /// Reset the pool to its initial state. For testing only.
    #[cfg(test)]
    pub fn reset(&self) {
        self.bitmap.store(0, Ordering::Release);
        for rc in &self.refcounts {
            rc.store(0, Ordering::Release);
        }
    }
}

/// The global audio block pool instance.
pub static POOL: AudioBlockPool = AudioBlockPool::new();

#[cfg(test)]
mod tests {
    use super::*;

    fn reset_pool() {
        POOL.reset();
    }

    #[test]
    fn alloc_returns_slot() {
        reset_pool();
        let slot = POOL.alloc();
        assert!(slot.is_some());
        let slot = slot.unwrap();
        assert!(slot < POOL_SIZE as u8);
        assert_eq!(POOL.refcount(slot), 1);
    }

    #[test]
    fn alloc_zeroes_data() {
        reset_pool();
        let slot = POOL.alloc().unwrap();
        unsafe {
            let data = &*POOL.data_ptr(slot);
            for &s in data.samples.iter() {
                assert_eq!(s, 0);
            }
        }
    }

    #[test]
    fn alloc_unique_slots() {
        reset_pool();
        let mut slots = [0u8; POOL_SIZE];
        for s in slots.iter_mut() {
            *s = POOL.alloc().unwrap();
        }
        // All slots should be unique
        slots.sort();
        for i in 0..POOL_SIZE - 1 {
            assert_ne!(slots[i], slots[i + 1]);
        }
    }

    #[test]
    fn alloc_exhaustion() {
        reset_pool();
        for _ in 0..POOL_SIZE {
            assert!(POOL.alloc().is_some());
        }
        assert!(POOL.alloc().is_none());
    }

    #[test]
    fn dealloc_frees_slot() {
        reset_pool();
        let slot = POOL.alloc().unwrap();
        assert_eq!(POOL.allocated_count(), 1);
        POOL.dec_ref(slot);
        assert_eq!(POOL.allocated_count(), 0);
        // Can allocate again
        let slot2 = POOL.alloc().unwrap();
        assert!(slot2 < POOL_SIZE as u8);
    }

    #[test]
    fn refcount_lifecycle() {
        reset_pool();
        let slot = POOL.alloc().unwrap();
        assert_eq!(POOL.refcount(slot), 1);

        POOL.inc_ref(slot);
        assert_eq!(POOL.refcount(slot), 2);

        POOL.dec_ref(slot);
        assert_eq!(POOL.refcount(slot), 1);
        assert_eq!(POOL.allocated_count(), 1); // still allocated

        POOL.dec_ref(slot);
        assert_eq!(POOL.allocated_count(), 0); // now freed
    }
}
