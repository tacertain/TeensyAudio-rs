//! One biquad section in `f32`, transposed direct form II, with the RBJ
//! cookbook's low-pass and high-pass designs: what the C++ library's
//! `AudioFilterBiquad` computes with `setLowpass()` / `setHighpass()`, in float.
//!
//! A per-sample kernel, not an [`AudioNode`](crate::node::AudioNode), for code
//! that runs its own loop over a block. Cascade sections for steeper slopes;
//! identical sections at [`Q_BUTTERWORTH`] are not a Butterworth of higher
//! order, so their -3 dB point sits below the corner.
//!
//! Designed at [`AUDIO_SAMPLE_RATE_EXACT`], the rate the Teensy's audio PLL
//! actually gives, as the C++ does.

use crate::constants::AUDIO_SAMPLE_RATE_EXACT;

/// The Q of a second-order Butterworth, 1/sqrt(2): maximally flat.
pub const Q_BUTTERWORTH: f32 = core::f32::consts::FRAC_1_SQRT_2;

/// One biquad section. See the module documentation.
#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    /// A section that passes its input unchanged.
    pub const fn passthrough() -> Self {
        Biquad {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn set(&mut self, b: [f32; 3], a: [f32; 3]) {
        let n = 1.0 / a[0];
        self.b0 = b[0] * n;
        self.b1 = b[1] * n;
        self.b2 = b[2] * n;
        self.a1 = a[1] * n;
        self.a2 = a[2] * n;
    }

    /// `cos w0` and `alpha`, with the corner held between 1 Hz and 0.49 of
    /// the sample rate.
    fn prewarp(hz: f32, q: f32) -> (f32, f32) {
        const FS: f32 = AUDIO_SAMPLE_RATE_EXACT;
        let w0 = 2.0 * core::f32::consts::PI * hz.clamp(1.0, 0.49 * FS) / FS;
        (libm::cosf(w0), libm::sinf(w0) / (2.0 * q))
    }

    /// A low-pass at `hz` with quality `q`. Keeps the state, so a corner
    /// can move while audio runs.
    pub fn set_lowpass(&mut self, hz: f32, q: f32) {
        let (c, alpha) = Self::prewarp(hz, q);
        let b = (1.0 - c) / 2.0;
        self.set([b, 1.0 - c, b], [1.0 + alpha, -2.0 * c, 1.0 - alpha]);
    }

    /// A high-pass at `hz` with quality `q`. Keeps the state, as
    /// [`set_lowpass`](Self::set_lowpass) does.
    pub fn set_highpass(&mut self, hz: f32, q: f32) {
        let (c, alpha) = Self::prewarp(hz, q);
        let b = (1.0 + c) / 2.0;
        self.set([b, -(1.0 + c), b], [1.0 + alpha, -2.0 * c, 1.0 - alpha]);
    }

    /// One sample in, one out.
    #[inline(always)]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

impl Default for Biquad {
    fn default() -> Self {
        Self::passthrough()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The RMS gain in dB for a sine of `hz`, after a settling half second.
    fn gain_db(bq: &mut Biquad, hz: f32) -> f32 {
        let fs = AUDIO_SAMPLE_RATE_EXACT;
        let settle = fs as usize / 2;
        let n = fs as usize / 2;
        let (mut sx, mut sy) = (0.0f64, 0.0f64);
        for i in 0..settle + n {
            let x = 0.5 * libm::sinf(2.0 * core::f32::consts::PI * hz * (i as f32) / fs);
            let y = bq.process(x);
            if i >= settle {
                sx += (x as f64) * (x as f64);
                sy += (y as f64) * (y as f64);
            }
        }
        10.0 * libm::log10(sy / sx) as f32
    }

    #[test]
    fn passthrough_is_identity() {
        let mut bq = Biquad::passthrough();
        for i in 0..1000 {
            let x = (i as f32 * 0.37).sin();
            assert_eq!(bq.process(x), x);
        }
    }

    #[test]
    fn lowpass_is_3_db_down_at_the_corner() {
        let mut bq = Biquad::passthrough();
        bq.set_lowpass(1000.0, Q_BUTTERWORTH);
        assert!(gain_db(&mut bq, 100.0).abs() < 0.05);
        assert!((gain_db(&mut bq, 1000.0) + 3.01).abs() < 0.05);
        // Two octaves above: 12 dB/oct, so about -24.
        let g = gain_db(&mut bq, 4000.0);
        assert!((-26.0..-22.0).contains(&g), "{} dB at 4 kHz", g);
    }

    #[test]
    fn highpass_is_3_db_down_at_the_corner() {
        let mut bq = Biquad::passthrough();
        bq.set_highpass(1000.0, Q_BUTTERWORTH);
        assert!(gain_db(&mut bq, 10000.0).abs() < 0.1);
        assert!((gain_db(&mut bq, 1000.0) + 3.01).abs() < 0.05);
        let g = gain_db(&mut bq, 250.0);
        assert!((-26.0..-22.0).contains(&g), "{} dB at 250 Hz", g);
    }

    #[test]
    fn highpass_removes_dc() {
        let mut bq = Biquad::passthrough();
        bq.set_highpass(50.0, Q_BUTTERWORTH);
        let mut y = 1.0;
        for _ in 0..44_100 {
            y = bq.process(0.5);
        }
        assert!(y.abs() < 1e-4, "{}", y);
    }

    #[test]
    fn a_new_corner_keeps_the_state() {
        // Moving the corner mid-signal must not reset the section: the output
        // right after the change continues from the old state instead of
        // restarting from zero.
        let mut moved = Biquad::passthrough();
        moved.set_lowpass(500.0, Q_BUTTERWORTH);
        for _ in 0..1000 {
            moved.process(0.5);
        }
        moved.set_lowpass(600.0, Q_BUTTERWORTH);
        let mut fresh = Biquad::passthrough();
        fresh.set_lowpass(600.0, Q_BUTTERWORTH);
        let (a, b) = (moved.process(0.5), fresh.process(0.5));
        assert!(a > 0.45 && b < 0.01, "moved {} fresh {}", a, b);
    }

    #[test]
    fn the_corner_is_clamped() {
        let mut hi = Biquad::passthrough();
        hi.set_lowpass(1.0e6, Q_BUTTERWORTH);
        let mut edge = Biquad::passthrough();
        edge.set_lowpass(0.49 * AUDIO_SAMPLE_RATE_EXACT, Q_BUTTERWORTH);
        for i in 0..100 {
            let x = (i as f32 * 0.9).sin();
            assert_eq!(hi.process(x), edge.process(x));
        }
    }
}
