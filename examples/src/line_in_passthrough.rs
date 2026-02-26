//! Line-in passthrough — headphone monitoring of the line input.
//!
//! Reads stereo audio from the Audio Shield line-in jacks and sends it
//! straight to the headphone output. Useful for verifying the codec,
//! I2S, and DMA data paths end-to-end.
//!
//! Hardware: Teensy 4.1 + Audio Shield Rev D (SGTL5000)
//!
//! Audio graph:
//! ```text
//!   AudioInputI2S (L) ──► AudioOutputI2S (L)
//!   AudioInputI2S (R) ──► AudioOutputI2S (R)
//!   SGTL5000: line-in selected, headphone output
//! ```
//!
//! Both DMA channels share the same ISR priority so they cannot preempt
//! each other. The TX DMA ISR drives the audio graph update; the RX DMA
//! ISR only captures incoming data. The input node is shared between the
//! two ISRs via an RTIC shared resource.

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use teensy4_panic as _;

/// Simple delay via ARM spin-loop — implements `embedded_hal::delay::DelayNs`.
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
    use teensy_audio::codec::{Input, Sgtl5000};
    use teensy_audio::io::input_i2s::AudioInputI2S;
    use teensy_audio::io::output_i2s::AudioOutputI2S;
    use teensy_audio::node::AudioNode;

    const AUDIO_BLOCK_SAMPLES: usize = 128;
    const DMA_BUF_LEN: usize = AUDIO_BLOCK_SAMPLES * 2;

    type SaiRx = hal::sai::Rx<1, 32, 2, hal::sai::PackingNone>;

    // ── RTIC resources ───────────────────────────────────────────────

    #[local]
    struct Local {
        led: board::Led,
        dma_tx: Channel,
        dma_rx: Channel,
        output: AudioOutputI2S,
        _sai_rx: SaiRx,
    }

    #[shared]
    struct Shared {
        /// The input node is accessed from both the RX and TX DMA ISRs.
        input: AudioInputI2S,
    }

    #[link_section = ".uninit.dma_tx"]
    static mut DMA_TX_BUF: core::mem::MaybeUninit<[u32; DMA_BUF_LEN]> =
        core::mem::MaybeUninit::uninit();

    #[link_section = ".uninit.dma_rx"]
    static mut DMA_RX_BUF: core::mem::MaybeUninit<[u32; DMA_BUF_LEN]> =
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
        let (Some(mut sai_tx), Some(mut sai_rx)) =
            sai.split::<32, 2, hal::sai::PackingNone>(&sai_config)
        else {
            panic!("SAI split failed");
        };

        // Enable RX — clock source in TxFollowRx mode.
        sai_rx.set_enable(true);

        // ── I2C + SGTL5000 codec ────────────────────────────────────
        let i2c = board::lpi2c(
            lpi2c1,
            pins.p19,
            pins.p18,
            board::Lpi2cClockSpeed::KHz400,
        );
        let mut codec = Sgtl5000::new(i2c, AsmDelay);
        codec.enable().expect("SGTL5000 enable");
        codec.volume(0.6).expect("SGTL5000 volume");
        codec
            .input_select(Input::LineIn)
            .expect("SGTL5000 input select");

        // ── Audio nodes ─────────────────────────────────────────────
        let input = AudioInputI2S::new(false); // update driven by output ISR
        let output = AudioOutputI2S::new(true);

        // ── DMA channel 0 → SAI1 TX ────────────────────────────────
        let mut dma_tx = dma[0].take().expect("DMA ch0");
        dma_tx.disable();
        dma_tx.set_disable_on_completion(true);
        dma_tx.set_interrupt_on_completion(true);
        dma_tx.set_channel_configuration(Configuration::enable(
            sai_tx.destination_signal(),
        ));
        unsafe {
            let buf = core::slice::from_raw_parts(
                core::ptr::addr_of!(DMA_TX_BUF) as *const u32,
                DMA_BUF_LEN,
            );
            channel::set_source_linear_buffer(&mut dma_tx, buf);
            channel::set_destination_hardware(
                &mut dma_tx,
                sai_tx.destination_address(),
            );
            dma_tx.set_minor_loop_bytes(core::mem::size_of::<u32>() as u32);
            dma_tx.set_transfer_iterations(DMA_BUF_LEN as u16);
        }

        // ── DMA channel 1 → SAI1 RX ────────────────────────────────
        let mut dma_rx = dma[1].take().expect("DMA ch1");
        dma_rx.disable();
        dma_rx.set_disable_on_completion(true);
        dma_rx.set_interrupt_on_completion(true);

        unsafe {
            let buf = core::slice::from_raw_parts_mut(
                core::ptr::addr_of_mut!(DMA_RX_BUF) as *mut u32,
                DMA_BUF_LEN,
            );
            channel::set_destination_linear_buffer(&mut dma_rx, buf);
            dma_rx.set_minor_loop_bytes(core::mem::size_of::<u32>() as u32);
            dma_rx.set_transfer_iterations(DMA_BUF_LEN as u16);
        }

        // Start everything.
        sai_tx.enable_dma_transmit();
        unsafe {
            dma_tx.enable();
            dma_rx.enable();
        }
        sai_tx.set_enable(true);

        (
            Shared { input },
            Local {
                led,
                dma_tx,
                dma_rx,
                output,
                _sai_rx: sai_rx,
            },
        )
    }

    // ── RX DMA ISR: capture incoming audio data ──────────────────────

    #[task(binds = DMA1_DMA17, shared = [input], local = [dma_rx, _sai_rx], priority = 2)]
    fn dma_rx_isr(mut cx: dma_rx_isr::Context) {
        let dma_rx = cx.local.dma_rx;

        while dma_rx.is_interrupt() {
            dma_rx.clear_interrupt();
        }
        dma_rx.clear_complete();

        // De-interleave captured audio into the input node's working blocks.
        let dma_buf = unsafe { &*DMA_RX_BUF.as_ptr() };
        cx.shared.input.lock(|input| {
            input.isr(dma_buf);
        });

        // Re-arm RX DMA.
        unsafe {
            let buf = core::slice::from_raw_parts_mut(
                core::ptr::addr_of_mut!(DMA_RX_BUF) as *mut u32,
                DMA_BUF_LEN,
            );
            channel::set_destination_linear_buffer(dma_rx, buf);
            dma_rx.set_transfer_iterations(DMA_BUF_LEN as u16);
            dma_rx.enable();
        }
    }

    // ── TX DMA ISR: send audio + drive the graph update ──────────────

    #[task(binds = DMA0_DMA16, shared = [input], local = [led, dma_tx, output, toggle: u32 = 0], priority = 2)]
    fn dma_tx_isr(mut cx: dma_tx_isr::Context) {
        let dma_tx = cx.local.dma_tx;
        let output = cx.local.output;
        let led = cx.local.led;
        let toggle = cx.local.toggle;

        while dma_tx.is_interrupt() {
            dma_tx.clear_interrupt();
        }
        dma_tx.clear_complete();

        let dma_buf = unsafe { &mut *DMA_TX_BUF.as_mut_ptr() };
        let should_update = output.isr(dma_buf);

        if should_update {
            // ── Audio passthrough: input → output ───────────────────
            cx.shared.input.lock(|input| {
                let mut input_outs: [Option<AudioBlockMut>; 2] =
                    [AudioBlockMut::alloc(), AudioBlockMut::alloc()];
                input.update(&[], &mut input_outs);

                let l: Option<AudioBlockRef> =
                    input_outs[0].take().map(|b| b.into_shared());
                let r: Option<AudioBlockRef> =
                    input_outs[1].take().map(|b| b.into_shared());
                output.update(&[l, r], &mut []);
            });
        }

        *toggle += 1;
        if *toggle % 172 == 0 {
            led.toggle();
        }

        // Re-arm TX DMA.
        unsafe {
            let buf = core::slice::from_raw_parts(
                core::ptr::addr_of!(DMA_TX_BUF) as *const u32,
                DMA_BUF_LEN,
            );
            channel::set_source_linear_buffer(dma_tx, buf);
            dma_tx.set_transfer_iterations(DMA_BUF_LEN as u16);
            dma_tx.enable();
        }
    }
}
