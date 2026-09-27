//! Line-in passthrough with a diagnostic harness, for the bench.
//!
//! The audio path is the same as `line_in_passthrough`. Around it:
//!
//! - **A report twice a second** from a timer interrupt *above* the audio
//!   interrupts' priority, flushed by a USB interrupt above that. It prints
//!   even if the audio interrupts are stuck or storming. Each line has the
//!   interrupt counts, the input peaks, the SAI and DMA status registers, and
//!   which interrupts are active or pending.
//! - **A hard-fault record.** A fault is written to memory that survives a
//!   reset, the chip is reset, and the next boot prints it. After two faults
//!   in a row the audio path is left off, so the report can still be read.
//! - **It enters the bootloader by itself** after `RUN_SECONDS`, so the next
//!   build can be flashed without pressing the button.
//!
//! Hardware: Teensy 4.1 + Audio Shield Rev D (SGTL5000)

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU32, Ordering};

use teensy4_bsp::rt::{exception, ExceptionFrame};
use teensy4_panic as _;

/// Simple delay via ARM spin-loop — implements `embedded_hal::delay::DelayNs`.
struct AsmDelay;

impl embedded_hal::delay::DelayNs for AsmDelay {
    fn delay_ns(&mut self, ns: u32) {
        let cycles = (ns as u64 * 6 + 9) / 10;
        cortex_m::asm::delay(cycles as u32);
    }
}

// ── Breadcrumbs and the fault record ─────────────────────────────────

/// How far the program got. Written as it goes, read by the report and by
/// the fault handler.
static STAGE: AtomicU32 = AtomicU32::new(0);

#[repr(C)]
#[derive(Clone, Copy)]
struct CrashRecord {
    magic: u32,
    count: u32,
    pc: u32,
    lr: u32,
    cfsr: u32,
    hfsr: u32,
    bfar: u32,
    mmfar: u32,
    stage: u32,
}

const CRASH_MAGIC: u32 = 0xC0DE_FA17;

/// Not initialised at start-up, so it survives a reset (not a power cycle).
#[link_section = ".uninit.crash"]
static mut CRASH: MaybeUninit<CrashRecord> = MaybeUninit::uninit();

fn crash_record() -> Option<CrashRecord> {
    let rec = unsafe { core::ptr::read_volatile(CRASH.as_ptr()) };
    (rec.magic == CRASH_MAGIC).then_some(rec)
}

fn clear_crash_record() {
    unsafe { core::ptr::write_volatile(core::ptr::addr_of_mut!((*CRASH.as_mut_ptr()).magic), 0) };
}

#[exception]
unsafe fn HardFault(ef: &ExceptionFrame) -> ! {
    let previous = crash_record().map_or(0, |r| r.count);
    let rec = CrashRecord {
        magic: CRASH_MAGIC,
        count: previous + 1,
        pc: ef.pc(),
        lr: ef.lr(),
        cfsr: unsafe { core::ptr::read_volatile(0xE000_ED28 as *const u32) },
        hfsr: unsafe { core::ptr::read_volatile(0xE000_ED2C as *const u32) },
        mmfar: unsafe { core::ptr::read_volatile(0xE000_ED34 as *const u32) },
        bfar: unsafe { core::ptr::read_volatile(0xE000_ED38 as *const u32) },
        stage: STAGE.load(Ordering::Relaxed),
    };
    unsafe { core::ptr::write_volatile(CRASH.as_mut_ptr(), rec) };
    cortex_m::peripheral::SCB::sys_reset()
}

/// Hand the chip to the Teensy bootloader, as Teensyduino does.
fn enter_bootloader() -> ! {
    clear_crash_record();
    unsafe { core::arch::asm!("bkpt #251") };
    loop {
        cortex_m::asm::nop();
    }
}

#[rtic::app(device = teensy4_bsp, peripherals = true, dispatchers = [KPP])]
mod app {
    use super::*;
    use bsp::board;
    use bsp::hal;
    use bsp::ral;
    use teensy4_bsp as bsp;

