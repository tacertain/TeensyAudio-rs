/* Audio Library for Teensy, Ladder Filter
 * Copyright (c) 2021, Richard van Hoesel
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice, development funding notice, and this permission
 * notice shall be included in all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
 * THE SOFTWARE.
 */

//-----------------------------------------------------------
// Huovilainen New Moog (HNM) model as per CMJ jun 2006
// Implemented as Teensy Audio Library compatible object
// Richard van Hoesel, Feb. 9 2021
// v1.5 adds polyphase FIR or Linear interpolation
// v1.4 FC extended to 18.7kHz, max res to 1.8, 4x oversampling,
//      and a minor Q-tuning adjustment
// v.1.03 adds oversampling, extended resonance,
// and exposes parameters input_drive and passband_gain
// v.1.02 now includes both cutoff and resonance "CV" modulation inputs
// please retain this header if you use this code.
//-----------------------------------------------------------

// https://forum.pjrc.com/threads/60488?p=271078&viewfull=1#post271078

//! The Moog ladder filter: a port of the C++ library's `AudioFilterLadder`
//! (`filter_ladder.cpp`, v1.5), Richard van Hoesel's implementation of the
//! Huovilainen New Moog model. His copyright and licence are above and travel
//! with this file.
//!
//! A per-sample kernel in `f32`, not an [`AudioNode`](crate::node::AudioNode):
//! one input sample and its cutoff CV in, one output sample out. It runs 4x
//! oversampled through the library's 36-tap polyphase FIR, interpolating and
//! decimating, which is the C++ block's default mode; its linear-interpolation
//! mode and its resonance CV input are not ported.
//!
//! Samples are full scale 1.0. The cutoff is `frequency x 2^(cv x octaves)`,
//! recomputed on every sample, so a CV sweeps it smoothly; with the CV held at
//! zero the cutoff is [`frequency`](Ladder::frequency).
//!
//! The arithmetic follows the C++ closely enough to have matched it within
//! 0.1 dB on the bench (`tacertain/teensy-pedal`'s envelope filter, which is
//! where this port was written). The truncated ln 2 in `fast_exp2f` and the
//! member defaults (drive 0.5, passband gain 0.5, resonance 0.25) are the C++'s.

use crate::constants::AUDIO_SAMPLE_RATE_EXACT;

const INTERPOLATION: usize = 4;
const TAPS: usize = 36;
const PHASE_LEN: usize = TAPS / INTERPOLATION;
const MAX_RESONANCE: f32 = 1.8;
const MAX_FREQUENCY: f32 = AUDIO_SAMPLE_RATE_EXACT * 0.425;
const MAX_OCTAVES: f32 = 7.0;

/// The C++ library's interpolation (and decimation) filter, symmetric. Its
/// taps sum to 1.0514, not 1, so the passband sits 0.87 dB above the drive
/// (0.44 dB each way); kept as the C++ has it.
#[allow(clippy::excessive_precision)]
const FIR: [f32; TAPS] = [
    -14.308_515_415_901_542e-6,
    0.001_348_560_352_009_071,
    0.004_029_285_548_698_377,
    0.007_644_563_345_368_599,
    0.010_936_856_250_494_802,
    0.011_982_063_548_666_887,
    0.008_882_946_305_001_046,
    826.659_811_647_155_607e-6,
    -0.011_008_071_930_708_746,
    -0.023_014_151_355_548_934,
    -0.029_736_402_750_934_567,
    -0.025_405_787_911_977_455,
    -0.006_012_006_772_274_640,
    0.028_729_626_071_574_525,
    0.074_466_890_595_619_062,
    0.122_757_573_409_695_370,
    0.163_145_421_379_242_955,
    0.186_152_844_567_746_417,
    0.186_152_844_567_746_417,
    0.163_145_421_379_242_955,
    0.122_757_573_409_695_370,
    0.074_466_890_595_619_062,
    0.028_729_626_071_574_525,
    -0.006_012_006_772_274_640,
    -0.025_405_787_911_977_455,
    -0.029_736_402_750_934_567,
    -0.023_014_151_355_548_934,
    -0.011_008_071_930_708_746,
    826.659_811_647_155_607e-6,
    0.008_882_946_305_001_046,
    0.011_982_063_548_666_887,
    0.010_936_856_250_494_802,
    0.007_644_563_345_368_599,
    0.004_029_285_548_698_377,
    0.001_348_560_352_009_071,
    -14.308_515_415_901_542e-6,
];

