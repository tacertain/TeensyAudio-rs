//! SGTL5000 audio codec driver module.
//!
//! Provides a full-featured driver for the NXP SGTL5000 codec found on the
//! Teensy Audio Shield. Ported from the C++ `AudioControlSGTL5000` class.
//!
//! # Feature gate
//!
//! This module is available when the `sgtl5000` feature is enabled (on by default).

pub(crate) mod registers;
mod sgtl5000;

pub use sgtl5000::{EqMode, HeadphoneSource, Input, Sgtl5000};
