//! DSP audio processing nodes.
//!
//! This module contains the initial set of audio nodes: mixers, synthesizers,
//! effects, and analyzers. Each implements the [`AudioNode`](crate::node::AudioNode) trait.

mod mixer;
mod amplifier;
mod synth_sine;
mod synth_dc;
mod effect_fade;
mod effect_envelope;
mod analyze_peak;
mod analyze_rms;

pub use mixer::AudioMixer;
pub use amplifier::AudioAmplifier;
pub use synth_sine::AudioSynthSine;
pub use synth_dc::AudioSynthWaveformDc;
pub use effect_fade::AudioEffectFade;
pub use effect_envelope::{AudioEffectEnvelope, EnvelopeState};
pub use analyze_peak::AudioAnalyzePeak;
pub use analyze_rms::AudioAnalyzeRms;