/// `2^x`, as the C++ computes it: the fraction through `(1 + f ln2 / 256)^256`.
/// The C++'s truncated ln 2 is kept on purpose; the point is to match it.
#[allow(clippy::approx_constant)]
#[inline]
fn fast_exp2f(x: f32) -> f32 {
    let (f, i) = libm::modff(x);
    let mut f = f * (0.693_147 / 256.0) + 1.0;
    for _ in 0..8 {
        f *= f;
    }
    libm::ldexpf(f, i as i32)
}

/// A rational `tanh`, exact at 0 and clamped to ±1 beyond ±3.
#[inline]
fn fast_tanh(x: f32) -> f32 {
    if x > 3.0 {
        return 1.0;
    }
    if x < -3.0 {
        return -1.0;
    }
    let x2 = x * x;
    x * (27.0 + x2) / (27.0 + 9.0 * x2)
}

/// A four-pole resonant low-pass, the Huovilainen model, 4x oversampled.
/// See the module documentation.
pub struct Ladder {
    alpha: f32,
    qadjust: f32,
    k: f32,
    fbase: f32,
    /// Octaves per unit of CV.
    octaves: f32,
    pbg: f32,
    overdrive: f32,
    host_overdrive: f32,
    z0: [f32; 4],
    z1: [f32; 4],
    /// The last PHASE_LEN inputs, each stored twice (at `i` and `i + PHASE_LEN`)
    /// so the window `up[up_pos..up_pos + PHASE_LEN]` is always contiguous,
    /// oldest first: no modulo per tap.
    up: [f32; 2 * PHASE_LEN],
    up_pos: usize,
    /// The last TAPS oversampled outputs, the same way.
    down: [f32; 2 * TAPS],
    down_pos: usize,
}

impl Ladder {
    /// A ladder at the C++ block's member defaults: 1 kHz, resonance 0.25,
    /// one octave per unit of CV, drive 0.5, passband gain 0.5.
    pub fn new() -> Self {
        let mut l = Ladder {
            alpha: 1.0,
            qadjust: 1.0,
            k: 1.0,
            fbase: 1000.0,
            octaves: 1.0,
            pbg: 0.5,
            overdrive: 0.5,
            host_overdrive: 1.0,
            z0: [0.0; 4],
            z1: [0.0; 4],
            up: [0.0; 2 * PHASE_LEN],
            up_pos: 0,
            down: [0.0; 2 * TAPS],
            down_pos: 0,
        };
        l.compute_coeffs(l.fbase);
        l
    }

    /// The cutoff with the CV at zero, Hz. Held between 5 Hz and
    /// 0.425 of the sample rate.
    pub fn frequency(&mut self, hz: f32) {
        self.fbase = hz;
        self.compute_coeffs(hz);
    }

    /// Resonance, 0 to 1.8; self-oscillation starts near 1.
    pub fn resonance(&mut self, r: f32) {
        self.k = 4.0 * r.clamp(0.0, MAX_RESONANCE);
    }

    /// How many octaves a CV of 1.0 moves the cutoff, 0 to 7.
    pub fn octave_control(&mut self, octaves: f32) {
        self.octaves = octaves.clamp(0.0, MAX_OCTAVES);
    }

    /// Passband gain, 0 to 0.5: how much of the loss that resonance causes
    /// in the passband is put back. Re-derives the drive, as the C++ does.
    pub fn passband_gain(&mut self, g: f32) {
        self.pbg = g.clamp(0.0, 0.5);
        self.input_drive(self.host_overdrive);
    }

