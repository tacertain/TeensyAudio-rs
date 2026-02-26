//! RMS (root-mean-square) level meter.
//!
//! Port of `TeensyAudio/analyze_rms.cpp`. Computes the RMS level over
//! one or more block periods.

use crate::block::{AudioBlockMut, AudioBlockRef};
use crate::constants::AUDIO_BLOCK_SAMPLES;
use crate::node::AudioNode;

/// RMS level meter. Analyzer node: 1 input, 0 outputs.
///
/// Accumulates sum-of-squares over one or more blocks, then computes
/// `sqrt(mean_square) / 32767` on `read()`.
///
/// # Example
/// ```ignore
/// let mut rms = AudioAnalyzeRms::new();
/// // ... after processing ...
/// if rms.available() {
///     let level = rms.read(); // 0.0–1.0
/// }
/// ```
pub struct AudioAnalyzeRms {
    /// Running sum of squared samples.
    accum: u64,
    /// Number of samples accumulated.
    count: u32,
    /// Whether new data is available since last read.
    new_output: bool,
}

impl AudioAnalyzeRms {
    /// Create a new RMS analyzer.
    pub const fn new() -> Self {
        AudioAnalyzeRms {
            accum: 0,
            count: 0,
            new_output: false,
        }
    }

    /// Returns `true` if new data has been accumulated since the last `read()`.
    pub fn available(&self) -> bool {
        self.new_output
    }

    /// Read the RMS level (0.0–1.0) and reset the accumulator.
    ///
    /// If no samples have been accumulated, returns 0.0.
    pub fn read(&mut self) -> f32 {
        let sum = self.accum;
        let num = self.count;
        self.accum = 0;
        self.count = 0;
        self.new_output = false;

        if num == 0 {
            return 0.0;
        }

        let mean_sq = sum as f64 / num as f64;
        let rms = libm::sqrt(mean_sq);
        (rms / 32767.0) as f32
    }
}

impl AudioNode for AudioAnalyzeRms {
    const NUM_INPUTS: usize = 1;
    const NUM_OUTPUTS: usize = 0;

    fn update(
        &mut self,
        inputs: &[Option<AudioBlockRef>],
        _outputs: &mut [Option<AudioBlockMut>],
    ) {
        match inputs[0] {
            Some(ref input) => {
                let mut sum = self.accum;
                for i in 0..AUDIO_BLOCK_SAMPLES {
                    let s = input[i] as i64;
                    sum += (s * s) as u64;
                }
                self.accum = sum;
                self.count += AUDIO_BLOCK_SAMPLES as u32;
                self.new_output = true;
            }
            None => {
                // No input: count silent samples (zeros contribute nothing to sum)
                self.count += AUDIO_BLOCK_SAMPLES as u32;
                self.new_output = true;
            }
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
    fn rms_no_data() {
        let mut rms = AudioAnalyzeRms::new();
        assert!(!rms.available());
        assert_eq!(rms.read(), 0.0);
    }

    #[test]
    fn rms_silence() {
        reset_pool();
        let mut rms = AudioAnalyzeRms::new();

        let input = AudioBlockMut::alloc().unwrap();
        // Block is zero-initialized by default? No, need to fill.
        let mut block = input;
        block.fill(0);
        let input_ref = block.into_shared();
        let inputs = [Some(input_ref)];
        let mut outputs: [Option<AudioBlockMut>; 0] = [];

        rms.update(&inputs, &mut outputs);

        assert!(rms.available());
        let level = rms.read();
        assert_eq!(level, 0.0);
    }

    #[test]
    fn rms_full_scale_dc() {
        reset_pool();
        let mut rms = AudioAnalyzeRms::new();

        let mut block = AudioBlockMut::alloc().unwrap();
        block.fill(32767); // full positive DC
        let input_ref = block.into_shared();
        let inputs = [Some(input_ref)];
        let mut outputs: [Option<AudioBlockMut>; 0] = [];

        rms.update(&inputs, &mut outputs);

        let level = rms.read();
        // RMS of constant 32767 = 32767, normalized = 1.0
        assert!((level - 1.0).abs() < 0.001, "expected ~1.0, got {}", level);
    }

    #[test]
    fn rms_half_scale_dc() {
        reset_pool();
        let mut rms = AudioAnalyzeRms::new();

        let mut block = AudioBlockMut::alloc().unwrap();
        block.fill(16384); // ~0.5 DC
        let input_ref = block.into_shared();
        let inputs = [Some(input_ref)];
        let mut outputs: [Option<AudioBlockMut>; 0] = [];

        rms.update(&inputs, &mut outputs);

        let level = rms.read();
        let expected = 16384.0 / 32767.0;
        assert!((level - expected).abs() < 0.01, "expected ~{}, got {}", expected, level);
    }

    #[test]
    fn rms_accumulates_across_blocks() {
        reset_pool();
        let mut rms = AudioAnalyzeRms::new();

        // Two blocks of the same DC value
        for _ in 0..2 {
            let mut block = AudioBlockMut::alloc().unwrap();
            block.fill(16384);
            let input_ref = block.into_shared();
            let inputs = [Some(input_ref)];
            let mut outputs: [Option<AudioBlockMut>; 0] = [];
            rms.update(&inputs, &mut outputs);
        }

        let level = rms.read();
        let expected = 16384.0 / 32767.0;
        assert!((level - expected).abs() < 0.01, "expected ~{}, got {}", expected, level);
    }

    #[test]
    fn rms_read_resets() {
        reset_pool();
        let mut rms = AudioAnalyzeRms::new();

        let mut block = AudioBlockMut::alloc().unwrap();
        block.fill(32767);
        let input_ref = block.into_shared();
        let inputs = [Some(input_ref)];
        let mut outputs: [Option<AudioBlockMut>; 0] = [];
        rms.update(&inputs, &mut outputs);

        let _ = rms.read(); // consume

        assert!(!rms.available());
        assert_eq!(rms.read(), 0.0);
    }

    #[test]
    fn rms_no_input_counts_silence() {
        reset_pool();
        let mut rms = AudioAnalyzeRms::new();

        let inputs: [Option<AudioBlockRef>; 1] = [None];
        let mut outputs: [Option<AudioBlockMut>; 0] = [];
        rms.update(&inputs, &mut outputs);

        assert!(rms.available());
        let level = rms.read();
        assert_eq!(level, 0.0);
    }
}
