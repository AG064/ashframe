//! The offline synthesiser.
//!
//! The original built its sounds with Web Audio in real time: a noise burst
//! through a swept filter, a tone with a pitch and gain envelope, and a handful
//! of those layered per sound effect. Godot has no equivalent graph to build, so
//! the same recipes are rendered into sample buffers once, at load.
//!
//! That is not a compromise so much as a simplification the original could not
//! make: a rendered buffer is a plain array of numbers, which means the sound
//! design is testable. The tests below assert the things that actually matter
//! about a sound — that it starts and ends silent, that it does not clip, that
//! the filter does not blow up — none of which can be checked by ear from a
//! screenshot.
//!
//! ## Determinism
//!
//! The original's noise came from `Math.random()`, so every launch of the game
//! sounded slightly different and no test could assert anything about it. The
//! generator here is seeded, so a rifle shot is the same rifle shot every run.

/// Sample rate for every rendered sound.
///
/// Low by modern standards and deliberately so: these are noise bursts and
/// simple tones with no content above 8 kHz, and halving the rate halves both
/// the load-time render and the memory the streams occupy.
pub const SAMPLE_RATE: u32 = 22_050;

/// A mono sample buffer in the range -1..1.
#[derive(Debug, Clone, Default)]
pub struct Buffer {
    pub samples: Vec<f32>,
}

impl Buffer {
    pub fn silence(seconds: f32) -> Self {
        let len = (seconds * SAMPLE_RATE as f32).ceil() as usize;
        Self {
            samples: vec![0.0; len],
        }
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Mix another buffer in, starting at `delay` seconds.
    pub fn mix_at(&mut self, other: &Buffer, delay: f32, gain: f32) {
        let offset = (delay * SAMPLE_RATE as f32).round() as usize;
        let needed = offset + other.len();
        if needed > self.samples.len() {
            self.samples.resize(needed, 0.0);
        }
        for (i, sample) in other.samples.iter().enumerate() {
            self.samples[offset + i] += sample * gain;
        }
    }

    /// Scale so the loudest sample sits at `peak`, if it is over.
    ///
    /// Sound design by ear produces buffers that clip when two layers land on
    /// the same sample. Rather than tuning each one by hand, the mix is
    /// normalised — which is also what stops a loud sound from being audibly
    /// crushed by the limiter in the mixer.
    pub fn normalise(&mut self, peak: f32) {
        let loudest = self.samples.iter().fold(0.0f32, |acc, s| acc.max(s.abs()));
        if loudest <= peak || loudest <= f32::EPSILON {
            return;
        }
        let scale = peak / loudest;
        for sample in &mut self.samples {
            *sample *= scale;
        }
    }

    /// Fade the first and last `seconds` in and out.
    ///
    /// A buffer that starts at full amplitude clicks, because the speaker cone
    /// is asked to jump from rest to a nonzero position in one sample.
    pub fn fade_edges(&mut self, seconds: f32) {
        let n = (seconds * SAMPLE_RATE as f32).round() as usize;
        let len = self.samples.len();
        if n == 0 || len < n * 2 {
            return;
        }
        for i in 0..n {
            let gain = i as f32 / n as f32;
            self.samples[i] *= gain;
            self.samples[len - 1 - i] *= gain;
        }
    }

    /// 16-bit little-endian PCM, which is what an `AudioStreamWAV` wants.
    pub fn to_pcm16(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.samples.len() * 2);
        for sample in &self.samples {
            let clamped = sample.clamp(-1.0, 1.0);
            let value = (clamped * 32_767.0).round() as i16;
            out.extend_from_slice(&value.to_le_bytes());
        }
        out
    }
}

/// A two-pole state-variable filter.
///
/// The same topology the original used through Web Audio's `BiquadFilterNode`,
/// and chosen for the same reason: one implementation gives lowpass, bandpass
/// and highpass, and its coefficients can be recomputed per sample, which is
/// what a swept filter needs.
#[derive(Debug, Clone, Copy, Default)]
pub struct Filter {
    low: f32,
    band: f32,
}

impl Filter {
    /// Process one sample. `cutoff` in hertz, `q` the resonance.
    ///
    /// Clamped so the filter cannot be driven unstable: the Chamberlin topology
    /// diverges once the coefficient approaches one, and a swept filter reaches
    /// that region at the top of its sweep if nothing stops it.
    pub fn process(&mut self, input: f32, cutoff: f32, q: f32) -> FilterOut {
        let nyquist = SAMPLE_RATE as f32 * 0.5;
        let cutoff = cutoff.clamp(30.0, nyquist * 0.45);
        let f = 2.0 * (std::f32::consts::PI * cutoff / SAMPLE_RATE as f32).sin();
        let damp = 1.0 / q.max(0.5);

        let high = input - self.low - damp * self.band;
        self.band += f * high;
        self.low += f * self.band;

        // A NaN anywhere would spread through the whole buffer, and a stream
        // full of NaNs is silence with extra steps. Cheaper to catch it here.
        if !self.low.is_finite() || !self.band.is_finite() {
            self.low = 0.0;
            self.band = 0.0;
        }
        if self.low.abs() > 4.0 || self.band.abs() > 4.0 {
            self.low = self.low.clamp(-4.0, 4.0);
            self.band = self.band.clamp(-4.0, 4.0);
        }

        FilterOut {
            low: self.low,
            band: self.band,
            high,
        }
    }
}