    use core::sync::atomic::AtomicU16;
    use hal::dma::channel::{self, Channel, Configuration};
    use hal::dma::peripheral::{Destination, Source};
    use imxrt_log as logging;

    use teensy_audio::block::{AudioBlockMut, AudioBlockRef};
    use teensy_audio::codec::{Input, Sgtl5000};
    use teensy_audio::io::input_i2s::AudioInputI2S;
    use teensy_audio::io::output_i2s::AudioOutputI2S;
    use teensy_audio::node::AudioNode;

    const AUDIO_BLOCK_SAMPLES: usize = 128;
    const DMA_BUF_LEN: usize = AUDIO_BLOCK_SAMPLES * 2;

    /// Reports per second.
    const REPORTS_PER_SECOND: u32 = 2;
    /// How long to run before entering the bootloader.
    const RUN_SECONDS: u32 = 12;
    const REPORT_CHANNEL: hal::pit::Channel = hal::pit::Channel::Chan0;

    /// After this many hard faults in a row, leave the audio path off.
    const SAFE_MODE_AFTER: u32 = 2;

    static RX_IRQS: AtomicU32 = AtomicU32::new(0);
    static TX_IRQS: AtomicU32 = AtomicU32::new(0);
    static RX_FIFO_ERRORS: AtomicU32 = AtomicU32::new(0);
    static TX_FIFO_ERRORS: AtomicU32 = AtomicU32::new(0);

    // A snapshot taken by the timer interrupt, formatted later by a task.
    static SNAP_TICKS: AtomicU32 = AtomicU32::new(0);
    static SNAP_STAGE: AtomicU32 = AtomicU32::new(0);
    static SNAP_TCSR: AtomicU32 = AtomicU32::new(0);
    static SNAP_RCSR: AtomicU32 = AtomicU32::new(0);
    static SNAP_ES: AtomicU32 = AtomicU32::new(0);
    static SNAP_ERQ: AtomicU32 = AtomicU32::new(0);
    static SNAP_ACTIVE: AtomicU32 = AtomicU32::new(0);
    static SNAP_PENDING: AtomicU32 = AtomicU32::new(0);

    /// The SAI's write-one-to-clear flags: word start, sync error, FIFO error.
    const SAI_W1C: u32 = (1 << 20) | (1 << 19) | (1 << 18);
    const SAI_FIFO_ERROR: u32 = 1 << 18;
    const SAI_FIFO_RESET: u32 = 1 << 25;

    /// Empty the receive FIFO and clear its error flags.
    ///
    /// A FIFO error latches: the receiver holds its FIFO empty until the flag
    /// is cleared, so no data arrives, no DMA request is made, and no
    /// interrupt runs that could clear it.
    fn restart_rx_fifo() {
        let sai = unsafe { ral::sai::SAI1::instance() };
        ral::modify_reg!(ral::sai, sai, RCSR, |r| (r & !SAI_W1C)
            | SAI_FIFO_RESET
            | SAI_W1C);
    }

    /// Clear a latched FIFO error on either side. Returns which had one.
    fn clear_fifo_errors() -> (bool, bool) {
        let sai = unsafe { ral::sai::SAI1::instance() };
        let tx = ral::read_reg!(ral::sai, sai, TCSR) & SAI_FIFO_ERROR != 0;
        let rx = ral::read_reg!(ral::sai, sai, RCSR) & SAI_FIFO_ERROR != 0;
        if tx {
            ral::modify_reg!(ral::sai, sai, TCSR, |r| (r & !SAI_W1C) | SAI_FIFO_ERROR);
        }
        if rx {
            ral::modify_reg!(ral::sai, sai, RCSR, |r| (r & !SAI_W1C) | SAI_FIFO_ERROR);
        }
        (tx, rx)
    }

    fn count_fifo_errors() {
        let (tx_err, rx_err) = clear_fifo_errors();
        if tx_err {
            TX_FIFO_ERRORS.fetch_add(1, Ordering::Relaxed);
        }
        if rx_err {
            RX_FIFO_ERRORS.fetch_add(1, Ordering::Relaxed);
        }
    }
    static PEAK_L: AtomicU16 = AtomicU16::new(0);
    static PEAK_R: AtomicU16 = AtomicU16::new(0);
    /// 1 if the audio path was started, 0 if it was left off (safe mode).
    static AUDIO_STARTED: AtomicU32 = AtomicU32::new(0);

