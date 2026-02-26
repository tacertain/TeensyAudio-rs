//! Audio graph macro for declarative node wiring.
//!
//! The [`audio_graph!`] macro generates a typed struct containing all audio nodes
//! with an `update_all()` method that processes them in the declared order and
//! routes audio blocks between connected ports.
//!
//! # Syntax
//!
//! Nodes are listed in **processing order** (sources first, then downstream
//! consumers). Each node's input connections are declared inline using `{ ... }`.
//!
//! ```ignore
//! use teensy_audio::audio_graph;
//! use teensy_audio::nodes::*;
//!
//! audio_graph! {
//!     pub struct MyGraph {
//!         sine: AudioSynthSine {},
//!         env: AudioEffectEnvelope { (sine, 0) },
//!         mixer: AudioMixer<4> { (env, 0) },
//!         peak: AudioAnalyzePeak { (mixer, 0) },
//!     }
//! }
//! ```
//!
//! ## Input connection syntax
//!
//! - `{}` — no inputs (source node)
//! - `{ (node, port) }` — input 0 connected to `node`'s output `port`
//! - `{ (node, 0), _ }` — input 0 connected, input 1 unconnected (silence)
//! - `{ (a, 0), (b, 0) }` — two inputs from different sources
//! - `{ (mixer, 0), (mixer, 0) }` — fan-out: same output to two inputs
//!
//! ## Generated API
//!
//! - A struct with `pub` fields for each node (direct access for configuration)
//! - `new()` — constructs all nodes via their `new()` methods
//! - `update_all()` — processes one block cycle, routing audio between nodes
//!
//! ## Block routing
//!
//! - Output blocks are converted to shared `AudioBlockRef` for routing
//! - Fan-out uses `AudioBlockRef::clone()` (refcount increment, no copy)
//! - Unconnected inputs (`_`) receive `None` (silence)
//! - Pool exhaustion degrades gracefully (nodes see `None` outputs)