/// The three outputs of one [`Filter`] pass.
#[derive(Debug, Clone, Copy)]
pub struct FilterOut {
    pub low: f32,
    pub band: f32,
    /// The highpass output. Nothing in the set uses it, and it is kept because
    /// the topology computes it for free and a filter with two of its three
    /// outputs missing is a filter nobody can extend.
    #[allow(dead_code)]
    pub high: f32,
}

/// Which output of the filter to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Low,
    Band,
}

impl Shape {
    fn take(self, out: FilterOut) -> f32 {
        match self {
            Self::Low => out.low,
            Self::Band => out.band,
        }
    }
}

/// A seeded generator, so a rendered sound is identical on every run.
#[derive(Debug, Clone)]
pub struct Noise {
    state: u32,
}

impl Default for Noise {
    fn default() -> Self {
        Self::new(0x1234_5678)
    }
}

impl Noise {
    pub fn new(seed: u32) -> Self {
        Self { state: seed | 1 }
    }

    /// White noise in -1..1.
    pub fn next(&mut self) -> f32 {
        // xorshift32, the same generator the simulation uses for its spread.
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        (self.state as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// A noise burst through a filter swept from `from_hz` to `to_hz`.
///
/// The workhorse of the whole set: a shot, an impact and an explosion are the
/// same burst with different sweeps and lengths.
pub fn noise_burst(
    duration: f32,
    gain: f32,
    from_hz: f32,
    to_hz: f32,
    shape: Shape,
    q: f32,
) -> Buffer {
    let len = (duration * SAMPLE_RATE as f32).ceil() as usize;
    let mut out = Buffer {
        samples: Vec::with_capacity(len),
    };
    let mut filter = Filter::default();
    let mut noise = Noise::new(0x9e37_79b9);

    for i in 0..len {
        let t = i as f32 / len.max(1) as f32;
        // Exponential sweep, which is how a pitch is heard. A linear sweep from
        // 2600 Hz to 320 Hz spends most of its time in the top octave and
        // sounds like it barely moves.
        let cutoff = from_hz * (to_hz / from_hz).powf(t);
        let raw = noise.next();
        let filtered = shape.take(filter.process(raw, cutoff, q));
        // The envelope decays exponentially to a floor rather than to zero,
        // because an exponential that reaches zero has to spend the rest of its
        // life there.
        let envelope = (1.0 - t).powf(1.6);
        out.samples.push(filtered * envelope * gain);
    }
    out
}

/// A tone sweeping from `from_hz` to `to_hz` with a fast attack and decay.
pub fn tone(wave: Wave, from_hz: f32, to_hz: f32, duration: f32, gain: f32) -> Buffer {
    let len = (duration * SAMPLE_RATE as f32).ceil() as usize;
    let mut out = Buffer {
        samples: Vec::with_capacity(len),
    };
    let mut phase = 0.0f32;
    let attack = (0.012 * SAMPLE_RATE as f32).min(len as f32 * 0.2);

    for i in 0..len {
        let t = i as f32 / len.max(1) as f32;
        let freq = from_hz * (to_hz / from_hz).powf(t);

        // The original's envelope: near-instant attack, exponential decay to a
        // floor.
        let envelope = if (i as f32) < attack {
            let a = i as f32 / attack.max(1.0);
            0.0001 + (gain - 0.0001) * a
        } else {
            let d = (i as f32 - attack) / (len as f32 - attack).max(1.0);
            gain * (1.0 - d).powf(1.8)
        };

        phase += freq / SAMPLE_RATE as f32;
        phase -= phase.floor();
        out.samples.push(wave.at(phase) * envelope);
    }
    out
}

/// The waveforms the set uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wave {
    Sine,
    Square,
    Saw,
    Triangle,
}

impl Wave {
    fn at(self, phase: f32) -> f32 {
        match self {
            Self::Sine => (phase * std::f32::consts::TAU).sin(),
            Self::Square => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            Self::Saw => phase * 2.0 - 1.0,
            Self::Triangle => {
                if phase < 0.5 {
                    phase * 4.0 - 1.0
                } else {
                    3.0 - phase * 4.0
                }
            }
        }
    }
}

/// Fade the seam of a loop so it can be played end to end without a click.
pub fn make_loopable(buffer: &mut Buffer, fade: f32) {
    let n = (fade * SAMPLE_RATE as f32).round() as usize;
    let len = buffer.len();
    if n == 0 || len < n * 4 {
        return;
    }
    // Cross-fade the tail over the head rather than fading both to silence: a
    // loop that dips to nothing once a second is worse than a click.
    for i in 0..n {
        let t = i as f32 / n as f32;
        let head = buffer.samples[i];
        let tail = buffer.samples[len - n + i];
        buffer.samples[i] = head * t + tail * (1.0 - t);
    }
    buffer.samples.truncate(len - n);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_starts_loud_and_ends_quiet() {
        let burst = noise_burst(0.15, 0.5, 2400.0, 300.0, Shape::Band, 1.2);
        assert_eq!(burst.len(), (0.15 * SAMPLE_RATE as f32).ceil() as usize);
        let head: f32 = burst.samples[..64].iter().map(|s| s.abs()).sum();
        let tail: f32 = burst.samples[burst.len() - 64..]
            .iter()
            .map(|s| s.abs())
            .sum();
        assert!(head > tail * 4.0, "head {head} tail {tail}");
    }

    #[test]
    fn nothing_clips_after_normalising() {
        let mut mix = Buffer::silence(0.3);
        mix.mix_at(&tone(Wave::Saw, 200.0, 60.0, 0.3, 0.8), 0.0, 1.0);
        mix.mix_at(
            &noise_burst(0.3, 0.9, 2000.0, 200.0, Shape::Low, 0.8),
            0.0,
            1.0,
        );
        mix.normalise(0.9);
        let peak = mix.samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(peak <= 0.9 + 1e-4, "peak {peak}");
        assert!(peak > 0.5, "normalising should not silence the mix: {peak}");
    }

    #[test]
    fn every_sample_is_finite() {
        // A NaN in a stream is silence that costs full price, and it would
        // spread through the filter's state for the rest of the buffer.
        for shape in [Shape::Low, Shape::Band] {
            let burst = noise_burst(0.4, 0.7, 6000.0, 40.0, shape, 4.0);
            assert!(
                burst.samples.iter().all(|sample| sample.is_finite()),
                "{shape:?} produced a non-finite sample"
            );
        }
    }

    #[test]
    fn the_filter_settles_rather_than_diverging() {
        // The pathological case: a cutoff far above what the topology is stable
        // for. The clamp is what keeps this bounded.
        let mut filter = Filter::default();
        let mut peak = 0.0f32;
        for _ in 0..4000 {
            let out = filter.process(1.0, 90_000.0, 12.0);
            peak = peak.max(out.band.abs());
        }
        assert!(peak.is_finite() && peak < 100.0, "peak {peak}");
    }

    #[test]
    fn a_tone_starts_and_ends_near_silence() {
        let t = tone(Wave::Sine, 440.0, 440.0, 0.2, 0.5);
        assert!(t.samples[0].abs() < 0.01, "starts at {}", t.samples[0]);
        assert!(
            t.samples[t.len() - 1].abs() < 0.02,
            "ends at {}",
            t.samples[t.len() - 1]
        );
    }

    #[test]
    fn a_square_wave_alternates() {
        let t = tone(Wave::Square, 100.0, 100.0, 0.05, 1.0);
        let positive = t.samples.iter().filter(|s| **s > 0.0).count();
        let negative = t.samples.iter().filter(|s| **s < 0.0).count();
        assert!(positive > 0 && negative > 0);
        assert!((positive as f32 - negative as f32).abs() < positive as f32 * 0.2);
    }

    #[test]
    fn fading_edges_leaves_the_middle_alone() {
        let mut buffer = Buffer::silence(0.2);
        buffer.samples.fill(1.0);
        let middle = buffer.samples[buffer.len() / 2];
        buffer.fade_edges(0.01);
        assert_eq!(buffer.samples[0], 0.0);
        assert_eq!(buffer.samples[buffer.len() - 1], 0.0);
        assert_eq!(buffer.samples[buffer.len() / 2], middle);
    }

    #[test]
    fn a_loop_seam_is_continuous() {
        let mut looped = tone(Wave::Saw, 42.0, 42.0, 0.5, 0.8);
        let before = looped.len();
        make_loopable(&mut looped, 0.01);
        assert!(looped.len() < before, "the cross-fade consumes the tail");
        // The last sample must be close to the first, or the loop clicks.
        let (first, last) = (looped.samples[0], looped.samples[looped.len() - 1]);
        assert!(
            (first - last).abs() < 0.35,
            "seam jumps from {last} to {first}"
        );
    }

    #[test]
    fn pcm_is_little_endian_and_saturates() {
        let buffer = Buffer {
            samples: vec![1.0, -1.0, 2.0, -2.0],
        };
        let bytes = buffer.to_pcm16();
        assert_eq!(bytes.len(), 8);
        assert_eq!(i16::from_le_bytes([bytes[0], bytes[1]]), 32_767);
        assert_eq!(i16::from_le_bytes([bytes[2], bytes[3]]), -32_767);
        // Out-of-range input is clamped rather than wrapped: a wrapped sample is
        // a loud crack, which is the one thing a limiter must never do.
        assert_eq!(i16::from_le_bytes([bytes[4], bytes[5]]), 32_767);
        assert_eq!(i16::from_le_bytes([bytes[6], bytes[7]]), -32_767);
    }

    #[test]
    fn the_noise_generator_repeats_for_a_given_seed() {
        let mut a = Noise::new(7);
        let mut b = Noise::new(7);
        for _ in 0..100 {
            assert_eq!(a.next(), b.next());
        }
        let mut c = Noise::new(8);
        assert_ne!(a.next(), c.next(), "different seeds, different noise");
    }
}
