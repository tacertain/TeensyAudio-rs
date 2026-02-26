use crate::block::{AudioBlockMut, AudioBlockRef};

/// Core trait for all audio processing nodes.
///
/// Each node receives input blocks and produces output blocks during `update()`.
/// The number of inputs and outputs is declared via associated constants.
pub trait AudioNode {
    /// Number of input channels this node accepts.
    const NUM_INPUTS: usize;

    /// Number of output channels this node produces.
    const NUM_OUTPUTS: usize;

    /// Process one block of audio.
    ///
    /// `inputs` contains `NUM_INPUTS` slots, each optionally holding a shared audio block.
    /// `outputs` contains `NUM_OUTPUTS` slots, each optionally holding an exclusive audio block
    /// allocated by the caller.
    fn update(
        &mut self,
        inputs: &[Option<AudioBlockRef>],
        outputs: &mut [Option<AudioBlockMut>],
    );
}
