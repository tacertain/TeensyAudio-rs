//! Graph-based synthesizer — multi-node audio pipeline with tremolo.
//!
//! Demonstrates wiring multiple audio nodes together by hand:
//!
//! ```text
//!   Sine oscillator (220 Hz)
//!         │
//!   Amplifier (gain cycles 0→1→0 for tremolo)
//!         │
//!      Mixer (1 active channel)
//!         │
//!   Output I2S (L + R)
//! ```
//!
//! A simple software envelope ramps the amplifier gain up and down over
//! ~1 second each direction, producing a continuous tremolo effect.
//!
//! Hardware: Teensy 4.1 + Audio Shield Rev D (SGTL5000)
//!
//! ---
//!
//! **Note on the `audio_graph!` macro:** For pure-DSP chains that terminate
//! in an analyzer node (e.g. `AudioAnalyzePeak`), the declarative
//! `audio_graph!` macro with `update_all()` is ideal. For chains that
//! terminate in an I/O node like `AudioOutputI2S` (which requires
//! `new(bool)` and ISR integration), manual wiring is cleaner.

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use teensy4_panic as _;

/// Spin-loop delay — implements `embedded_hal::delay::DelayNs`.
struct AsmDelay;

impl embedded_hal::delay::DelayNs for AsmDelay {
    fn delay_ns(&mut self, ns: u32) {
        let cycles = (ns as u64 * 6 + 9) / 10;
        cortex_m::asm::delay(cycles as u32);
    }
}

#[rtic::app(device = teensy4_bsp, peripherals = true, dispatchers = [KPP])]
mod app {
    use super::AsmDelay;
    use bsp::board;
    use bsp::hal;
    use bsp::ral;
    use teensy4_bsp as bsp;

    use hal::dma::channel::{self, Channel, Configuration};
    use hal::dma::peripheral::Destination;

    use teensy_audio::block::{AudioBlockMut, AudioBlockRef};
    use teensy_audio::codec::Sgtl5000;
    use teensy_audio::io::output_i2s::{AudioOutputI2S, DmaHalf};
    use teensy_audio::node::AudioNode;
    use teensy_audio::nodes::{AudioAmplifier, AudioMixer, AudioSynthSine};

    const AUDIO_BLOCK_SAMPLES: usize = 128;
    const DMA_BUF_LEN: usize = AUDIO_BLOCK_SAMPLES;

    type SaiTx = hal::sai::Tx<1, 32, 2, hal::sai::PackingNone>;

    // ── Synth state ──────────────────────────────────────────────────

    struct Synth {
        sine: AudioSynthSine,
        amp: AudioAmplifier,
        mixer: AudioMixer<4>,
    }

    impl Synth {
        fn new() -> Self {
            let mut sine = AudioSynthSine::new();
            sine.frequency(220.0);
            sine.amplitude(1.0);

            let amp = AudioAmplifier::new();
            // Gain is set dynamically in the ISR (tremolo).

            let mut mixer = AudioMixer::<4>::new();
            mixer.gain(0, 1.0);

            Self { sine, amp, mixer }
        }

        /// Process one block cycle through the pipeline.
        ///
        /// Returns the mono mix block for routing to the output I2S.
        fn process(&mut self) -> Option<AudioBlockRef> {
            // 1. Generate sine.
            let mut sine_out: [Option<AudioBlockMut>; 1] =
                [AudioBlockMut::alloc()];
            self.sine.update(&[], &mut sine_out);
            let sine_ref: Option<AudioBlockRef> =
                sine_out[0].take().map(|b| b.into_shared());

            // 2. Amplifier.
            let mut amp_out: [Option<AudioBlockMut>; 1] =
                [AudioBlockMut::alloc()];
            self.amp.update(&[sine_ref], &mut amp_out);
            let amp_ref: Option<AudioBlockRef> =
                amp_out[0].take().map(|b| b.into_shared());

            // 3. Mixer (channel 0 only; 1–3 are silent).
            let mut mixer_out: [Option<AudioBlockMut>; 1] =
                [AudioBlockMut::alloc()];
            self.mixer
                .update(&[amp_ref, None, None, None], &mut mixer_out);

            mixer_out[0].take().map(|b| b.into_shared())
        }
    }

    // ── RTIC resources ───────────────────────────────────────────────

    #[local]
    struct Local {
        led: board::Led,
        dma_chan: Channel,
        sai_tx: SaiTx,
        output: AudioOutputI2S,
        synth: Synth,
        gain: f32,
        gain_dir: f32,
    }

    #[shared]
    struct Shared {}

    #[link_section = ".uninit.dma_tx"]
    static mut DMA_TX_BUF: core::mem::MaybeUninit<[u32; DMA_BUF_LEN]> =
        core::mem::MaybeUninit::uninit();

    // ── Init ─────────────────────────────────────────────────────────

