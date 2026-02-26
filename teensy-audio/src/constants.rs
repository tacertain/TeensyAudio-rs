/// Number of 16-bit samples per audio block.
pub const AUDIO_BLOCK_SAMPLES: usize = 128;

/// Number of audio blocks in the global pool.
pub const POOL_SIZE: usize = 32;

/// Exact audio sample rate in Hz (matches Teensy hardware PLL configuration).
pub const AUDIO_SAMPLE_RATE_EXACT: f32 = 44_117.647;