    /// Input drive, 0 to 4: the level into the saturating stage. Above 1 it
    /// is scaled back by the passband gain, as in the C++.
    pub fn input_drive(&mut self, drive: f32) {
        self.host_overdrive = drive;
        if drive > 1.0 {
            self.host_overdrive = drive.min(4.0);
            self.overdrive = 1.0 + (self.host_overdrive - 1.0) * (1.0 - self.pbg);
        } else {
            self.overdrive = drive.max(0.0);
        }
    }

    fn compute_coeffs(&mut self, c: f32) {
        let c = c.clamp(5.0, MAX_FREQUENCY);
        let wc =
            c * (2.0 * core::f32::consts::PI / (INTERPOLATION as f32 * AUDIO_SAMPLE_RATE_EXACT));
        let wc2 = wc * wc;
        self.alpha = 0.9892 * wc - 0.4324 * wc2 + 0.1381 * wc * wc2 - 0.0202 * wc2 * wc2;
        self.qadjust = 1.006 + 0.0536 * wc - 0.095 * wc2 - 0.05 * wc2 * wc2;
    }

    #[inline(always)]
    fn lpf(&mut self, s: f32, i: usize) -> f32 {
        let mut ft = s * (1.0 / 1.3) + (0.3 / 1.3) * self.z0[i] - self.z1[i];
        ft = ft * self.alpha + self.z1[i];
        self.z1[i] = ft;
        self.z0[i] = s;
        ft
    }

    /// One input sample and its cutoff CV, both full scale 1.0, to one
    /// output sample.
    #[inline]
    pub fn process(&mut self, x: f32, cv: f32) -> f32 {
        // Upsample: the polyphase interpolator. Zero-stuffing loses a factor
        // of INTERPOLATION, which the input scaling puts back.
        let v = x * self.overdrive * INTERPOLATION as f32;
        self.up[self.up_pos] = v;
        self.up[self.up_pos + PHASE_LEN] = v;
        self.up_pos = if self.up_pos + 1 == PHASE_LEN {
            0
        } else {
            self.up_pos + 1
        };
        // Oldest first; the newest input is window[PHASE_LEN - 1].
        let window: [f32; PHASE_LEN] = self.up[self.up_pos..self.up_pos + PHASE_LEN]
            .try_into()
            .unwrap_or([0.0; PHASE_LEN]);

        let ftot = (self.fbase * fast_exp2f(cv * self.octaves)).min(MAX_FREQUENCY);
        self.compute_coeffs(ftot);
        let ktot = self.k.clamp(0.0, MAX_RESONANCE * 4.0);

        for phase in 0..INTERPOLATION {
            let mut input = 0.0;
            for k in 0..PHASE_LEN {
                input += FIR[phase + INTERPOLATION * k] * window[PHASE_LEN - 1 - k];
            }
            let u = fast_tanh(input - (self.z1[3] - self.pbg * input) * ktot * self.qadjust);
            let s1 = self.lpf(u, 0);
            let s2 = self.lpf(s1, 1);
            let s3 = self.lpf(s2, 2);
            let s4 = self.lpf(s3, 3);
            self.down[self.down_pos] = s4;
            self.down[self.down_pos + TAPS] = s4;
            self.down_pos = if self.down_pos + 1 == TAPS {
                0
            } else {
                self.down_pos + 1
            };
        }

        // Decimate: the same FIR at the newest oversampled sample. The FIR is
        // symmetric, so taking the window oldest-first gives the same sum.
        let window = &self.down[self.down_pos..self.down_pos + TAPS];
        FIR.iter().zip(window).map(|(h, v)| h * v).sum()
    }
}

