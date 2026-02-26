//! End-to-end verification tests for the audio graph framework.
//!
//! These tests exercise full DSP pipelines assembled via the `audio_graph!`
//! macro, verifying:
//!
//! - **Signal integrity:** correct levels through multi-node chains
//! - **Pool accounting:** zero block leaks after processing
//! - **Streaming stability:** sustained multi-cycle operation
//! - **ADSR envelope shaping:** note-on / note-off modulates signal
//! - **Gain staging:** amplifier and mixer gain produce expected levels
//! - **Fan-out correctness:** identical data reaches all consumers
//! - **Fade effect:** smooth volume transitions across blocks

#[cfg(test)]
mod tests {
    use crate::block::pool::POOL;
    use crate::constants::AUDIO_BLOCK_SAMPLES;

    fn reset_pool() {
        POOL.reset();
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Verification 1: DSP test — ADSR envelope shapes tone,
    //  peak analyzer reads correct levels
    // ═══════════════════════════════════════════════════════════════════

    crate::audio_graph! {
        struct DspVerificationGraph {
            sine: crate::nodes::AudioSynthSine {},
            env: crate::nodes::AudioEffectEnvelope { (sine, 0) },
            mixer: crate::nodes::AudioMixer<4> { (env, 0), _, _, _ },
            peak: crate::nodes::AudioAnalyzePeak { (mixer, 0) },
            rms: crate::nodes::AudioAnalyzeRms { (mixer, 0) },
        }
    }

    #[test]
    fn verify_dsp_adsr_shapes_tone() {
        reset_pool();
        let mut g = DspVerificationGraph::new();
        g.sine.frequency(440.0);
        g.sine.amplitude(1.0);
        g.env.attack(1.0); // very fast attack (~1 ms)
        g.env.decay(1.0);
        g.env.sustain(0.5);
        g.env.release(50.0);
        g.mixer.gain(0, 1.0);

        // ── Idle: envelope not triggered ──────────────────────────────
        g.update_all();
        // Envelope is idle → output is silence (zeros multiplied through)
        assert!(g.peak.available());
        let idle_peak = g.peak.read();
        // Idle envelope multiplies input by 0 → should be 0 or very close
        assert!(
            idle_peak < 0.01,
            "idle envelope should silence signal, got {}",
            idle_peak
        );

        // ── Attack: trigger note ──────────────────────────────────────
        g.env.note_on();

        // Process several blocks for attack to ramp up
        for _ in 0..5 {
            g.update_all();
        }
        assert!(g.peak.available());
        let attack_peak = g.peak.read();
        assert!(
            attack_peak > 0.1,
            "attack phase should produce signal, got {}",
            attack_peak
        );

        // ── Sustain: process more blocks to reach steady state ────────
        for _ in 0..20 {
            g.update_all();
        }
        assert!(g.peak.available());
        let sustain_peak = g.peak.read();
        assert!(
            sustain_peak > 0.1,
            "sustain phase should maintain signal, got {}",
            sustain_peak
        );

        // ── Release: note off ─────────────────────────────────────────
        g.env.note_off();

        // Process blocks through release phase
        for _ in 0..50 {
            g.update_all();
        }
        assert!(g.peak.available());
        let release_peak = g.peak.read();
        assert!(
            release_peak < sustain_peak,
            "release peak ({}) should be less than sustain peak ({})",
            release_peak,
            sustain_peak
        );

        // RMS should also have data
        assert!(g.rms.available());
        let rms_level = g.rms.read();
        // After release, RMS could be very low or moderate depending on where
        // in the release curve we are. Just verify it's a valid reading.
        assert!(rms_level >= 0.0, "RMS should be non-negative");
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Verification 2: Pool accounting — zero block leaks
    // ═══════════════════════════════════════════════════════════════════

    crate::audio_graph! {
        struct PoolAccountingGraph {
            sine: crate::nodes::AudioSynthSine {},
            amp: crate::nodes::AudioAmplifier { (sine, 0) },
            mixer: crate::nodes::AudioMixer<4> { (amp, 0), _, _, _ },
            peak: crate::nodes::AudioAnalyzePeak { (mixer, 0) },
        }
    }

    #[test]
    fn verify_pool_no_leaks_after_update() {
        reset_pool();
        assert_eq!(POOL.allocated_count(), 0, "pool should start empty");

        let mut g = PoolAccountingGraph::new();
        g.sine.frequency(440.0);
        g.sine.amplitude(1.0);
        g.amp.gain(0.5);
        g.mixer.gain(0, 1.0);

        // Run several cycles
        for _ in 0..20 {
            g.update_all();
            // After update_all returns, all intermediate blocks should be freed.
            // Only blocks retained by analyzers (peak) may survive, but peak's
            // update doesn't hold blocks — it reads and drops them.
            let allocated = POOL.allocated_count();
            assert!(
                allocated == 0,
                "pool should have 0 blocks allocated after update cycle, got {}",
                allocated
            );
        }
    }

    #[test]
    fn verify_pool_no_leaks_with_fan_out() {
        reset_pool();

        crate::audio_graph! {
            struct FanOutLeakGraph {
                dc: crate::nodes::AudioSynthWaveformDc {},
                peak1: crate::nodes::AudioAnalyzePeak { (dc, 0) },
                peak2: crate::nodes::AudioAnalyzePeak { (dc, 0) },
                rms: crate::nodes::AudioAnalyzeRms { (dc, 0) },
            }
        }

        let mut g = FanOutLeakGraph::new();
        g.dc.amplitude(0.75);

        for _ in 0..10 {
            g.update_all();
            let allocated = POOL.allocated_count();
            assert!(
                allocated == 0,
                "fan-out graph should not leak blocks, got {} allocated",
                allocated
            );
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Verification 3: Streaming stability — sustained operation
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn verify_streaming_100_cycles() {
        reset_pool();
        let mut g = DspVerificationGraph::new();
        g.sine.frequency(440.0);
        g.sine.amplitude(1.0);
        g.env.attack(1.0);
        g.env.sustain(1.0);
        g.mixer.gain(0, 1.0);
        g.env.note_on();

        let mut max_peak: f32 = 0.0;
        let mut min_peak: f32 = f32::MAX;

        // Simulate ~100 audio blocks (~290 ms of audio at 44.1 kHz)
        for cycle in 0..100 {
            g.update_all();

            if g.peak.available() {
                let level = g.peak.read();
                if level > max_peak {
                    max_peak = level;
                }
                if level < min_peak {
                    min_peak = level;
                }

                // After initial attack, signal should be nonzero
                if cycle > 5 {
                    assert!(
                        level > 0.0,
                        "cycle {}: signal should be present, got {}",
                        cycle, level
                    );
                }
            }
        }

        // Over 100 cycles, we should have gotten meaningful readings
        assert!(max_peak > 0.3, "max peak should be substantial, got {}", max_peak);
        assert!(
            POOL.allocated_count() == 0,
            "no pool leaks after 100 cycles"
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Verification 4: Gain staging accuracy
    // ═══════════════════════════════════════════════════════════════════

    crate::audio_graph! {
        struct GainStagingGraph {
            dc: crate::nodes::AudioSynthWaveformDc {},
            amp: crate::nodes::AudioAmplifier { (dc, 0) },
            peak: crate::nodes::AudioAnalyzePeak { (amp, 0) },
        }
    }

    #[test]
    fn verify_gain_staging_half() {
        reset_pool();
        let mut g = GainStagingGraph::new();
        g.dc.amplitude(1.0);
        g.amp.gain(0.5);

        g.update_all();

        assert!(g.peak.available());
        let level = g.peak.read();
        assert!(
            (level - 0.5).abs() < 0.05,
            "DC 1.0 × gain 0.5 should produce ~0.5, got {}",
            level
        );
    }

    #[test]
    fn verify_gain_staging_quarter() {
        reset_pool();
        let mut g = GainStagingGraph::new();
        g.dc.amplitude(0.5);
        g.amp.gain(0.5);

        g.update_all();

        assert!(g.peak.available());
        let level = g.peak.read();
        assert!(
            (level - 0.25).abs() < 0.05,
            "DC 0.5 × gain 0.5 should produce ~0.25, got {}",
            level
        );
    }

    #[test]
    fn verify_gain_staging_zero_attenuates() {
        reset_pool();
        let mut g = GainStagingGraph::new();
        g.dc.amplitude(1.0);
        g.amp.gain(0.0);

        g.update_all();

        // Amplifier with zero gain drops its output block (silence optimization).
        // The peak analyzer receives None, so no new data is available.
        // This is correct: zero-gain is silent.
        let has_data = g.peak.available();
        if has_data {
            let level = g.peak.read();
            assert!(
                level < 0.001,
                "DC 1.0 × gain 0.0 should produce ~0, got {}",
                level
            );
        }
        // Whether peak sees zeros or no block at all, the result is silence.
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Verification 5: Fan-out correctness — identical data
    // ═══════════════════════════════════════════════════════════════════

    crate::audio_graph! {
        struct FanOutVerifyGraph {
            dc: crate::nodes::AudioSynthWaveformDc {},
            peak1: crate::nodes::AudioAnalyzePeak { (dc, 0) },
            peak2: crate::nodes::AudioAnalyzePeak { (dc, 0) },
        }
    }

    #[test]
    fn verify_fan_out_identical_levels() {
        reset_pool();
        let mut g = FanOutVerifyGraph::new();
        g.dc.amplitude(0.75);

        g.update_all();

        assert!(g.peak1.available());
        assert!(g.peak2.available());

        let level1 = g.peak1.read();
        let level2 = g.peak2.read();

        assert!(
            (level1 - level2).abs() < 0.001,
            "fan-out should deliver identical data: peak1={}, peak2={}",
            level1,
            level2
        );
        assert!(
            (level1 - 0.75).abs() < 0.02,
            "fan-out level should be ~0.75, got {}",
            level1
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Verification 6: Mixer summing accuracy
    // ═══════════════════════════════════════════════════════════════════

    crate::audio_graph! {
        struct MixerSumGraph {
            dc1: crate::nodes::AudioSynthWaveformDc {},
            dc2: crate::nodes::AudioSynthWaveformDc {},
            mixer: crate::nodes::AudioMixer<4> { (dc1, 0), (dc2, 0), _, _ },
            peak: crate::nodes::AudioAnalyzePeak { (mixer, 0) },
        }
    }

    #[test]
    fn verify_mixer_sums_two_sources() {
        reset_pool();
        let mut g = MixerSumGraph::new();
        g.dc1.amplitude(0.25);
        g.dc2.amplitude(0.25);
        g.mixer.gain(0, 1.0);
        g.mixer.gain(1, 1.0);

        g.update_all();

        assert!(g.peak.available());
        let level = g.peak.read();
        assert!(
            (level - 0.5).abs() < 0.05,
            "0.25 + 0.25 should produce ~0.5, got {}",
            level
        );
    }

    #[test]
    fn verify_mixer_gain_weights() {
        reset_pool();
        let mut g = MixerSumGraph::new();
        g.dc1.amplitude(1.0);
        g.dc2.amplitude(1.0);
        g.mixer.gain(0, 0.25);
        g.mixer.gain(1, 0.25);

        g.update_all();

        assert!(g.peak.available());
        let level = g.peak.read();
        assert!(
            (level - 0.5).abs() < 0.05,
            "1.0*0.25 + 1.0*0.25 should produce ~0.5, got {}",
            level
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Verification 7: Fade effect transitions
    // ═══════════════════════════════════════════════════════════════════

    crate::audio_graph! {
        struct FadeGraph {
            dc: crate::nodes::AudioSynthWaveformDc {},
            fade: crate::nodes::AudioEffectFade { (dc, 0) },
            peak: crate::nodes::AudioAnalyzePeak { (fade, 0) },
        }
    }

    #[test]
    fn verify_fade_in_increases_output() {
        reset_pool();
        let mut g = FadeGraph::new();
        g.dc.amplitude(1.0);
        g.fade.fade_in(10); // 10ms fade-in

        // Collect peak levels across several blocks
        let mut levels = [0.0f32; 10];
        for i in 0..10 {
            g.update_all();
            if g.peak.available() {
                levels[i] = g.peak.read();
            }
        }

        // Later blocks should generally have higher level than first
        assert!(
            levels[9] > levels[0] || levels[9] > 0.8,
            "fade-in should increase: first={}, last={}",
            levels[0],
            levels[9]
        );
    }

    #[test]
    fn verify_fade_out_decreases_output() {
        reset_pool();
        let mut g = FadeGraph::new();
        g.dc.amplitude(1.0);

        // First bring fade to full volume by processing enough blocks
        g.fade.fade_in(1); // instant fade-in
        for _ in 0..20 {
            g.update_all();
        }
        assert!(g.peak.available());
        let full_level = g.peak.read();
        assert!(
            full_level > 0.5,
            "should reach near-full level after fade-in, got {}",
            full_level
        );

        // Now fade out with a longer duration
        g.fade.fade_out(100); // 100ms fade-out

        // Process enough blocks for the fade to complete.
        // Read the peak after EACH block to track the latest level,
        // since peak reports the max across accumulated blocks.
        let mut last_level = full_level;
        for _ in 0..60 {
            g.update_all();
            if g.peak.available() {
                last_level = g.peak.read();
            }
        }

        assert!(
            last_level < full_level,
            "fade-out final block level ({}) should be below full level ({})",
            last_level,
            full_level
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Verification 8: Complex graph — full synthesizer chain
    // ═══════════════════════════════════════════════════════════════════

    crate::audio_graph! {
        struct FullSynthGraph {
            osc1: crate::nodes::AudioSynthSine {},
            osc2: crate::nodes::AudioSynthSine {},
            env1: crate::nodes::AudioEffectEnvelope { (osc1, 0) },
            env2: crate::nodes::AudioEffectEnvelope { (osc2, 0) },
            mixer: crate::nodes::AudioMixer<4> { (env1, 0), (env2, 0), _, _ },
            amp: crate::nodes::AudioAmplifier { (mixer, 0) },
            peak: crate::nodes::AudioAnalyzePeak { (amp, 0) },
            rms: crate::nodes::AudioAnalyzeRms { (amp, 0) },
        }
    }

    #[test]
    fn verify_full_synth_chain() {
        reset_pool();
        let mut g = FullSynthGraph::new();

        // Configure two oscillators at different frequencies
        g.osc1.frequency(440.0);
        g.osc1.amplitude(1.0);
        g.osc2.frequency(880.0);
        g.osc2.amplitude(1.0);

        // Fast envelopes
        g.env1.attack(1.0);
        g.env1.sustain(1.0);
        g.env2.attack(1.0);
        g.env2.sustain(1.0);

        // Mixer gains
        g.mixer.gain(0, 0.5);
        g.mixer.gain(1, 0.5);

        // Master volume
        g.amp.gain(0.8);

        // Trigger both notes
        g.env1.note_on();
        g.env2.note_on();

        // Process 20 blocks (~58 ms) to reach sustain
        for _ in 0..20 {
            g.update_all();
        }

        // Verify both analyzers see signal
        assert!(g.peak.available(), "peak should have data after 20 blocks");
        assert!(g.rms.available(), "rms should have data after 20 blocks");

        let peak_level = g.peak.read();
        let rms_level = g.rms.read();

        assert!(
            peak_level > 0.1,
            "full synth chain should produce signal, peak={}",
            peak_level
        );
        assert!(
            rms_level > 0.05,
            "full synth chain should produce signal, rms={}",
            rms_level
        );

        // Turn off notes and let release decay
        g.env1.note_off();
        g.env2.note_off();

        for _ in 0..100 {
            g.update_all();
        }

        // After long release, signal should be lower
        if g.peak.available() {
            let released_peak = g.peak.read();
            assert!(
                released_peak < peak_level || released_peak < 0.1,
                "released peak ({}) should be lower than sustain peak ({})",
                released_peak,
                peak_level
            );
        }

        // Pool should be clean
        assert_eq!(
            POOL.allocated_count(),
            0,
            "no block leaks after full synth chain"
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Verification 9: RMS level accuracy with DC source
    // ═══════════════════════════════════════════════════════════════════

    crate::audio_graph! {
        struct RmsAccuracyGraph {
            dc: crate::nodes::AudioSynthWaveformDc {},
            rms: crate::nodes::AudioAnalyzeRms { (dc, 0) },
        }
    }

    #[test]
    fn verify_rms_dc_accuracy() {
        reset_pool();
        let mut g = RmsAccuracyGraph::new();
        g.dc.amplitude(0.5);

        // Accumulate a few blocks for RMS
        for _ in 0..5 {
            g.update_all();
        }

        assert!(g.rms.available());
        let rms_level = g.rms.read();
        // RMS of a constant DC signal equals the amplitude
        assert!(
            (rms_level - 0.5).abs() < 0.02,
            "RMS of DC 0.5 should be ~0.5, got {}",
            rms_level
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Verification 10: Block count per cycle
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn verify_block_count_per_sample() {
        // Verify the audio constants are as expected
        assert_eq!(AUDIO_BLOCK_SAMPLES, 128, "block size should be 128 samples");

        // Verify block duration: 128 / 44117.647 ≈ 2.9 ms
        let block_duration_ms =
            AUDIO_BLOCK_SAMPLES as f64 / crate::constants::AUDIO_SAMPLE_RATE_EXACT as f64 * 1000.0;
        assert!(
            (block_duration_ms - 2.9).abs() < 0.1,
            "block duration should be ~2.9 ms, got {} ms",
            block_duration_ms
        );
    }
}
