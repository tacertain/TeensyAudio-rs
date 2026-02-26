//! I/O drivers for the audio processing graph.
//!
//! This module provides DMA-driven I2S input/output nodes and user-facing
//! queue buffers for injecting/extracting audio data.
//!
//! ## Components
//!
//! | Node | Inputs | Outputs | Description |
//! |------|--------|---------|-------------|
//! | [`AudioOutputI2S`] | 2 (L, R) | 0 | DMA-driven I2S stereo output |
//! | [`AudioInputI2S`] | 0 | 2 (L, R) | DMA-driven I2S stereo input |
//! | [`AudioPlayQueue`] | 0 | 1 | User code → audio graph |
//! | [`AudioRecordQueue`] | 1 | 0 | Audio graph → user code |
//!
//! ## Utilities
//!
//! - [`interleave`] — Stereo interleave/deinterleave for DMA buffers
//! - [`spsc`] — Lock-free single-producer single-consumer ring buffer
//!
//! ## DMA Buffer Layout
//!
//! The I2S drivers use circular DMA buffers of `[u32; 128]`:
//! - Each `u32` = one stereo frame (left in lower 16 bits, right in upper 16)
//! - Buffer is split into two halves (64 frames each)
//! - DMA fires half-complete and complete interrupts
//! - ISR fills/reads the inactive half while DMA operates on the active half

pub mod interleave;
pub mod spsc;
pub mod output_i2s;
pub mod input_i2s;
pub mod play_queue;
pub mod record_queue;

pub use output_i2s::{AudioOutputI2S, DmaHalf};
pub use input_i2s::AudioInputI2S;
pub use play_queue::AudioPlayQueue;
pub use record_queue::AudioRecordQueue;

#[cfg(test)]
mod integration_tests;