    #[local]
    struct Local {
        led: board::Led,
        dma_tx: Channel,
        dma_rx: Channel,
        output: AudioOutputI2S,
        sai_rx: hal::sai::Rx,
        _sai_tx: hal::sai::Tx,
        poller: logging::Poller,
        pit: hal::pit::Pit,
    }

    #[shared]
    struct Shared {
        input: AudioInputI2S,
    }

    #[link_section = ".uninit.dma_tx"]
    static mut DMA_TX_BUF: MaybeUninit<[u32; DMA_BUF_LEN]> = MaybeUninit::uninit();

    #[link_section = ".uninit.dma_rx"]
    static mut DMA_RX_BUF: MaybeUninit<[u32; DMA_BUF_LEN]> = MaybeUninit::uninit();

    #[init]
    fn init(cx: init::Context) -> (Shared, Local) {
        STAGE.store(1, Ordering::Relaxed);
        let board::Resources {
            mut gpio2,
            pins,
            mut dma,
            sai1,
            lpi2c1,
            usb,
            mut pit,
            ..
        } = board::t41(cx.device);

        let led = board::led(&mut gpio2, pins.p13);
        let poller = logging::log::usbd(usb, logging::Interrupts::Enabled).unwrap();
        STAGE.store(2, Ordering::Relaxed);

        let safe_mode = crash_record().is_some_and(|r| r.count >= SAFE_MODE_AFTER);

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
        STAGE.store(3, Ordering::Relaxed);

        // Enable RX — clock source in TxFollowRx mode.
        sai_rx.set_enable(true);

        // ── I2C + SGTL5000 codec ────────────────────────────────────
        let i2c = board::lpi2c(lpi2c1, pins.p19, pins.p18, board::Lpi2cClockSpeed::KHz400);
        let mut codec = Sgtl5000::new(i2c, AsmDelay);
        codec.enable().expect("SGTL5000 enable");
        codec.volume(0.4).expect("SGTL5000 volume");
        codec
            .input_select(Input::LineIn)
            .expect("SGTL5000 input select");
        STAGE.store(4, Ordering::Relaxed);

        let input = AudioInputI2S::new(false);
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

        // ── DMA channel 1 ← SAI1 RX ────────────────────────────────
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
        STAGE.store(5, Ordering::Relaxed);

        // ── The report timer ────────────────────────────────────────
        pit.set_load_timer_value(REPORT_CHANNEL, board::PERCLK_FREQUENCY / REPORTS_PER_SECOND);
        pit.set_interrupt_enable(REPORT_CHANNEL, true);
        pit.enable(REPORT_CHANNEL);

        // ── Start the audio path, unless it has faulted twice ───────
        if !safe_mode {
            // The DMA channels first, then the requests, and only then the
            // FIFO: the receiver has been running since before the codec was
            // set up, so its FIFO overflowed long ago.
            unsafe {
                dma_tx.enable();
                dma_rx.enable();
            }
            sai_rx.enable_dma_receive();
            sai_tx.enable_dma_transmit();
            restart_rx_fifo();
            sai_tx.set_enable(true);
            AUDIO_STARTED.store(1, Ordering::Relaxed);
        }
        STAGE.store(6, Ordering::Relaxed);

        (
            Shared { input },
            Local {
                led,
                dma_tx,
                dma_rx,
                output,
                sai_rx,
                _sai_tx: sai_tx,
                poller,
                pit,
            },
        )
    }

    // ── RX DMA ISR ───────────────────────────────────────────────────

