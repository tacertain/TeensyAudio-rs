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
//! Once a second a low-priority task reports over USB serial how many transmit
//! and receive interrupts ran, the peak level seen on each input channel, and how many
//! FIFO errors the SAI flagged on each side. A receive count of zero means no
//! data is arriving from the codec at all; a peak of zero with a healthy count
//! means data is arriving and it is silence.
//!
//! # Two things this example has to get right
//!
//! **A FIFO error latches.** After an overflow or an underrun the SAI holds
//! that FIFO idle until the error flag is cleared. With DMA that is a trap:
//! no data means no DMA request, so no interrupt runs that could clear the
//! flag, and the stream is dead until reset. Both interrupts clear any error
//! they find, so a late interrupt costs a click and not the stream.
//!
//! **The receiver is switched on last.** It supplies the bit and frame
//! clocks, so it is tempting to switch it on first. But then its FIFO
//! overflows while the codec is being set up, and emptying a FIFO while the
//! receiver runs lands in mid-frame: the next word stored is the right
//! channel, and left and right are swapped from then on. Switched on with its
//! DMA channel already waiting, it starts at a frame boundary, left first,
//! and never overflows. The codec does not need it: MCLK alone is enough for
//! its set-up over I2C.
//!
//! **The DMA channels are one-shot and re-armed by their interrupts.** The
//! FIFO holds 32 words, about a third of a millisecond of stereo audio, and
//! that is how long the interrupt has to re-arm. Nothing slow may run at or
//! above the audio interrupts' priority. The report is formatted in a task
//! below them for that reason: formatting it inside the transmit interrupt
//! underran the FIFO on the first report.
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
    use hal::dma::peripheral::{Destination, Source};

    use core::sync::atomic::{AtomicU16, AtomicU32, Ordering};
    use imxrt_log as logging;
    use rtic_monotonics::systick::*;

    use teensy_audio::block::{AudioBlockMut, AudioBlockRef};
    use teensy_audio::codec::{Input, Sgtl5000};
    use teensy_audio::io::input_i2s::AudioInputI2S;
    use teensy_audio::io::output_i2s::AudioOutputI2S;
    use teensy_audio::node::AudioNode;

    const AUDIO_BLOCK_SAMPLES: usize = 128;
    const DMA_BUF_LEN: usize = AUDIO_BLOCK_SAMPLES * 2;

    type SaiRx = hal::sai::Rx;

    // ── Input monitor ────────────────────────────────────────────────
    //
    // The two DMA interrupts only add to these counters. A low-priority task
    // reads and clears them once a second and does the formatting, so nothing
    // slow runs at the audio interrupts' priority.

    static RX_IRQS: AtomicU32 = AtomicU32::new(0);
    static TX_IRQS: AtomicU32 = AtomicU32::new(0);
    static RX_FIFO_ERRORS: AtomicU32 = AtomicU32::new(0);
    static TX_FIFO_ERRORS: AtomicU32 = AtomicU32::new(0);
    /// Largest magnitude seen on each input channel, 0 to 32768.
    static PEAK_L: AtomicU16 = AtomicU16::new(0);
    static PEAK_R: AtomicU16 = AtomicU16::new(0);

    // ── FIFO errors ──────────────────────────────────────────────────

    /// The SAI's write-one-to-clear flags: word start, sync error, FIFO error.
    const SAI_W1C: u32 = (1 << 20) | (1 << 19) | (1 << 18);
    const SAI_FIFO_ERROR: u32 = 1 << 18;

    /// Clear a latched FIFO error on either side, and count it.
    fn clear_fifo_errors() {
        let sai = unsafe { ral::sai::SAI1::instance() };
        if ral::read_reg!(ral::sai, sai, TCSR) & SAI_FIFO_ERROR != 0 {
            ral::modify_reg!(ral::sai, sai, TCSR, |r| (r & !SAI_W1C) | SAI_FIFO_ERROR);
            TX_FIFO_ERRORS.fetch_add(1, Ordering::Relaxed);
        }
        if ral::read_reg!(ral::sai, sai, RCSR) & SAI_FIFO_ERROR != 0 {
            ral::modify_reg!(ral::sai, sai, RCSR, |r| (r & !SAI_W1C) | SAI_FIFO_ERROR);
            RX_FIFO_ERRORS.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Hand the chip to the Teensy bootloader, as Teensyduino does.
    #[cfg(feature = "auto-bootloader")]
    fn enter_bootloader() -> ! {
        unsafe { core::arch::asm!("bkpt #251") };
        loop {
            cortex_m::asm::nop();
        }
    }

    /// A peak as dBFS, for the log. Silence is reported as -99.
    fn dbfs(peak: u16) -> i32 {
        if peak == 0 {
            return -99;
        }
        (20.0 * libm::log10f(peak as f32 / 32768.0)) as i32
    }

    // ── RTIC resources ───────────────────────────────────────────────

    #[local]
    struct Local {
        led: board::Led,
        dma_tx: Channel,
        dma_rx: Channel,
        output: AudioOutputI2S,
        _sai_rx: SaiRx,
        poller: logging::Poller,
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
            usb,
            ..
        } = board::t41(cx.device);

        let led = board::led(&mut gpio2, pins.p13);
        let poller = logging::log::usbd(usb, logging::Interrupts::Enabled).unwrap();

        Systick::start(
            cx.core.SYST,
            board::ARM_FREQUENCY,
            rtic_monotonics::create_systick_token!(),
        );
        report::spawn().unwrap();

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
        let (Some(mut sai_tx), Some(mut sai_rx)) = sai
            .split(32, 2, hal::sai::Packing::None, &sai_config)
            .expect("SAI packing")
        else {
            panic!("SAI split failed");
        };

        // The receiver is switched on last; see the top of this file.

        // ── I2C + SGTL5000 codec ────────────────────────────────────
        let i2c = board::lpi2c(lpi2c1, pins.p19, pins.p18, board::Lpi2cClockSpeed::KHz400);
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
        dma_tx.set_channel_configuration(Configuration::enable(sai_tx.destination_signal()));
        unsafe {
            let buf = core::slice::from_raw_parts(
                core::ptr::addr_of!(DMA_TX_BUF) as *const u32,
                DMA_BUF_LEN,
            );
            channel::set_source_linear_buffer(&mut dma_tx, buf);
            channel::set_destination_hardware(&mut dma_tx, sai_tx.destination_address());
            dma_tx.set_minor_loop_bytes(core::mem::size_of::<u32>() as u32);
            dma_tx.set_transfer_iterations(DMA_BUF_LEN as u16);
        }

        // ── DMA channel 1 → SAI1 RX ────────────────────────────────
        //
        // The mirror image of the transmit channel above: route the SAI's
        // receive request to this channel, and read from its data register.
        // Without these the channel is armed and never triggered, and the
        // input node only ever sees an empty buffer.
        let mut dma_rx = dma[1].take().expect("DMA ch1");
        dma_rx.disable();
        dma_rx.set_disable_on_completion(true);
        dma_rx.set_interrupt_on_completion(true);
        dma_rx.set_channel_configuration(Configuration::enable(sai_rx.source_signal()));

        unsafe {
            let buf = core::slice::from_raw_parts_mut(
                core::ptr::addr_of_mut!(DMA_RX_BUF) as *mut u32,
                DMA_BUF_LEN,
            );
            channel::set_source_hardware(&mut dma_rx, sai_rx.source_address());
            channel::set_destination_linear_buffer(&mut dma_rx, buf);
            dma_rx.set_minor_loop_bytes(core::mem::size_of::<u32>() as u32);
            dma_rx.set_transfer_iterations(DMA_BUF_LEN as u16);
        }

        // Start everything: the DMA channels, then the requests, then the
        // receiver, which supplies the clocks, then the transmitter.
        unsafe {
            dma_tx.enable();
            dma_rx.enable();
        }
        sai_rx.enable_dma_receive();
        sai_tx.enable_dma_transmit();
        sai_rx.set_enable(true);
        sai_tx.set_enable(true);

        (
            Shared { input },
            Local {
                led,
                dma_tx,
                dma_rx,
                output,
                _sai_rx: sai_rx,
                poller,
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

        RX_IRQS.fetch_add(1, Ordering::Relaxed);
        clear_fifo_errors();

        // De-interleave captured audio into the input node's working blocks.
        let dma_buf = unsafe { &*DMA_RX_BUF.as_ptr() };

        // Peak of each channel. The 16-bit sample is the top half of each
        // 32-bit word, left then right.
        let (mut peak_l, mut peak_r) = (0u16, 0u16);
        for frame in dma_buf.chunks_exact(2) {
            peak_l = peak_l.max(((frame[0] >> 16) as i16).unsigned_abs());
            peak_r = peak_r.max(((frame[1] >> 16) as i16).unsigned_abs());
        }
        PEAK_L.fetch_max(peak_l, Ordering::Relaxed);
        PEAK_R.fetch_max(peak_r, Ordering::Relaxed);

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

                let l: Option<AudioBlockRef> = input_outs[0].take().map(|b| b.into_shared());
                let r: Option<AudioBlockRef> = input_outs[1].take().map(|b| b.into_shared());
                output.update(&[l, r], &mut []);
            });
        }

        clear_fifo_errors();
        TX_IRQS.fetch_add(1, Ordering::Relaxed);

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

    // ── Input monitor report ─────────────────────────────────────────

    #[task]
    async fn report(_cx: report::Context) {
        let mut seconds = 0u32;
        loop {
            Systick::delay(1000.millis()).await;
            seconds += 1;

            #[cfg(feature = "auto-bootloader")]
            if seconds > 12 {
                enter_bootloader();
            }

            let peak_l = PEAK_L.swap(0, Ordering::Relaxed);
            let peak_r = PEAK_R.swap(0, Ordering::Relaxed);
            log::info!(
                "[{:>4}s] tx_irqs={} rx_irqs={} fifo_errors tx={} rx={}  input peak L={} ({} dBFS) R={} ({} dBFS)",
                seconds,
                TX_IRQS.swap(0, Ordering::Relaxed),
                RX_IRQS.swap(0, Ordering::Relaxed),
                TX_FIFO_ERRORS.swap(0, Ordering::Relaxed),
                RX_FIFO_ERRORS.swap(0, Ordering::Relaxed),
                peak_l,
                dbfs(peak_l),
                peak_r,
                dbfs(peak_r),
            );
        }
    }

    // ── USB serial logging ───────────────────────────────────────────

    #[task(binds = USB_OTG1, local = [poller])]
    fn usb_log(cx: usb_log::Context) {
        cx.local.poller.poll();
    }
}
