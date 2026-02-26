use core::ops::{Deref, DerefMut};

use crate::constants::AUDIO_BLOCK_SAMPLES;

use super::pool::POOL;

/// Exclusive (mutable) handle to an audio block in the pool.
///
/// There is exactly one `AudioBlockMut` per allocated slot.
/// Provides `DerefMut` access to the underlying `[i16; 128]` samples.
/// Dropping an `AudioBlockMut` decrements the refcount (and frees the slot if it reaches zero).
pub struct AudioBlockMut {
    slot: u8,
}

impl AudioBlockMut {
    /// Create a new `AudioBlockMut` for the given pool slot.
    ///
    /// # Safety
    /// The caller must ensure the slot was just allocated with refcount = 1
    /// and no other `AudioBlockMut` or `AudioBlockRef` exists for this slot.
    pub(crate) fn new(slot: u8) -> Self {
        AudioBlockMut { slot }
    }

    /// Convert this exclusive reference into a shared reference.
    /// This is a zero-cost conversion (no data copy, no refcount change).
    pub fn into_shared(self) -> AudioBlockRef {
        let slot = self.slot;
        core::mem::forget(self); // don't run Drop (don't dec_ref)
        AudioBlockRef { slot }
    }

    /// Get the pool slot index.
    pub fn slot(&self) -> u8 {
        self.slot
    }

    /// Allocate a new audio block from the global pool.
    /// Returns `None` if the pool is exhausted.
    pub fn alloc() -> Option<Self> {
        POOL.alloc().map(AudioBlockMut::new)
    }
}

impl Deref for AudioBlockMut {
    type Target = [i16; AUDIO_BLOCK_SAMPLES];

    fn deref(&self) -> &Self::Target {
        // SAFETY: We hold exclusive access (refcount == 1, unique AudioBlockMut).
        unsafe { &(*POOL.data_ptr(self.slot)).samples }
    }
}

impl DerefMut for AudioBlockMut {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: We hold exclusive access (refcount == 1, unique AudioBlockMut).
        unsafe { &mut (*POOL.data_ptr(self.slot)).samples }
    }
}

impl Drop for AudioBlockMut {
    fn drop(&mut self) {
        POOL.dec_ref(self.slot);
    }
}

/// Shared (immutable) handle to an audio block in the pool.
///
/// Multiple `AudioBlockRef`s can point to the same slot. Cloning increments the
/// refcount; dropping decrements it. When the last reference is dropped, the
/// pool slot is freed.
pub struct AudioBlockRef {
    slot: u8,
}

impl AudioBlockRef {
    /// Get the pool slot index.
    pub fn slot(&self) -> u8 {
        self.slot
    }

    /// Try to convert back to an exclusive mutable reference.
    ///
    /// - If this is the only reference (refcount == 1), converts in place (no copy).
    /// - If there are other references, allocates a new block, copies the data,
    ///   and returns the new exclusive block. Returns `None` if the pool is exhausted.
    pub fn into_mut(self) -> Option<AudioBlockMut> {
        let refcount = POOL.refcount(self.slot);
        if refcount == 1 {
            // We're the sole owner — convert in place
            let slot = self.slot;
            core::mem::forget(self);
            Some(AudioBlockMut::new(slot))
        } else {
            // Clone-on-write: allocate a new block and copy
            let new_slot = POOL.alloc()?;
            unsafe {
                let src = &(*POOL.data_ptr(self.slot)).samples;
                let dst = &mut (*POOL.data_ptr(new_slot)).samples;
                *dst = *src;
            }
            // Drop self (decrements refcount on old slot)
            drop(self);
            Some(AudioBlockMut::new(new_slot))
        }
    }
}

impl Deref for AudioBlockRef {
    type Target = [i16; AUDIO_BLOCK_SAMPLES];

    fn deref(&self) -> &Self::Target {
        // SAFETY: Slot is allocated and data is immutable through shared references.
        unsafe { &(*POOL.data_ptr(self.slot)).samples }
    }
}

impl Clone for AudioBlockRef {
    fn clone(&self) -> Self {
        POOL.inc_ref(self.slot);
        AudioBlockRef { slot: self.slot }
    }
}

impl Drop for AudioBlockRef {
    fn drop(&mut self) {
        POOL.dec_ref(self.slot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::pool::POOL;

    fn reset_pool() {
        POOL.reset();
    }

    #[test]
    fn alloc_and_drop() {
        reset_pool();
        {
            let block = AudioBlockMut::alloc().unwrap();
            assert_eq!(POOL.allocated_count(), 1);
            assert_eq!(POOL.refcount(block.slot()), 1);
        }
        assert_eq!(POOL.allocated_count(), 0);
    }

    #[test]
    fn write_and_read() {
        reset_pool();
        let mut block = AudioBlockMut::alloc().unwrap();
        block[0] = 1234;
        block[127] = -5678;
        assert_eq!(block[0], 1234);
        assert_eq!(block[127], -5678);
    }

    #[test]
    fn into_shared() {
        reset_pool();
        let mut block = AudioBlockMut::alloc().unwrap();
        block[0] = 42;
        let slot = block.slot();

        let shared = block.into_shared();
        assert_eq!(shared.slot(), slot);
        assert_eq!(shared[0], 42);
        assert_eq!(POOL.refcount(slot), 1); // no extra ref
        assert_eq!(POOL.allocated_count(), 1);
    }

    #[test]
    fn shared_clone_and_drop() {
        reset_pool();
        let mut block = AudioBlockMut::alloc().unwrap();
        block[0] = 99;
        let slot = block.slot();
        let shared = block.into_shared();

        let shared2 = shared.clone();
        assert_eq!(POOL.refcount(slot), 2);
        assert_eq!(shared2[0], 99);

        drop(shared);
        assert_eq!(POOL.refcount(slot), 1);
        assert_eq!(POOL.allocated_count(), 1);

        drop(shared2);
        assert_eq!(POOL.allocated_count(), 0);
    }

    #[test]
    fn into_mut_sole_owner() {
        reset_pool();
        let mut block = AudioBlockMut::alloc().unwrap();
        block[0] = 77;
        let slot = block.slot();
        let shared = block.into_shared();

        // sole owner => convert in place
        let mut exclusive = shared.into_mut().unwrap();
        assert_eq!(exclusive.slot(), slot); // same slot
        assert_eq!(exclusive[0], 77);
        exclusive[0] = 88;
        assert_eq!(exclusive[0], 88);
    }

    #[test]
    fn into_mut_clone_on_write() {
        reset_pool();
        let mut block = AudioBlockMut::alloc().unwrap();
        block[0] = 55;
        let slot = block.slot();
        let shared = block.into_shared();
        let shared2 = shared.clone();
        assert_eq!(POOL.refcount(slot), 2);

        // multiple owners => clone-on-write
        let mut exclusive = shared.into_mut().unwrap();
        assert_ne!(exclusive.slot(), slot); // different slot (new allocation)
        assert_eq!(exclusive[0], 55); // data was copied
        exclusive[0] = 66;

        // Original shared ref is unaffected
        assert_eq!(shared2[0], 55);
        assert_eq!(POOL.refcount(slot), 1); // old slot refcount decremented
    }
}
