//! # TeensyAudio-rs
//!
//! A `no_std`, zero-allocation audio processing framework for the
//! [Teensy 4.x](https://www.pjrc.com/teensy/) (i.MX RT1062, Cortex-M7) written
//! in pure Rust. It provides the same node-graph programming model as the
//! [PJRC Teensy Audio Library](https://www.pjrc.com/teensy/td_libs_Audio.html)
//! but leverages Rust's type system for compile-time safety.
//!
//! ## Architecture
//!
//! | Layer | Module | Purpose |
//! |-------|--------|---------|
//! | Memory | [`block`] | Fixed-size audio block pool with refcounted handles |
//! | Trait | [`node`] / [`control`] | `AudioNode` and `AudioControl` traits |
//! | I/O | [`io`] | I²S input/output, play/record queues |
//! | Codec | [`codec`] | SGTL5000 codec driver (feature-gated) |
//! | DSP | [`dsp`] / [`nodes`] | Synthesis, effects, analysis (feature-gated) |
//! | Graph | [`graph`] | [`audio_graph!`] macro for declarative wiring |
//!
//! ## Quick start
//!
//! ```ignore
//! use teensy_audio::audio_graph;
//! use teensy_audio::nodes::*;
//!
//! // Declare a graph: sine → amplifier → peak analyzer
//! audio_graph! {
//!     pub struct MyGraph {
//!         sine: AudioSynthSine {},
//!         amp:  AudioAmplifier  { (sine, 0) },
//!         peak: AudioAnalyzePeak { (amp, 0) },
//!     }
//! }
//!
//! let mut g = MyGraph::new();
//! g.sine.frequency(440.0);
//! g.sine.amplitude(1.0);
//! g.amp.gain(0.5);
//!
//! // In your audio ISR / timer callback:
//! g.update_all();
//!
//! if g.peak.available() {
//!     let level = g.peak.read();
//! }
//! ```
//!
//! ## Features
//!
//! | Feature | Default | Enables |
//! |---------|---------|---------|
//! | `dsp` | yes | DSP math utilities, synthesis/effect/analysis nodes |
//! | `sgtl5000` | yes | SGTL5000 codec driver (requires `embedded-hal`) |
//!
//! ## Audio parameters
//!
//! - **Block size:** 128 samples ([`constants::AUDIO_BLOCK_SAMPLES`])
//! - **Sample rate:** 44 117.647 Hz ([`constants::AUDIO_SAMPLE_RATE`])
//! - **Sample format:** `i16` (signed 16-bit)
//! - **Block pool:** 32 blocks ([`constants::AUDIO_MEMORY_BLOCKS`])

#![no_std]

pub mod constants;
pub mod block;
pub mod node;
pub mod control;
pub mod io;
pub mod graph;

#[cfg(feature = "sgtl5000")]
pub mod codec;

#[cfg(feature = "dsp")]
pub mod dsp;

#[cfg(feature = "dsp")]
pub mod nodes;