impl Default for Ladder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The RMS gain for a sine of `hz` at amplitude `amp`, after a settling
    /// second, with the CV at `cv`.
    fn gain(l: &mut Ladder, hz: f32, amp: f32, cv: f32) -> f32 {
        let fs = AUDIO_SAMPLE_RATE_EXACT;
        let settle = fs as usize;
        let n = fs as usize / 2;
        let (mut sx, mut sy) = (0.0f64, 0.0f64);
        for i in 0..settle + n {
            let x = amp * libm::sinf(2.0 * core::f32::consts::PI * hz * (i as f32) / fs);
            let y = l.process(x, cv);
            if i >= settle {
                sx += (x as f64) * (x as f64);
                sy += (y as f64) * (y as f64);
            }
        }
        (sy / sx).sqrt() as f32
    }

    fn db(g: f32) -> f32 {
        20.0 * libm::log10f(g)
    }

    #[test]
    fn fast_exp2f_is_close() {
        let mut x = -7.0f32;
        while x <= 7.0 {
            let want = libm::exp2f(x);
            let got = fast_exp2f(x);
            assert!(
                (got / want - 1.0).abs() < 1e-3,
                "2^{} = {} not {}",
                x,
                got,
                want
            );
            x += 0.37;
        }
        assert_eq!(fast_exp2f(1.0), 2.0);
        assert_eq!(fast_exp2f(0.0), 1.0);
    }

    #[test]
    fn silence_stays_silent() {
        let mut l = Ladder::new();
        for _ in 0..1000 {
            assert_eq!(l.process(0.0, 0.3), 0.0);
        }
    }

    #[test]
    fn passes_below_the_cutoff_at_the_drive() {
        let mut l = Ladder::new();
        l.frequency(5000.0);
        l.resonance(0.0);
        // Small enough that the saturator is linear: the gain is the drive
        // times the FIR's DC gain, once up and once down.
        let fir: f32 = FIR.iter().sum();
        let g = gain(&mut l, 100.0, 0.05, 0.0);
        let want = db(0.5 * fir * fir);
        assert!(
            (db(g) - want).abs() < 0.05,
            "passband {} dB, want {}",
            db(g),
            want
        );
    }

    #[test]
    fn four_poles_above_the_cutoff() {
        let mut l = Ladder::new();
        l.frequency(200.0);
        l.resonance(0.0);
        // Four octaves above the corner: ~96 dB for 24 dB/oct, well past 60.
        let g = gain(&mut l, 3200.0, 0.05, 0.0);
        assert!(db(g) - db(0.5) < -60.0, "stopband {} dB", db(g));
    }

    #[test]
    fn resonance_lifts_the_corner() {
        let mut flat = Ladder::new();
        flat.frequency(1000.0);
        flat.resonance(0.0);
        let mut peaked = Ladder::new();
        peaked.frequency(1000.0);
        peaked.resonance(0.9);
        let (a, b) = (
            gain(&mut flat, 1000.0, 0.05, 0.0),
            gain(&mut peaked, 1000.0, 0.05, 0.0),
        );
        assert!(
            db(b) > db(a) + 6.0,
            "flat {} dB, peaked {} dB",
            db(a),
            db(b)
        );
    }

    #[test]
    fn one_unit_of_cv_is_the_octave_control() {
        // 500 Hz moved up one octave by the CV must be 1000 Hz exactly:
        // fast_exp2f(1.0) is exactly 2, so the two run the same arithmetic.
        let mut by_cv = Ladder::new();
        by_cv.frequency(500.0);
        by_cv.octave_control(1.0);
        let mut direct = Ladder::new();
        direct.frequency(1000.0);
        for i in 0..4000 {
            let x = 0.3 * libm::sinf(i as f32 * 0.07);
            assert_eq!(by_cv.process(x, 1.0), direct.process(x, 0.0));
        }
    }

    #[test]
    fn drive_above_one_is_scaled_by_the_passband_gain() {
        let mut l = Ladder::new();
        l.input_drive(3.0);
        assert_eq!(l.overdrive, 1.0 + 2.0 * 0.5);
        l.passband_gain(0.0);
        assert_eq!(l.overdrive, 3.0);
        l.input_drive(9.0);
        assert_eq!(l.overdrive, 4.0);
    }
}