    #[task(binds = DMA1_DMA17, shared = [input], local = [dma_rx, sai_rx], priority = 3)]
    fn dma_rx_isr(mut cx: dma_rx_isr::Context) {
        STAGE.store(20, Ordering::Relaxed);
        let dma_rx = cx.local.dma_rx;
        let sai_rx = cx.local.sai_rx;

        while dma_rx.is_interrupt() {
            dma_rx.clear_interrupt();
        }
        dma_rx.clear_complete();

        RX_IRQS.fetch_add(1, Ordering::Relaxed);
        let _ = sai_rx;
        count_fifo_errors();

        let dma_buf = unsafe { &*DMA_RX_BUF.as_ptr() };

        let (mut peak_l, mut peak_r) = (0u16, 0u16);
        for frame in dma_buf.chunks_exact(2) {
            peak_l = peak_l.max(((frame[0] >> 16) as i16).unsigned_abs());
            peak_r = peak_r.max(((frame[1] >> 16) as i16).unsigned_abs());
        }
        PEAK_L.fetch_max(peak_l, Ordering::Relaxed);
        PEAK_R.fetch_max(peak_r, Ordering::Relaxed);

        STAGE.store(21, Ordering::Relaxed);
        cx.shared.input.lock(|input| {
            input.isr(dma_buf);
        });

        STAGE.store(22, Ordering::Relaxed);
        unsafe {
            let buf = core::slice::from_raw_parts_mut(
                core::ptr::addr_of_mut!(DMA_RX_BUF) as *mut u32,
                DMA_BUF_LEN,
            );
            channel::set_destination_linear_buffer(dma_rx, buf);
            dma_rx.set_transfer_iterations(DMA_BUF_LEN as u16);
            dma_rx.enable();
        }
        STAGE.store(29, Ordering::Relaxed);
    }

    // ── TX DMA ISR ───────────────────────────────────────────────────

    #[task(binds = DMA0_DMA16, shared = [input], local = [led, dma_tx, output, toggle: u32 = 0], priority = 3)]
    fn dma_tx_isr(mut cx: dma_tx_isr::Context) {
        STAGE.store(10, Ordering::Relaxed);
        let dma_tx = cx.local.dma_tx;
        let output = cx.local.output;
        let led = cx.local.led;
        let toggle = cx.local.toggle;

        while dma_tx.is_interrupt() {
            dma_tx.clear_interrupt();
        }
        dma_tx.clear_complete();

        let dma_buf = unsafe { &mut *DMA_TX_BUF.as_mut_ptr() };
        STAGE.store(11, Ordering::Relaxed);
        let should_update = output.isr(dma_buf);

        if should_update {
            STAGE.store(12, Ordering::Relaxed);
            cx.shared.input.lock(|input| {
                let mut input_outs: [Option<AudioBlockMut>; 2] =
                    [AudioBlockMut::alloc(), AudioBlockMut::alloc()];
                STAGE.store(13, Ordering::Relaxed);
                input.update(&[], &mut input_outs);

                STAGE.store(14, Ordering::Relaxed);
                let l: Option<AudioBlockRef> = input_outs[0].take().map(|b| b.into_shared());
                let r: Option<AudioBlockRef> = input_outs[1].take().map(|b| b.into_shared());
                STAGE.store(15, Ordering::Relaxed);
                output.update(&[l, r], &mut []);
            });
        }

        count_fifo_errors();

        TX_IRQS.fetch_add(1, Ordering::Relaxed);
        *toggle += 1;
        if *toggle % 172 == 0 {
            led.toggle();
        }

        STAGE.store(16, Ordering::Relaxed);
        unsafe {
            let buf = core::slice::from_raw_parts(
                core::ptr::addr_of!(DMA_TX_BUF) as *const u32,
                DMA_BUF_LEN,
            );
            channel::set_source_linear_buffer(dma_tx, buf);
            dma_tx.set_transfer_iterations(DMA_BUF_LEN as u16);
            dma_tx.enable();
        }
        STAGE.store(19, Ordering::Relaxed);
    }

    // ── The report ───────────────────────────────────────────────────
    //
    // The timer interrupt runs above the audio interrupts, so it always gets
    // in: it takes a snapshot, which costs microseconds, and it is what enters
    // the bootloader. The formatting is slow and runs in a task below them.

