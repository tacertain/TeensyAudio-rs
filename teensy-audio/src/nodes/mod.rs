//! DSP audio processing nodes.
//!
//! This module contains the initial set of audio nodes: mixers, synthesizers,
//! effects, and analyzers. Each implements the [`AudioNode`](crate::node::AudioNode) trait.

mod amplifier;
mod analyze_peak;
mod analyze_rms;
mod effect_envelope;
mod effect_fade;
mod mixer;
mod synth_dc;
mod synth_sine;

pub use amplifier::AudioAmplifier;
pub use analyze_peak::AudioAnalyzePeak;
pub use analyze_rms::AudioAnalyzeRms;
pub use effect_envelope::{AudioEffectEnvelope, EnvelopeState};
pub use effect_fade::AudioEffectFade;
pub use mixer::AudioMixer;
pub use synth_dc::AudioSynthWaveformDc;
pub use synth_sine::AudioSynthSine;
