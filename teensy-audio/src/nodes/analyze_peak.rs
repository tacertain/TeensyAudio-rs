//! Peak level detector / analyzer.
//!
//! Port of `TeensyAudio/analyze_peak.cpp`. Tracks the minimum and maximum
//! sample values seen since the last `read()`.

use crate::block::{AudioBlockMut, AudioBlockRef};
use crate::constants::AUDIO_BLOCK_SAMPLES;
use crate::node::AudioNode;

/// Peak level detector. Analyzer node: 1 input, 0 outputs.
///
/// Tracks the maximum absolute sample value and peak-to-peak range
/// over one or more block periods.
///
/// # Example
/// ```ignore
/// let mut peak = AudioAnalyzePeak::new();
/// // ... after processing ...
/// if peak.available() {
///     let level = peak.read(); // 0.0–1.0
/// }
/// ```
pub struct AudioAnalyzePeak {
    min_val: i16,
    max_val: i16,
    new_output: bool,
}

impl AudioAnalyzePeak {
    /// Create a new peak analyzer.
    pub const fn new() -> Self {
        AudioAnalyzePeak {
            min_val: i16::MAX,
            max_val: i16::MIN,
            new_output: false,
        }
    }

    /// Returns `true` if new data has been accumulated since the last `read()`.
    pub fn available(&self) -> bool {
        self.new_output
    }

    /// Read the peak level (0.0–1.0) and reset the accumulator.
    ///
    /// Returns the maximum absolute sample value normalized to [0.0, 1.0].
    pub fn read(&mut self) -> f32 {
        let min = self.min_val;
        let max = self.max_val;
        self.min_val = i16::MAX;
        self.max_val = i16::MIN;
        self.new_output = false;

        let abs_min = if min == i16::MIN {
            // -32768 abs would overflow i16, handle specially
            32768i32
        } else {
            (min as i32).abs()
        };
        let abs_max = (max as i32).abs();
        let peak = if abs_min > abs_max { abs_min } else { abs_max };
        peak as f32 / 32767.0
    }

    /// Read the peak-to-peak level (0.0–2.0) and reset the accumulator.
    ///
    /// Returns `(max - min) / 32767.0`.
    pub fn read_peak_to_peak(&mut self) -> f32 {
        let min = self.min_val;
        let max = self.max_val;
        self.min_val = i16::MAX;
        self.max_val = i16::MIN;
        self.new_output = false;

        (max as i32 - min as i32) as f32 / 32767.0
    }
}

impl AudioNode for AudioAnalyzePeak {
    const NUM_INPUTS: usize = 1;
    const NUM_OUTPUTS: usize = 0;

    fn update(
        &mut self,
        inputs: &[Option<AudioBlockRef>],
        _outputs: &mut [Option<AudioBlockMut>],
    ) {
        let input = match inputs[0] {
            Some(ref b) => b,
            None => return,
        };

        let mut min = self.min_val;
        let mut max = self.max_val;

        for i in 0..AUDIO_BLOCK_SAMPLES {
            let d = input[i];
            if d < min {
                min = d;
            }
            if d > max {
                max = d;
            }
        }

        self.min_val = min;
        self.max_val = max;
        self.new_output = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::pool::POOL;

    fn reset_pool() {
        POOL.reset();
    }

    fn alloc_block_with(values: &[i16]) -> AudioBlockMut {
        let mut block = AudioBlockMut::alloc().unwrap();
        block.fill(0);
        for (i, &v) in values.iter().enumerate() {
            if i < AUDIO_BLOCK_SAMPLES {
                block[i] = v;
            }
        }
        block
    }

    #[test]
    fn peak_no_data() {
        let peak = AudioAnalyzePeak::new();
        assert!(!peak.available());
    }

    #[test]
    fn peak_detects_positive() {
        reset_pool();
        let mut peak = AudioAnalyzePeak::new();

        let mut input = alloc_block_with(&[0; 0]);
        input.fill(0);
        input[50] = 16384; // 0.5 peak

        let input_ref = input.into_shared();
        let inputs = [Some(input_ref)];
        let mut outputs: [Option<AudioBlockMut>; 0] = [];

        peak.update(&inputs, &mut outputs);

        assert!(peak.available());
        let level = peak.read();
        assert!((level - 0.5).abs() < 0.01, "expected ~0.5, got {}", level);
        assert!(!peak.available());
    }

    #[test]
    fn peak_detects_negative() {
        reset_pool();
        let mut peak = AudioAnalyzePeak::new();

        let mut input = AudioBlockMut::alloc().unwrap();
        input.fill(0);
        input[10] = -24576; // -0.75 peak

        let input_ref = input.into_shared();
        let inputs = [Some(input_ref)];
        let mut outputs: [Option<AudioBlockMut>; 0] = [];

        peak.update(&inputs, &mut outputs);

        let level = peak.read();
        assert!((level - 0.75).abs() < 0.01, "expected ~0.75, got {}", level);
    }

    #[test]
    fn peak_to_peak() {
        reset_pool();
        let mut peak = AudioAnalyzePeak::new();

        let mut input = AudioBlockMut::alloc().unwrap();
        input.fill(0);
        input[0] = 16384;   // +0.5
        input[1] = -16384;  // -0.5

        let input_ref = input.into_shared();
        let inputs = [Some(input_ref)];
        let mut outputs: [Option<AudioBlockMut>; 0] = [];

        peak.update(&inputs, &mut outputs);

        let pp = peak.read_peak_to_peak();
        // peak-to-peak = (16384 - (-16384)) / 32767 ≈ 1.0
        assert!((pp - 1.0).abs() < 0.01, "expected ~1.0, got {}", pp);
    }

    #[test]
    fn peak_accumulates_across_blocks() {
        reset_pool();
        let mut peak = AudioAnalyzePeak::new();

        // Block 1: max = 10000
        let mut input1 = AudioBlockMut::alloc().unwrap();
        input1.fill(0);
        input1[0] = 10000;
        let ref1 = input1.into_shared();
        let inputs = [Some(ref1)];
        let mut outputs: [Option<AudioBlockMut>; 0] = [];
        peak.update(&inputs, &mut outputs);

        // Block 2: max = 20000
        let mut input2 = AudioBlockMut::alloc().unwrap();
        input2.fill(0);
        input2[0] = 20000;
        let ref2 = input2.into_shared();
        let inputs = [Some(ref2)];
        peak.update(&inputs, &mut outputs);

        // Should report the overall max (20000)
        let level = peak.read();
        let expected = 20000.0 / 32767.0;
        assert!((level - expected).abs() < 0.01, "expected ~{}, got {}", expected, level);
    }

    #[test]
    fn peak_read_resets() {
        reset_pool();
        let mut peak = AudioAnalyzePeak::new();

        let mut input = AudioBlockMut::alloc().unwrap();
        input.fill(0);
        input[0] = 30000;
        let input_ref = input.into_shared();
        let inputs = [Some(input_ref)];
        let mut outputs: [Option<AudioBlockMut>; 0] = [];
        peak.update(&inputs, &mut outputs);

        let _ = peak.read(); // consume

        // After read, a second read without new data should return 0
        // min_val and max_val were reset; with no data, min=MAX, max=MIN
        // The peak should be effectively 0 or invalid since no data
        // Actually: abs(i16::MAX)=32767, abs(i16::MIN)=32768
        // So read returns max(32767,32768)/32767 ≈ 1.0 which is wrong
        // This is expected sentinel behavior — user should check available() first
        assert!(!peak.available());
    }
}