    #[init]
    fn init(cx: init::Context) -> (Shared, Local) {
        let board::Resources {
            mut gpio2,
            pins,
            mut dma,
            sai1,
            lpi2c1,
            ..
        } = board::t41(cx.device);

        let led = board::led(&mut gpio2, pins.p13);

        // ── MCLK direction: output ──────────────────────────────────
        unsafe {
            let gpr = ral::iomuxc_gpr::IOMUXC_GPR::instance();
            ral::modify_reg!(ral::iomuxc_gpr, gpr, GPR1, SAI1_MCLK_DIR: 1);
        }

        // ── Configure SAI1 ──────────────────────────────────────────
        let sai = hal::sai::Sai::new(
            sai1,
            pins.p23,
            hal::sai::Pins {
                sync: pins.p27,
                bclk: pins.p26,
                data: pins.p7,
            },
            hal::sai::Pins {
                sync: pins.p20,
                bclk: pins.p21,
                data: pins.p8,
            },
        );

        let sai_config = {
            let mut c = hal::sai::SaiConfig::i2s(hal::sai::bclk_div(4));
            c.sync_mode = hal::sai::SyncMode::TxFollowRx;
            c.mclk_source = hal::sai::MclkSource::Select1;
            c
        };
        let (Some(mut sai_tx), Some(sai_rx)) =
            sai.split::<32, 2, hal::sai::PackingNone>(&sai_config)
        else {
            panic!("SAI split failed");
        };
        drop(sai_rx);

        // ── I2C + SGTL5000 codec ────────────────────────────────────
        let i2c = board::lpi2c(
            lpi2c1,
            pins.p19,
            pins.p18,
            board::Lpi2cClockSpeed::KHz400,
        );
        let mut codec = Sgtl5000::new(i2c, AsmDelay);
        codec.enable().expect("SGTL5000 enable");
        codec.volume(0.5).expect("SGTL5000 volume");

        // ── Synth + output ──────────────────────────────────────────
        let synth = Synth::new();
        let output = AudioOutputI2S::new(true);

        // ── DMA channel 0 → SAI1 TX ────────────────────────────────
        let mut dma_chan = dma[0].take().expect("DMA ch0");
        dma_chan.disable();
        dma_chan.set_disable_on_completion(true);
        dma_chan.set_interrupt_on_completion(true);
        dma_chan.set_channel_configuration(Configuration::enable(
            sai_tx.destination_signal(),
        ));
        unsafe {
            let buf = core::slice::from_raw_parts(
                core::ptr::addr_of!(DMA_TX_BUF) as *const u32,
                DMA_BUF_LEN,
            );
            channel::set_source_linear_buffer(&mut dma_chan, buf);
            channel::set_destination_hardware(
                &mut dma_chan,
                sai_tx.destination_address(),
            );
            dma_chan.set_minor_loop_bytes(core::mem::size_of::<u32>() as u32);
            dma_chan.set_transfer_iterations(DMA_BUF_LEN as u16);
        }

        sai_tx.enable_dma_transmit();
        unsafe { dma_chan.enable() };
        sai_tx.set_enable(true);

        (
            Shared {},
            Local {
                led,
                dma_chan,
                sai_tx,
                output,
                synth,
                gain: 0.0,
                gain_dir: 1.0,
            },
        )
    }

    // ── DMA ISR ──────────────────────────────────────────────────────

    #[task(binds = DMA0_DMA16, local = [led, dma_chan, sai_tx, output, synth, gain, gain_dir, toggle: u32 = 0], priority = 2)]
    fn dma_isr(cx: dma_isr::Context) {
        let dma_chan = cx.local.dma_chan;
        let output = cx.local.output;
        let synth = cx.local.synth;
        let led = cx.local.led;
        let toggle = cx.local.toggle;
        let gain = cx.local.gain;
        let gain_dir = cx.local.gain_dir;

        while dma_chan.is_interrupt() {
            dma_chan.clear_interrupt();
        }
        dma_chan.clear_complete();

        let half = if *toggle % 2 == 0 {
            DmaHalf::First
        } else {
            DmaHalf::Second
        };

        let dma_buf = unsafe { &mut *DMA_TX_BUF.as_mut_ptr() };
        let should_update = output.isr(dma_buf, half);

        if should_update {
            // ── Tremolo envelope ────────────────────────────────────
            // ~344 updates/sec → ramp 0..1 in ~1 sec (step ≈ 0.003).
            const STEP: f32 = 0.003;
            *gain += STEP * *gain_dir;
            if *gain >= 1.0 {
                *gain = 1.0;
                *gain_dir = -1.0;
            } else if *gain <= 0.0 {
                *gain = 0.0;
                *gain_dir = 1.0;
            }
            synth.amp.gain(*gain);

            // ── Run the pipeline ────────────────────────────────────
            if let Some(mono) = synth.process() {
                // Fan-out: same block to both L and R.
                let inputs: [Option<AudioBlockRef>; 2] =
                    [Some(mono.clone()), Some(mono)];
                output.update(&inputs, &mut []);
            }
        }

        *toggle += 1;
        if *toggle % 172 == 0 {
            led.toggle();
        }

        // Re-arm DMA.
        unsafe {
            let buf = core::slice::from_raw_parts(
                core::ptr::addr_of!(DMA_TX_BUF) as *const u32,
                DMA_BUF_LEN,
            );
            channel::set_source_linear_buffer(dma_chan, buf);
            dma_chan.set_transfer_iterations(DMA_BUF_LEN as u16);
            dma_chan.enable();
        }
    }
}