    #[task(binds = PIT, local = [pit, ticks: u32 = 0], priority = 4)]
    fn snapshot(cx: snapshot::Context) {
        let pit = cx.local.pit;
        while pit.is_elapsed(REPORT_CHANNEL) {
            pit.clear_elapsed(REPORT_CHANNEL);
        }
        *cx.local.ticks += 1;
        let ticks = *cx.local.ticks;

        if ticks > RUN_SECONDS * REPORTS_PER_SECOND + 1 {
            enter_bootloader();
        }

        let sai = unsafe { ral::sai::SAI1::instance() };
        let dma = unsafe { ral::dma::DMA::instance() };
        let nvic = unsafe { &*cortex_m::peripheral::NVIC::PTR };
        SNAP_TICKS.store(ticks, Ordering::Relaxed);
        SNAP_STAGE.store(STAGE.load(Ordering::Relaxed), Ordering::Relaxed);
        SNAP_TCSR.store(ral::read_reg!(ral::sai, sai, TCSR), Ordering::Relaxed);
        SNAP_RCSR.store(ral::read_reg!(ral::sai, sai, RCSR), Ordering::Relaxed);
        SNAP_ES.store(ral::read_reg!(ral::dma, dma, ES), Ordering::Relaxed);
        SNAP_ERQ.store(ral::read_reg!(ral::dma, dma, ERQ), Ordering::Relaxed);
        SNAP_ACTIVE.store(nvic.iabr[0].read(), Ordering::Relaxed);
        SNAP_PENDING.store(nvic.ispr[0].read(), Ordering::Relaxed);

        report::spawn().ok();
    }

    #[task(priority = 1)]
    async fn report(_cx: report::Context) {
        let ticks = SNAP_TICKS.load(Ordering::Relaxed);
        if ticks > RUN_SECONDS * REPORTS_PER_SECOND {
            log::info!("DIAG done: entering the bootloader");
            return;
        }

        log::info!(
            "DIAG t={}.{} stage={} audio={} tx={} rx={} tx_fifo_err={} rx_fifo_err={} peakL={} peakR={}",
            ticks / REPORTS_PER_SECOND,
            (ticks % REPORTS_PER_SECOND) * 5,
            SNAP_STAGE.load(Ordering::Relaxed),
            AUDIO_STARTED.load(Ordering::Relaxed),
            TX_IRQS.load(Ordering::Relaxed),
            RX_IRQS.load(Ordering::Relaxed),
            TX_FIFO_ERRORS.load(Ordering::Relaxed),
            RX_FIFO_ERRORS.load(Ordering::Relaxed),
            PEAK_L.swap(0, Ordering::Relaxed),
            PEAK_R.swap(0, Ordering::Relaxed),
        );
        log::info!(
            "DIAG   TCSR={:08X} RCSR={:08X}  DMA ES={:08X} ERQ={:08X}  active={:08X} pending={:08X}",
            SNAP_TCSR.load(Ordering::Relaxed),
            SNAP_RCSR.load(Ordering::Relaxed),
            SNAP_ES.load(Ordering::Relaxed),
            SNAP_ERQ.load(Ordering::Relaxed),
            SNAP_ACTIVE.load(Ordering::Relaxed),
            SNAP_PENDING.load(Ordering::Relaxed),
        );
        if ticks <= 2 {
            match crash_record() {
                Some(r) => log::warn!(
                    "DIAG   PREVIOUS HARD FAULT #{}: pc={:08X} lr={:08X} cfsr={:08X} hfsr={:08X} bfar={:08X} mmfar={:08X} stage={}",
                    r.count,
                    r.pc,
                    r.lr,
                    r.cfsr,
                    r.hfsr,
                    r.bfar,
                    r.mmfar,
                    r.stage,
                ),
                None => log::info!("DIAG   no hard fault recorded"),
            }
        }
    }

    // ── USB serial logging, above everything ─────────────────────────

    #[task(binds = USB_OTG1, local = [poller], priority = 2)]
    fn usb_log(cx: usb_log::Context) {
        cx.local.poller.poll();
    }
}
