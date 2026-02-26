//! Sine tone playback — simplest possible audio example.
//!
//! Generates a 440 Hz sine wave and plays it through both channels of the
//! Teensy Audio Shield headphone output.
//!
//! Hardware: Teensy 4.1 + Audio Shield Rev D (SGTL5000)
//!
//! Audio graph:
//! ```text
//!   AudioSynthSine ──► AudioOutputI2S (left + right)
//!   SGTL5000 codec: enable + volume
//! ```
//!
//! Pins (directly on the Audio Shield):
//!   p23: SAI1_MCLK    p26: SAI1_TX_BCLK    p27: SAI1_TX_SYNC (LRCLK)
//!   p7:  SAI1_TX_DATA0 p20: SAI1_RX_SYNC    p21: SAI1_RX_BCLK
//!   p8:  SAI1_RX_DATA0 p18: LPI2C1_SDA      p19: LPI2C1_SCL

#![no_std]
#![no_main]
#![allow(static_mut_refs)] // DMA buffer access in ISR — standard embedded pattern

use teensy4_panic as _;

// ── Spin-loop delay for SGTL5000 init (implements embedded-hal 1.0) ──

/// Simple delay using ARM `NOP` spin-loop. The SGTL5000 codec driver
/// requires `embedded_hal::delay::DelayNs`; `cortex_m::delay::Delay`
/// only provides the 0.2 trait, so we roll our own.
struct AsmDelay;

impl embedded_hal::delay::DelayNs for AsmDelay {
    fn delay_ns(&mut self, ns: u32) {
        // Teensy 4.1 @ 600 MHz → 1 ns ≈ 0.6 cycles. Round up.
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
    use teensy_audio::nodes::AudioSynthSine;

    const AUDIO_BLOCK_SAMPLES: usize = 128;
    const DMA_BUF_LEN: usize = AUDIO_BLOCK_SAMPLES;

    type SaiTx = hal::sai::Tx<1, 32, 2, hal::sai::PackingNone>;

    // ── RTIC resources ───────────────────────────────────────────────

    #[local]
    struct Local {
        led: board::Led,
        dma_chan: Channel,
        sai_tx: SaiTx,
        output: AudioOutputI2S,
        sine: AudioSynthSine,
    }

    #[shared]
    struct Shared {}

    /// DMA transmit buffer in non-cached OCRAM.
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

        // ── MCLK direction: output on pin 23 ────────────────────────
        unsafe {
            let gpr = ral::iomuxc_gpr::IOMUXC_GPR::instance();
            ral::modify_reg!(ral::iomuxc_gpr, gpr, GPR1, SAI1_MCLK_DIR: 1);
        }

        // ── Configure SAI1 for I2S ──────────────────────────────────
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
        // Enable RX first — it is the clock source in TxFollowRx mode.
        drop(sai_rx); // RX is enabled at split; we keep TX handle.

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

        // ── Audio nodes ─────────────────────────────────────────────
        let mut sine = AudioSynthSine::new();
        sine.frequency(440.0);
        sine.amplitude(0.8);

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
                sine,
            },
        )
    }

    // ── DMA ISR: interleave audio into DMA buffer ────────────────────

    #[task(binds = DMA0_DMA16, local = [led, dma_chan, sai_tx, output, sine, toggle: u32 = 0], priority = 2)]
    fn dma_isr(cx: dma_isr::Context) {
        let dma_chan = cx.local.dma_chan;
        let output = cx.local.output;
        let sine = cx.local.sine;
        let led = cx.local.led;
        let toggle = cx.local.toggle;

        // Acknowledge DMA interrupt.
        while dma_chan.is_interrupt() {
            dma_chan.clear_interrupt();
        }
        dma_chan.clear_complete();

        // Determine which half the DMA completed.
        let half = if *toggle % 2 == 0 {
            DmaHalf::First
        } else {
            DmaHalf::Second
        };

        // Let the output node interleave queued blocks into the DMA buffer.
        let dma_buf = unsafe { &mut *DMA_TX_BUF.as_mut_ptr() };
        let should_update = output.isr(dma_buf, half);

        if should_update {
            // ── Audio pipeline: sine → output ───────────────────────
            let mut sine_outs: [Option<AudioBlockMut>; 1] =
                [AudioBlockMut::alloc()];
            sine.update(&[], &mut sine_outs);

            // Fan-out mono sine to both L and R channels.
            let shared: Option<AudioBlockRef> =
                sine_outs[0].take().map(|b| b.into_shared());
            let inputs: [Option<AudioBlockRef>; 2] =
                [shared.clone(), shared];
            output.update(&inputs, &mut []);
        }

        *toggle += 1;

        // LED heartbeat (~1.3 Hz at 44 100 Hz / 128 samples ≈ 344 updates/s).
        if *toggle % 172 == 0 {
            led.toggle();
        }

        // Re-arm DMA transfer.
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