/// Declare and wire an audio processing graph.
///
/// See the [module documentation](crate::graph) for full syntax.
#[macro_export]
macro_rules! audio_graph {
    // ── Main entry point ──────────────────────────────────────────────
    (
        $(#[$struct_meta:meta])*
        $vis:vis struct $name:ident {
            $(
                $node_name:ident : $node_type:ty { $( $input_item:tt ),* $(,)? }
            ),+
            $(,)?
        }
    ) => {
        // ── Struct definition ─────────────────────────────────────────
        $(#[$struct_meta])*
        $vis struct $name {
            $( pub $node_name: $node_type, )+
        }

        impl $name {
            /// Create a new audio graph with all nodes default-initialized.
            pub fn new() -> Self {
                Self {
                    $( $node_name: <$node_type>::new(), )+
                }
            }

            /// Process one block cycle through the entire graph.
            ///
            /// Calls `update()` on each node in declaration order, allocating
            /// output blocks and routing them to connected input ports.
            #[allow(unused_variables)]
            pub fn update_all(&mut self) {
                $(
                    // Process node: $node_name
                    #[allow(unused_variables, clippy::let_unit_value)]
                    let $node_name: [Option<$crate::block::AudioBlockRef>;
                        <$node_type as $crate::node::AudioNode>::NUM_OUTPUTS
                    ] = {
                        // Build input array from connection specifications
                        let _inputs: [Option<$crate::block::AudioBlockRef>;
                            <$node_type as $crate::node::AudioNode>::NUM_INPUTS
                        ] = [ $( $crate::audio_graph!(@input_expr $input_item) ),* ];

                        // Allocate output blocks
                        let mut _outs: [Option<$crate::block::AudioBlockMut>;
                            <$node_type as $crate::node::AudioNode>::NUM_OUTPUTS
                        ] = core::array::from_fn(|_| $crate::block::AudioBlockMut::alloc());

                        // Call the node's update method
                        <$node_type as $crate::node::AudioNode>::update(
                            &mut self.$node_name, &_inputs, &mut _outs
                        );

                        // Convert outputs to shared refs for downstream routing
                        _outs.map(|opt| opt.map(|b| b.into_shared()))
                    };
                )+
            }
        }
    };

    // ── Input expression helpers ──────────────────────────────────────
    // Unconnected input: produces None (silence)
    (@input_expr _) => { None };

    // Connected input: clone a shared ref from a source node's output port
    (@input_expr ($src:ident, $port:expr)) => {
        $src[$port].clone()
    };
}

#[cfg(test)]
mod verification_tests;

#[cfg(test)]
mod tests {
    use crate::block::pool::POOL;

    fn reset_pool() {
        POOL.reset();
    }

    // ── Simple source → analyzer graph ────────────────────────────────
    crate::audio_graph! {
        struct SineToAnalyzer {
            sine: crate::nodes::AudioSynthSine {},
            peak: crate::nodes::AudioAnalyzePeak { (sine, 0) },
        }
    }

    #[test]
    fn graph_new_creates_all_nodes() {
        let graph = SineToAnalyzer::new();
        assert!(!graph.peak.available());
    }

    #[test]
    fn graph_update_routes_blocks() {
        reset_pool();
        let mut graph = SineToAnalyzer::new();
        graph.sine.frequency(440.0);
        graph.sine.amplitude(1.0);

        graph.update_all();

        assert!(graph.peak.available());
        let level = graph.peak.read();
        assert!(level > 0.0, "peak should detect signal, got {}", level);
    }

    // ── Multi-node chain with fan-out ─────────────────────────────────
    crate::audio_graph! {
        struct ChainGraph {
            sine: crate::nodes::AudioSynthSine {},
            amp: crate::nodes::AudioAmplifier { (sine, 0) },
            peak: crate::nodes::AudioAnalyzePeak { (amp, 0) },
            rms: crate::nodes::AudioAnalyzeRms { (amp, 0) },
        }
    }

    #[test]
    fn graph_fan_out() {
        reset_pool();
        let mut graph = ChainGraph::new();
        graph.sine.frequency(1000.0);
        graph.sine.amplitude(1.0);
        graph.amp.gain(0.5);

        graph.update_all();

        // Both analyzers should receive data from the amplifier
        assert!(graph.peak.available());
        assert!(graph.rms.available());

        let peak_level = graph.peak.read();
        let rms_level = graph.rms.read();
        assert!(peak_level > 0.0, "peak should detect signal");
        assert!(rms_level > 0.0, "rms should detect signal");
    }

    // ── Mixer graph with multiple inputs ──────────────────────────────
    crate::audio_graph! {
        struct MixerGraph {
            sine1: crate::nodes::AudioSynthSine {},
            sine2: crate::nodes::AudioSynthSine {},
            mixer: crate::nodes::AudioMixer<4> { (sine1, 0), (sine2, 0), _, _ },
            peak: crate::nodes::AudioAnalyzePeak { (mixer, 0) },
        }
    }

    #[test]
    fn graph_mixer_multiple_inputs() {
        reset_pool();
        let mut graph = MixerGraph::new();
        graph.sine1.frequency(440.0);
        graph.sine1.amplitude(0.5);
        graph.sine2.frequency(880.0);
        graph.sine2.amplitude(0.5);
        graph.mixer.gain(0, 1.0);
        graph.mixer.gain(1, 1.0);

        graph.update_all();

        assert!(graph.peak.available());
        let level = graph.peak.read();
        assert!(level > 0.0, "mixer output should have signal");
    }

    // ── Envelope chain ────────────────────────────────────────────────
    crate::audio_graph! {
        struct EnvelopeGraph {
            sine: crate::nodes::AudioSynthSine {},
            env: crate::nodes::AudioEffectEnvelope { (sine, 0) },
            peak: crate::nodes::AudioAnalyzePeak { (env, 0) },
        }
    }

    #[test]
    fn graph_envelope_modulates_signal() {
        reset_pool();
        let mut graph = EnvelopeGraph::new();
        graph.sine.frequency(440.0);
        graph.sine.amplitude(1.0);
        graph.env.attack(1.0); // very fast attack
        graph.env.sustain(1.0);

        // Before note_on: envelope is idle, should produce no output
        graph.update_all();
        let level_idle = if graph.peak.available() { graph.peak.read() } else { 0.0 };

        // Trigger note and process
        graph.env.note_on();
        graph.update_all();
        assert!(graph.peak.available());
        let level_active = graph.peak.read();

        assert!(
            level_active > level_idle,
            "active level ({}) should exceed idle level ({})",
            level_active, level_idle
        );
    }

    // ── DC source test ────────────────────────────────────────────────
    crate::audio_graph! {
        struct DcGraph {
            dc: crate::nodes::AudioSynthWaveformDc {},
            peak: crate::nodes::AudioAnalyzePeak { (dc, 0) },
        }
    }

    #[test]
    fn graph_dc_source() {
        reset_pool();
        let mut graph = DcGraph::new();
        graph.dc.amplitude(0.5);

        graph.update_all();

        assert!(graph.peak.available());
        let level = graph.peak.read();
        assert!(
            (level - 0.5).abs() < 0.02,
            "DC 0.5 should produce ~0.5 peak, got {}",
            level
        );
    }

    // ── Silent graph (no amplitude) ───────────────────────────────────
    #[test]
    fn graph_silent_source() {
        reset_pool();
        let mut graph = SineToAnalyzer::new();
        // Don't set amplitude (default is 0)

        graph.update_all();

        // Sine with zero amplitude returns early without taking the output block.
        // The preallocated zeroed block reaches the peak analyzer as silence.
        assert!(graph.peak.available());
        let level = graph.peak.read();
        assert!(
            level == 0.0,
            "silent source should produce zero peak, got {}",
            level
        );
    }

    // ── Multiple update cycles ────────────────────────────────────────
    #[test]
    fn graph_multiple_updates() {
        reset_pool();
        let mut graph = SineToAnalyzer::new();
        graph.sine.frequency(440.0);
        graph.sine.amplitude(1.0);

        for _ in 0..10 {
            graph.update_all();
        }

        assert!(graph.peak.available());
        let level = graph.peak.read();
        assert!(level > 0.0);
    }
}
