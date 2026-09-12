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
//! the filter does not blow up, that a machine gun is brighter than an
//! autocannon — none of which can be checked by ear from a screenshot.
//!
//! ## Layers
//!
//! The first pass at this set was one noise burst and one tone per sound, which
//! is why it sounded thin. A weapon is not one sound. It is a transient that
//! gives it an edge, a body that gives it weight, and a tail that gives it a
//! room to happen in, and the three are summed with different delays and
//! different decay rates. The recipes at the bottom of this file are mostly
//! that observation applied over and over:
//!
//! - **Transient** — a few milliseconds of bright noise swept downwards. A
//!   gunshot's leading edge is a shock, and a shock's "pitch" falls.
//! - **Body** — a low sine dropping in pitch. This is the layer that carries
//!   weight; without it a shot is a firework.
//! - **Ring** — inharmonic partials with independent decay rates, for anything
//!   metal. A struck plate rings at 1, 1.59, 2.14, 2.30, 2.92 … times its
//!   fundamental, and the high modes die first, which is why a clang is not a
//!   note.
//! - **Tail** — filtered noise, quieter and longer than the rest, which is what
//!   the world does to a loud short sound.
//!
//! ## Sample rate
//!
//! 44.1 kHz, up from the 22 050 the first pass used, and the reason is a
//! property of the filter: the state-variable filter clamps its cutoff to 45 %
//! of Nyquist, so at 22 050 Hz the brightest a swept layer could be was 4.9 kHz.
//! A crack, a machine-gun transient and a struck plate all live above that, so
//! every sound in the set was dulled by a ceiling rather than by its recipe.
//! 44.1 kHz is also Godot's mix rate, so a stream rendered here is played
//! without resampling.
//!
//! ## Cost
//!
//! Every sound in the set is rendered once, before the first frame. At 44.1 kHz
//! the twenty seconds of mono audio the game holds for the rest of the session
//! is about 1.7 MB as 16-bit PCM, and rendering it costs a quarter of a second
//! of load time in a release build and most of a second in a debug one — which
//! is the reason the rate is not higher than it is. Three tests hold that line:
//! `the_whole_set_fits_the_budget` and
//! `the_set_renders_in_the_time_a_load_can_afford` here, and
//! `audio::tests::the_set_is_affordable_to_render_at_load` over the take list as
//! the game actually renders it, which is longer because of the second takes.
//!
//! ## Determinism
//!
//! The original's noise came from `Math.random()`, so every launch of the game
//! sounded slightly different and no test could assert anything about it. The
//! generator here is seeded, so a rifle shot is the same rifle shot every run.
//! Sounds that would otherwise repeat — a burst of fire, a foot, an impact —
//! are rendered as several takes with different seeds and alternated at play
//! time, which is variation that is designed rather than variation that is
//! random.

use std::f32::consts::TAU;

/// Sample rate for every rendered sound.
pub const SAMPLE_RATE: u32 = 44_100;

/// The level a one-shot is normalised to.
///
/// Just under full scale, because a stream that peaks at 1.0 arrives at the
/// mixer with nothing left for the sum of several of them.
const PEAK_ONE_SHOT: f32 = 0.88;

/// The level a sustained loop is normalised to. Quieter than a one-shot on
/// purpose: the engine plays under everything else for the entire mission.
const PEAK_LOOP: f32 = 0.62;

/// The level a HUD sound is normalised to.
///
/// A confirmation tone at the level of an explosion is a confirmation tone the
/// player turns the game down to avoid, and then cannot hear the explosion.
const PEAK_UI: f32 = 0.5;

/// Seconds of audio that `seconds` is worth, rounded up.
fn samples(seconds: f32) -> usize {
    (seconds * SAMPLE_RATE as f32).ceil().max(0.0) as usize
}

/// A mono sample buffer in the range -1..1.
#[derive(Debug, Clone, Default)]
pub struct Buffer {
    pub samples: Vec<f32>,
}

impl Buffer {
    pub fn silence(seconds: f32) -> Self {
        Self {
            samples: vec![0.0; samples(seconds)],
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

    /// The loudest sample, as a positive number.
    pub fn peak(&self) -> f32 {
        self.samples.iter().fold(0.0f32, |acc, s| acc.max(s.abs()))
    }

    /// Scale so the loudest sample sits at `peak`, if it is over.
    ///
    /// Sound design by ear produces buffers that clip when two layers land on
    /// the same sample. Rather than tuning each one by hand, the mix is
    /// normalised — which is also what stops a loud sound from being audibly
    /// crushed by the limiter in the mixer.
    pub fn normalise(&mut self, peak: f32) {
        let loudest = self.peak();
        if loudest <= peak || loudest <= f32::EPSILON {
            return;
        }
        self.scale(peak / loudest);
    }

    /// Scale so the loudest sample sits at `peak`, in either direction.
    ///
    /// The difference from [`Buffer::normalise`] is the direction. A finished
    /// sound has to arrive at the mixer at a known level: a weapon that happens
    /// to sum to 0.3 would be heard as a quieter weapon rather than as the same
    /// weapon, and the loudest sound in the set would then set the volume for
    /// all of them.
    pub fn normalise_exact(&mut self, peak: f32) {
        let loudest = self.peak();
        if loudest <= f32::EPSILON {
            return;
        }
        self.scale(peak / loudest);
    }

    fn scale(&mut self, factor: f32) {
        for sample in &mut self.samples {
            *sample *= factor;
        }
    }

    /// Subtract the mean.
    ///
    /// A noise burst through a lowpass comes out with an offset, and an offset
    /// is headroom spent on a frequency nobody can hear.
    pub fn remove_dc(&mut self) {
        if self.samples.is_empty() {
            return;
        }
        let mean = self.samples.iter().sum::<f32>() / self.samples.len() as f32;
        for sample in &mut self.samples {
            *sample -= mean;
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

/// A one-shot amplitude envelope: a rise to a peak, and a fall away from it.
///
/// Exponential in the sense that matters — the shape a decaying physical object
/// has — while still being evaluated against a length, because a sound has to
/// end. The curve is a single exponent rather than a rate so that a recipe can
/// say "this falls away fast" without also saying how long it lives.
#[derive(Debug, Clone, Copy)]
pub struct Envelope {
    pub duration: f32,
    /// Seconds from the start to the peak. Never zero: a sound that starts at
    /// full amplitude is asking the speaker cone to jump.
    pub attack: f32,
    /// Exponent of the decay after the peak. 1 is linear, above 1 falls away
    /// sooner, which is what a struck object does.
    pub curve: f32,
    /// Exponent of the rise before the peak. 1 is a straight ramp; above 1
    /// holds the sound back until late, which is what a whoosh does.
    pub rise_curve: f32,
}

impl Envelope {
    pub fn new(duration: f32, attack: f32, curve: f32) -> Self {
        // Clamped so that a very short sound cannot have an attack longer than
        // itself, and so that the two are never equal — the decay is a division
        // by the distance between them.
        let shortest = 2.0 / SAMPLE_RATE as f32;
        let duration = duration.max(shortest * 2.0);
        Self {
            duration,
            attack: attack.clamp(shortest, duration - shortest),
            curve,
            rise_curve: 1.0,
        }
    }

    /// An envelope that peaks partway through: the shape of a sound going past.
    pub fn bell(duration: f32, peak_at: f32, curve: f32) -> Self {
        let mut env = Self::new(duration, duration * peak_at.clamp(0.05, 0.95), curve);
        env.rise_curve = 2.0;
        env
    }

    pub fn gain(&self, t: f32) -> f32 {
        if t <= 0.0 || t >= self.duration {
            return 0.0;
        }
        if t < self.attack {
            (t / self.attack).powf(self.rise_curve)
        } else {
            let fallen = (t - self.attack) / (self.duration - self.attack);
            (1.0 - fallen).powf(self.curve)
        }
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
    /// The highpass output. Kept because the topology computes it for free and
    /// a filter with two of its three outputs missing is a filter nobody can
    /// extend.
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

    /// White noise in 0..1, for the recipes that need a decision rather than a
    /// sample: where a fragment lands, how big it is.
    pub fn unit(&mut self) -> f32 {
        (self.next() + 1.0) * 0.5
    }
}

/// Spread a recipe's seed before it reaches the generator.
///
/// [`Noise::new`] forces the low bit of its state, because an all-zero state is
/// a generator that returns nothing but zeros for ever. The cost is that two
/// seeds which differ only in that bit — 8 and 9 — draw *identical* noise, and
/// a take is supposed to be a different performance of the same sound. Passing
/// consecutive seeds is the most natural thing for a caller to do, so the seeds
/// are whitened here rather than trusted. The mixing is a bijection, so two
/// different seeds still give two different seeds.
fn mixed(seed: u32) -> u32 {
    let mut z = seed.wrapping_add(0x9e37_79b9);
    z = (z ^ (z >> 16)).wrapping_mul(0x85eb_ca6b);
    z = (z ^ (z >> 13)).wrapping_mul(0xc2b2_ae35);
    z ^ (z >> 16)
}

/// A filtered noise layer: the workhorse of the whole set.
///
/// A shot, an impact and an explosion are the same burst with different sweeps,
/// lengths and envelope shapes, so the differences are fields rather than
/// arguments — a five-argument call repeated twenty times is a recipe nobody
/// can read.
#[derive(Debug, Clone, Copy)]
pub struct NoiseSpec {
    pub duration: f32,
    pub gain: f32,
    pub from_hz: f32,
    pub to_hz: f32,
    pub shape: Shape,
    /// Filter resonance. Above 2 the burst starts to have a pitch of its own,
    /// which is what a narrow band around a strike frequency is for.
    pub q: f32,
    pub seed: u32,
    /// Envelope attack in seconds, ignored when `peak` is set.
    pub attack: f32,
    /// Envelope decay exponent.
    pub curve: f32,
    /// Where the envelope peaks, as a fraction of the duration. Zero means a
    /// plain attack-then-decay; a half is a swell that arrives and passes.
    pub peak: f32,
}

impl Default for NoiseSpec {
    fn default() -> Self {
        Self {
            duration: 0.2,
            gain: 0.5,
            from_hz: 2000.0,
            to_hz: 500.0,
            shape: Shape::Band,
            q: 1.0,
            seed: 0x9e37_79b9,
            attack: 0.0008,
            curve: 2.0,
            peak: 0.0,
        }
    }
}

impl NoiseSpec {
    /// A burst of `duration` with the defaults for everything else.
    pub fn new(duration: f32) -> Self {
        Self {
            duration,
            ..Self::default()
        }
    }

    fn envelope(&self) -> Envelope {
        if self.peak > 0.0 {
            Envelope::bell(self.duration, self.peak, self.curve)
        } else {
            Envelope::new(self.duration, self.attack, self.curve)
        }
    }
}

/// A noise burst through a filter swept from `from_hz` to `to_hz`.
///
/// The sweep is exponential, which is how a pitch is heard: a linear sweep from
/// 2600 Hz to 320 Hz spends most of its time in the top octave and sounds like
/// it barely moves.
pub fn noise_layer(spec: NoiseSpec) -> Buffer {
    let len = samples(spec.duration);
    let envelope = spec.envelope();
    let ratio = (spec.to_hz / spec.from_hz).max(1e-4);
    let mut out = Buffer {
        samples: Vec::with_capacity(len),
    };
    let mut filter = Filter::default();
    let mut noise = Noise::new(spec.seed);

    for i in 0..len {
        let t = i as f32 / SAMPLE_RATE as f32;
        let progress = i as f32 / len.max(1) as f32;
        let cutoff = spec.from_hz * ratio.powf(progress);
        let raw = noise.next();
        let filtered = spec.shape.take(filter.process(raw, cutoff, spec.q));
        out.samples.push(filtered * envelope.gain(t) * spec.gain);
    }
    out
}

/// A burst with an attack-decay envelope and no other opinions, kept because
/// most layers are exactly this.
pub fn noise_burst(
    duration: f32,
    gain: f32,
    from_hz: f32,
    to_hz: f32,
    shape: Shape,
    q: f32,
) -> Buffer {
    noise_layer(NoiseSpec {
        duration,
        gain,
        from_hz,
        to_hz,
        shape,
        q,
        curve: 1.6,
        ..NoiseSpec::new(duration)
    })
}

/// A tone sweeping from `from_hz` to `to_hz` with a fast attack and decay.
pub fn tone(wave: Wave, from_hz: f32, to_hz: f32, duration: f32, gain: f32) -> Buffer {
    let len = samples(duration);
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

/// A low sine dropping in pitch: the body layer of an impact.
///
/// The drop is what makes a thud read as a weight arriving rather than as a
/// bass note, and the second harmonic is what stops it sounding like a test
/// tone. It is not loud enough to add a zero crossing, so the pitch of the
/// result is still the pitch of the fundamental.
pub fn thump(from_hz: f32, to_hz: f32, duration: f32, gain: f32, curve: f32) -> Buffer {
    let len = samples(duration);
    let envelope = Envelope::new(duration, 0.0006, curve);
    let ratio = (to_hz / from_hz).max(1e-4);
    let mut out = Buffer {
        samples: Vec::with_capacity(len),
    };
    let mut phase = 0.0f32;

    for i in 0..len {
        let t = i as f32 / SAMPLE_RATE as f32;
        let progress = i as f32 / len.max(1) as f32;
        let freq = from_hz * ratio.powf(progress);
        let body = (phase * TAU).sin() + 0.3 * (phase * TAU * 2.0).sin();
        // The phase advances after the sample rather than before it, so the
        // first sample of the layer is exactly zero and the mix starts from
        // rest without needing a fade to hide a step.
        phase = (phase + freq / SAMPLE_RATE as f32).fract();
        out.samples.push(body * envelope.gain(t) * gain);
    }
    out
}

/// One mode of a struck object.
#[derive(Debug, Clone, Copy)]
pub struct Partial {
    /// Frequency as a multiple of the object's fundamental. A plate's modes are
    /// inharmonic — 1, 1.59, 2.14, 2.30, 2.92 … — which is the whole reason a
    /// clang does not sound like a note.
    pub ratio: f32,
    pub gain: f32,
    /// Seconds for this mode to fall by 60 dB.
    pub decay: f32,
}

impl Partial {
    pub const fn new(ratio: f32, gain: f32, decay: f32) -> Self {
        Self { ratio, gain, decay }
    }
}

/// Sum inharmonic partials with independent decay rates: struck metal.
///
/// Each mode is a sine with its own -60 dB time, so the timbre of the ring
/// changes as it decays — bright at the strike, hollow afterwards. That change
/// is most of what the ear uses to tell a struck object from a filtered noise
/// burst, and it cannot be faked with one envelope on the sum.
pub fn struck(duration: f32, fundamental: f32, modes: &[Partial]) -> Buffer {
    let mut out = Buffer::silence(duration);
    let ceiling = SAMPLE_RATE as f32 * 0.45;

    for mode in modes {
        let hz = fundamental * mode.ratio;
        // A mode at or above Nyquist is not a mode, it is an alias.
        if hz >= ceiling || mode.gain <= 0.0 {
            continue;
        }
        let step = hz / SAMPLE_RATE as f32;
        let tau = (mode.decay / 6.907_755).max(0.002);
        let mut phase = 0.0f32;
        for (i, sample) in out.samples.iter_mut().enumerate() {
            let t = i as f32 / SAMPLE_RATE as f32;
            // Phase advances after the sample: every mode starts at zero, so a
            // sum of nine of them has no step at the front of it either.
            *sample += (phase * TAU).sin() * mode.gain * (-t / tau).exp();
            phase = (phase + step).fract();
        }
    }
    out
}

/// The modes of a struck steel plate, as `(ratio, level, decay)`.
///
/// The ratios are the ideal circular-plate modes. The decay column is relative:
/// a caller scales it by how long the thing rings, and the fact that it falls
/// from 1.0 to 0.1 is why a big clang goes hollow as it fades.
const PLATE_MODES: [(f32, f32, f32); 9] = [
    (1.00, 0.55, 1.00),
    (1.59, 0.40, 0.72),
    (2.14, 0.30, 0.52),
    (2.30, 0.25, 0.43),
    (2.92, 0.19, 0.33),
    (3.60, 0.14, 0.25),
    (4.06, 0.11, 0.19),
    (4.69, 0.08, 0.14),
    (6.20, 0.05, 0.09),
];

/// A struck plate: the ring of a hard surface, sized by its fundamental.
fn plate(duration: f32, fundamental: f32, ring: f32, gain: f32) -> Buffer {
    let modes: Vec<Partial> = PLATE_MODES
        .iter()
        .map(|(ratio, level, decay)| Partial::new(*ratio, level * gain, decay * ring))
        .collect();
    struck(duration, fundamental, &modes)
}

/// A feedback comb, in place.
///
/// A short delay with feedback puts a resonance at 1/delay, which is what a
/// steel receiver does to the crack of a round leaving it, and what a bulkhead
/// does to a hit on it. The feedback is capped below one because a comb that
/// reaches unity never stops ringing and a buffer that never decays is a buffer
/// that is still going when the game closes.
pub fn comb(buffer: &mut Buffer, delay: f32, feedback: f32, mix: f32) {
    let len = buffer.len();
    if len == 0 {
        return;
    }
    let d = ((delay * SAMPLE_RATE as f32).round() as usize).clamp(1, len);
    let feedback = feedback.clamp(0.0, 0.95);
    let mix = mix.clamp(0.0, 1.0);
    let mut line = vec![0.0f32; d];

    for (i, sample) in buffer.samples.iter_mut().enumerate() {
        let dry = *sample;
        let slot = i % d;
        let wet = dry + feedback * line[slot];
        line[slot] = wet;
        *sample = dry * (1.0 - mix) + wet * mix;
    }
}

/// A comb whose delay moves across the buffer: motion.
///
/// A static comb is a resonance; a comb whose delay lengthens is a resonance
/// falling in pitch, which the ear hears as a source going away. It is the
/// cheapest way to make a whoosh travel, and it costs one interpolated read per
/// sample.
pub fn comb_sweep(buffer: &mut Buffer, from_delay: f32, to_delay: f32, feedback: f32, mix: f32) {
    let len = buffer.len();
    if len == 0 {
        return;
    }
    let shortest = (from_delay.min(to_delay) * SAMPLE_RATE as f32).max(2.0);
    let longest = (from_delay.max(to_delay) * SAMPLE_RATE as f32).max(shortest);
    let capacity = longest.ceil() as usize + 4;
    let ratio = (to_delay / from_delay).max(1e-4);
    let feedback = feedback.clamp(0.0, 0.95);
    let mix = mix.clamp(0.0, 1.0);

    let mut line = vec![0.0f32; capacity];
    let mut write = 0usize;
    for (i, sample) in buffer.samples.iter_mut().enumerate() {
        let progress = i as f32 / len.max(1) as f32;
        let delay = (from_delay * ratio.powf(progress) * SAMPLE_RATE as f32).max(shortest);
        // Read `delay` samples back from the write head, interpolated: an
        // integer delay that steps by one sample per sample is a comb whose
        // pitch jumps in audible increments.
        let position = write as f32 + capacity as f32 - delay;
        let base = position.floor();
        let frac = position - base;
        let first = (base as usize) % capacity;
        let second = (first + 1) % capacity;
        let delayed = line[first] * (1.0 - frac) + line[second] * frac;

        let dry = *sample;
        let wet = dry + feedback * delayed;
        line[write] = wet;
        write = (write + 1) % capacity;
        *sample = dry * (1.0 - mix) + wet * mix;
    }
}

/// A scatter of small hard ticks: fragments coming down after an explosion, or
/// grit thrown up by a foot.
///
/// The pieces land in order of size — the big ones first, the dust last — so
/// the scatter is built from a schedule rather than from a burst of noise, and
/// it decays over its whole length instead of stopping. `centre_hz` and
/// `spread` set how bright the pieces are: an explosion throws things that ring
/// across two octaves, and a foot throws grit.
pub fn debris(
    duration: f32,
    gain: f32,
    count: usize,
    centre_hz: f32,
    spread: f32,
    seed: u32,
) -> Buffer {
    let mut out = Buffer::silence(duration);
    if count == 0 {
        return out;
    }
    let mut rng = Noise::new(mixed(seed));
    let span = duration * 0.82;

    for i in 0..count {
        let progress = if count > 1 {
            i as f32 / (count - 1) as f32
        } else {
            0.0
        };
        let at = span * progress + rng.unit() * duration * 0.06;
        let level = gain * (1.0 - 0.72 * progress) * (0.55 + 0.45 * rng.unit());
        let centre = centre_hz * (1.0 - spread * 0.5 + spread * rng.unit());
        let tick = noise_layer(NoiseSpec {
            duration: 0.006 + 0.014 * rng.unit(),
            gain: level,
            from_hz: centre * 1.8,
            to_hz: centre * 0.6,
            shape: Shape::Band,
            q: 1.8,
            seed: 1 + (rng.unit() * 900_000.0) as u32,
            attack: 0.0004,
            curve: 2.6,
            peak: 0.0,
        });
        out.mix_at(&tick, at, 1.0);
    }
    out
}

/// A small motor taking a load: geared, detuned and hunting for its position.
///
/// One tone is an oscillator; three detuned tones with a tremolo on the pitch
/// are a mechanism. The band above them is the gear teeth.
pub fn servo(duration: f32, gain: f32, from_hz: f32, to_hz: f32, seed: u32) -> Buffer {
    let len = samples(duration);
    let envelope = Envelope::new(duration, 0.004, 1.8);
    let ratio = (to_hz / from_hz).max(1e-4);
    let mut whine = Buffer {
        samples: Vec::with_capacity(len),
    };
    let voices = [(0.0, 1.0), (7.0, 0.7), (-5.0, 0.55)];
    let mut phases = [0.0f32; 3];

    for i in 0..len {
        let t = i as f32 / SAMPLE_RATE as f32;
        let progress = i as f32 / len.max(1) as f32;
        let hz = from_hz * ratio.powf(progress);
        let wobble = 1.0 + 0.02 * (TAU * 26.0 * t).sin();
        let mut sum = 0.0;
        for (voice, (detune, level)) in voices.iter().enumerate() {
            let step = (hz + detune) * wobble / SAMPLE_RATE as f32;
            phases[voice] = (phases[voice] + step).fract();
            sum += Wave::Triangle.at(phases[voice]) * level;
        }
        whine.samples.push(sum * envelope.gain(t) * gain * 0.5);
    }

    let teeth = noise_layer(NoiseSpec {
        duration,
        gain: gain * 0.5,
        from_hz: from_hz * 1.8,
        to_hz: to_hz * 1.6,
        shape: Shape::Band,
        q: 3.0,
        seed: mixed(seed),
        attack: 0.004,
        curve: 1.6,
        peak: 0.0,
    });
    layered(duration, &[(whine, 0.0, 1.0), (teeth, 0.0, 0.5)])
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
            Self::Sine => (phase * TAU).sin(),
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

/// Sum layers into one buffer: `(buffer, delay_seconds, gain)`.
///
/// The duration is a floor rather than a cap — a layer that runs past it makes
/// the mix longer, because silently truncating a tail is how a sound loses its
/// ending. Every recipe here is finished afterwards, which is what fades the
/// edges whatever the length turns out to be.
pub fn layered(duration: f32, layers: &[(Buffer, f32, f32)]) -> Buffer {
    let mut out = Buffer::silence(duration);
    for (layer, delay, gain) in layers {
        out.mix_at(layer, *delay, *gain);
    }
    out
}

/// Close the seam of a loop, for a source that runs `fade` seconds past its end.
///
/// The tail is cross-faded over the head rather than both being faded to
/// silence: a loop that dips to nothing once a second is worse than a click.
///
/// How the two are blended depends on how much they have in common, because the
/// two cases want opposite curves. Two unrelated halves of a noise bed summed
/// with weights that add to one are 3 dB down in the middle of the fade, which
/// is heard as a flutter once a second for the whole mission — they want an
/// equal-*power* blend. Two halves of a tone whose period divides the loop are
/// the same signal, and an equal-power blend of those is 3 dB *up* in the middle
/// — they want an equal-gain blend. The correlation between the two is one
/// number that says which of the two this is, so it is measured and the blend is
/// corrected by it.
///
/// The result is `fade` seconds shorter than the source. Anything meant to
/// survive the seam exactly — a tone that divides the loop length — has to be
/// added after this, not before.
pub fn make_loopable(buffer: &mut Buffer, fade: f32) {
    let n = (fade * SAMPLE_RATE as f32).round() as usize;
    let len = buffer.len();
    if n == 0 || len < n * 4 {
        return;
    }

    let (mut dot, mut head_energy, mut tail_energy) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..n {
        let head = buffer.samples[i] as f64;
        let tail = buffer.samples[len - n + i] as f64;
        dot += head * tail;
        head_energy += head * head;
        tail_energy += tail * tail;
    }
    let correlation = if head_energy <= 1e-18 || tail_energy <= 1e-18 {
        0.0
    } else {
        (dot / (head_energy * tail_energy).sqrt()) as f32
    };

    for i in 0..n {
        let t = i as f32 / n as f32;
        let (head_gain, tail_gain) = (t, 1.0 - t);
        // The power the blend leaves behind, and the factor that puts it back
        // to one. Clamped so that a pair that cancels completely cannot be
        // corrected into a spike: the samples being corrected are near zero
        // anyway, and a fade that doubles is the most that is ever wanted.
        let power = head_gain * head_gain
            + tail_gain * tail_gain
            + 2.0 * head_gain * tail_gain * correlation;
        let fix = 1.0 / power.max(0.25).sqrt();
        buffer.samples[i] =
            (buffer.samples[i] * head_gain + buffer.samples[len - n + i] * tail_gain) * fix;
    }
    buffer.samples.truncate(len - n);
}

/// The last thing every one-shot passes through.
///
/// The offset goes first, because removing it after fading would put a step
/// back; then a fade over the first and last millisecond, which is short enough
/// to leave a crack sharp and long enough that the very first sample cannot be
/// a step; and the level last, so that the number the level is measured at is
/// the number the mixer sees. Fading after normalising instead would quietly
/// cost a sound whose peak is in its first millisecond — which is every
/// percussive sound in the set — a tenth of its level.
fn finish(mut buffer: Buffer, peak: f32) -> Buffer {
    buffer.remove_dc();
    buffer.fade_edges(0.001);
    buffer.normalise_exact(peak);
    buffer
}

// ---------------------------------------------------------------------------
// The sustained loops
// ---------------------------------------------------------------------------

/// Length of the sustained loops, in seconds.
///
/// One second exactly, so every frequency that is a whole number of hertz is an
/// exact multiple of the loop's fundamental of 1 Hz. That is what lets the
/// tonal layer close on the sample instead of almost closing on it.
const LOOP_SECONDS: f32 = 1.0;
/// The cross-fade that closes a loop, in seconds.
const LOOP_FADE: f32 = 0.02;

/// A stack of sine partials whose frequencies are snapped to the loop's grid.
///
/// The snapping is the point. A partial at 38.4 Hz in a one-second loop arrives
/// at the seam half a cycle out of phase, and half a cycle is a click in the
/// middle of a note that is supposed to be continuous. Each partial also gets a
/// random starting phase, which is what stops the stack sounding like a sawtooth
/// with the same shape every cycle.
fn harmonic_stack(
    len: usize,
    fundamental: f32,
    harmonics: usize,
    roll_off: f32,
    detune: f32,
    seed: u32,
) -> Buffer {
    let bin = SAMPLE_RATE as f32 / len.max(1) as f32;
    let snap = |hz: f32| (hz / bin).round().max(1.0) * bin;
    let mut rng = Noise::new(seed);
    let mut voices: Vec<(f32, f32, f32)> = Vec::with_capacity(harmonics * 2);

    for h in 1..=harmonics {
        let level = 1.0 / (h as f32).powf(roll_off);
        let hz = fundamental * h as f32;
        voices.push((snap(hz) / SAMPLE_RATE as f32, level, rng.unit()));
        if detune > 0.0 {
            // A partner a hair away, which beats against the first: one
            // frequency is a test tone, two is a machine.
            voices.push((
                snap(hz + detune) / SAMPLE_RATE as f32,
                level * 0.55,
                rng.unit(),
            ));
        }
    }

    let mut out = Buffer {
        samples: vec![0.0; len],
    };
    for sample in out.samples.iter_mut() {
        let mut sum = 0.0;
        for (step, level, phase) in voices.iter_mut() {
            *phase = (*phase + *step).fract();
            sum += (*phase * TAU).sin() * *level;
        }
        *sample = sum;
    }
    out
}

/// Steady filtered noise with no envelope: a bed to sit under a loop.
///
/// It carries no envelope on purpose. A bed that swells has to swell across the
/// seam as well, and the seam is exactly where a swell is most obvious, so the
/// swell belongs to the caller — where it can be counted in whole cycles.
fn noise_bed(len: usize, gain: f32, cutoff: f32, shape: Shape, q: f32, seed: u32) -> Buffer {
    let mut filter = Filter::default();
    let mut noise = Noise::new(seed);
    let mut out = Buffer {
        samples: Vec::with_capacity(len),
    };
    for _ in 0..len {
        let filtered = shape.take(filter.process(noise.next(), cutoff, q));
        out.samples.push(filtered * gain);
    }
    out
}

/// A band of noise whose cutoff moves up and down, for a hiss that breathes.
fn moving_noise(
    len: usize,
    gain: f32,
    centre_hz: f32,
    spread: f32,
    cycles: f32,
    q: f32,
    seed: u32,
) -> Buffer {
    let mut filter = Filter::default();
    let mut noise = Noise::new(seed);
    let mut out = Buffer {
        samples: Vec::with_capacity(len),
    };
    for i in 0..len {
        let t = i as f32 / len.max(1) as f32;
        let cutoff = centre_hz * (1.0 + spread * (TAU * cycles * t).sin());
        let filtered = Shape::Band.take(filter.process(noise.next(), cutoff, q));
        out.samples.push(filtered * gain);
    }
    out
}

/// Join a noise part and a tonal part into one seamless loop.
///
/// The two need opposite treatment at the seam. Noise has to be cross-faded:
/// there is no reason for the end of a random signal to meet its beginning. A
/// tone must not be, because blending two points of a waveform that are out of
/// phase partly cancels it. So the noise is closed first — its source is
/// `LOOP_FADE` longer than the loop, which is why the caller renders it that way
/// — and the tone, whose every frequency divides the loop length, is added
/// afterwards and simply continues.
fn sustained(noise: Buffer, tone: Buffer) -> Buffer {
    let mut bed = noise;
    make_loopable(&mut bed, LOOP_FADE);
    let mut out = layered(LOOP_SECONDS, &[(bed, 0.0, 1.0), (tone, 0.0, 1.0)]);
    // `normalise` rather than `normalise_exact`: a sustained layer's level is a
    // ceiling, not a target, because the movement code mixes underneath it every
    // step and lifting a quiet loop would only mean doing the same job twice.
    out.normalise(PEAK_LOOP);
    out
}

/// The length of a noise layer that is going to be closed into `LOOP_SECONDS`.
fn loop_source_len() -> usize {
    samples(LOOP_SECONDS) + (LOOP_FADE * SAMPLE_RATE as f32).round() as usize
}

/// The engine at rest: a large machine turning over, with air around it.
pub fn engine_bed() -> Buffer {
    let source = loop_source_len();
    let len = samples(LOOP_SECONDS);

    let mut drum = noise_bed(source, 0.5, 240.0, Shape::Low, 0.8, 0x1002);
    let mut whine = moving_noise(source, 0.10, 1_450.0, 0.22, 2.0, 2.5, 0x1003);
    for (i, sample) in drum.samples.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        *sample *= 0.78 + 0.22 * (TAU * 3.0 * t).sin();
    }
    for (i, sample) in whine.samples.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        *sample *= 0.7 + 0.3 * (TAU * 5.0 * t).sin().max(0.0);
    }

    let mut body = harmonic_stack(len, 38.0, 16, 1.15, 1.0, 0x1001);
    for (i, sample) in body.samples.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        // The lope: a large engine does not turn smoothly, and the unevenness
        // is most of what makes it sound like it is working rather than humming.
        let lope = 0.66 + 0.34 * (TAU * 6.0 * t).sin().max(0.0).powf(0.5);
        *sample *= lope * (1.0 + 0.06 * (TAU * 9.0 * t).sin());
    }

    sustained(
        layered(LOOP_SECONDS, &[(drum, 0.0, 1.0), (whine, 0.0, 1.0)]),
        body,
    )
}

/// The engine under load: the layer that only arrives when the mech is moving.
pub fn engine_load() -> Buffer {
    let source = loop_source_len();
    let len = samples(LOOP_SECONDS);

    let mut strain = noise_bed(source, 0.55, 620.0, Shape::Low, 1.4, 0x2002);
    let mut whine = moving_noise(source, 0.5, 2_400.0, 0.35, 3.0, 5.0, 0x2003);
    for (i, sample) in strain.samples.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        *sample *= 0.72 + 0.28 * (TAU * 9.0 * t).sin().max(0.0);
    }
    for (i, sample) in whine.samples.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        *sample *= 0.6 + 0.4 * (TAU * 4.0 * t).sin().max(0.0);
    }

    let mut body = harmonic_stack(len, 57.0, 12, 1.3, 1.0, 0x2001);
    for (i, sample) in body.samples.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        *sample *= 0.6 + 0.4 * (TAU * 9.0 * t).sin().max(0.0).powf(0.5);
    }

    sustained(
        layered(LOOP_SECONDS, &[(strain, 0.0, 1.0), (whine, 0.0, 1.0)]),
        body,
    )
}

/// The thrusters at full: a roar with a hiss over it.
pub fn thruster_roar() -> Buffer {
    let source = loop_source_len();
    let len = samples(LOOP_SECONDS);

    let mut roar = noise_bed(source, 0.9, 420.0, Shape::Low, 1.1, 0x3002);
    let mut grit = noise_bed(source, 0.35, 1_100.0, Shape::Band, 0.9, 0x3003);
    for (i, sample) in roar.samples.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        *sample *= 0.82 + 0.18 * (TAU * 5.0 * t).sin();
    }
    for (i, sample) in grit.samples.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        *sample *= 0.6 + 0.4 * (TAU * 7.0 * t).sin().max(0.0);
    }

    let mut body = harmonic_stack(len, 62.0, 10, 1.2, 1.0, 0x3001);
    for (i, sample) in body.samples.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        *sample *= 0.7 + 0.3 * (TAU * 3.0 * t).sin();
    }

    sustained(
        layered(LOOP_SECONDS, &[(roar, 0.0, 1.0), (grit, 0.0, 0.5)]),
        body,
    )
}

/// The thrusters' upper band: the part of a rocket that cuts through everything.
pub fn thruster_hiss() -> Buffer {
    let source = loop_source_len();
    let len = samples(LOOP_SECONDS);

    let mut hiss = moving_noise(source, 0.85, 3_200.0, 0.5, 4.0, 0.9, 0x4002);
    let edge = moving_noise(source, 0.5, 900.0, 0.4, 2.0, 2.0, 0x4003);
    for (i, sample) in hiss.samples.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        *sample *= 0.7 + 0.3 * (TAU * 6.0 * t).sin();
    }

    let mut whistle = Buffer {
        samples: vec![0.0; len],
    };
    for (i, sample) in whistle.samples.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        let vibrato = TAU * 1_900.0 * t + 0.4 * (TAU * 3.0 * t).sin();
        *sample = (vibrato.sin() * 0.18 + (vibrato * 1.5).sin() * 0.08)
            * (0.65 + 0.35 * (TAU * 2.0 * t).sin());
    }

    sustained(
        layered(LOOP_SECONDS, &[(hiss, 0.0, 1.0), (edge, 0.0, 0.4)]),
        whistle,
    )
}

// ---------------------------------------------------------------------------
// Weapons
// ---------------------------------------------------------------------------

/// The player's autocannon: a heavy round leaving a heavy gun.
///
/// Four layers, because that is what a real one is. The crack is the shock
/// leaving the muzzle; the action is the receiver ringing, which is a comb
/// rather than a tone because a box full of steel has no single pitch; the body
/// is the mass of the round and the propellant; the tail is the world answering
/// back. Without the body it is a firework, and without the tail it is a
/// firework in a vacuum.
pub fn autocannon(seed: u32) -> Buffer {
    let crack = noise_layer(NoiseSpec {
        duration: 0.045,
        gain: 1.0,
        from_hz: 8_000.0,
        to_hz: 1_400.0,
        shape: Shape::Band,
        q: 0.6,
        seed: mixed(seed),
        attack: 0.0004,
        curve: 3.4,
        peak: 0.0,
    });
    // The middle of the crack, where a gunshot actually lives: the first layer
    // is the shock and this is the air coming back together behind it. Without
    // it the sound is a click sitting on a boom with a hole in between.
    let snap = noise_layer(NoiseSpec {
        duration: 0.05,
        gain: 0.6,
        from_hz: 5_000.0,
        to_hz: 1_100.0,
        shape: Shape::Band,
        q: 0.9,
        seed: mixed(seed ^ 0x1f),
        attack: 0.0004,
        curve: 2.2,
        peak: 0.0,
    });

    let mut action = noise_layer(NoiseSpec {
        duration: 0.07,
        gain: 0.75,
        from_hz: 4_200.0,
        to_hz: 1_300.0,
        shape: Shape::Band,
        q: 1.4,
        seed: mixed(seed ^ 0x51),
        attack: 0.0004,
        curve: 2.4,
        peak: 0.0,
    });
    comb(&mut action, 0.0011, 0.55, 0.5);

    // The body is a thump rather than a boom: it is there to give the round a
    // mass, and a gun whose body outlives its crack is a bass drum.
    let body = thump(160.0, 45.0, 0.11, 0.5, 2.6);
    let body_edge = thump(320.0, 96.0, 0.07, 0.22, 3.0);
    let tail = noise_layer(NoiseSpec {
        duration: 0.3,
        gain: 0.2,
        from_hz: 3_000.0,
        to_hz: 500.0,
        shape: Shape::Band,
        q: 0.9,
        seed: mixed(seed ^ 0x9e),
        attack: 0.001,
        curve: 2.2,
        peak: 0.0,
    });

    finish(
        layered(
            0.34,
            &[
                (crack, 0.0, 1.0),
                (snap, 0.0, 0.8),
                (action, 0.002, 0.9),
                (body, 0.0, 0.95),
                (body_edge, 0.0, 0.5),
                (tail, 0.012, 0.9),
            ],
        ),
        PEAK_ONE_SHOT,
    )
}

/// The enemy's machine gun: lighter and higher than the autocannon.
///
/// The same four layers with everything moved up and shortened, so that in a
/// firefight the player can tell which of the two sounds is pointed at them
/// without looking. The body is two octaves up and a third as long, which is
/// what "lighter round" means physically and what it has to mean here.
pub fn machine_gun(seed: u32) -> Buffer {
    let crack = noise_layer(NoiseSpec {
        duration: 0.02,
        gain: 1.0,
        from_hz: 12_000.0,
        to_hz: 4_500.0,
        shape: Shape::Band,
        q: 0.7,
        seed: mixed(seed),
        attack: 0.0003,
        curve: 3.8,
        peak: 0.0,
    });
    let snap = noise_layer(NoiseSpec {
        duration: 0.026,
        gain: 0.7,
        from_hz: 7_000.0,
        to_hz: 2_400.0,
        shape: Shape::Band,
        q: 0.9,
        seed: mixed(seed ^ 0x0d),
        attack: 0.0003,
        curve: 2.4,
        peak: 0.0,
    });

    let mut action = noise_layer(NoiseSpec {
        duration: 0.035,
        gain: 0.6,
        from_hz: 6_500.0,
        to_hz: 2_600.0,
        shape: Shape::Band,
        q: 1.6,
        seed: mixed(seed ^ 0x37),
        attack: 0.0003,
        curve: 2.6,
        peak: 0.0,
    });
    comb(&mut action, 0.0008, 0.5, 0.4);

    // Lighter than the autocannon in both senses: less under it, and shorter.
    let body = thump(320.0, 110.0, 0.06, 0.55, 3.2);
    // The one layer kept from the first pass: a plain swept burst is exactly
    // right for a short tail.
    let tail = noise_burst(0.09, 0.24, 4_800.0, 1_200.0, Shape::Band, 1.0);

    finish(
        layered(
            0.17,
            &[
                (crack, 0.0, 1.0),
                (snap, 0.0, 0.8),
                (action, 0.001, 0.85),
                (body, 0.0, 0.7),
                (tail, 0.008, 0.8),
            ],
        ),
        PEAK_ONE_SHOT,
    )
}

/// A missile leaving: a whoosh that rises and travels, over a roar.
///
/// "Travels" is the comb sweep. The delay lengthens across the sound, which
/// drops the resonance the air is ringing at, which is what a source going away
/// from the listener does to everything it makes. The filter sweep on the noise
/// rises at the same time, because the motor is spooling up as the missile
/// leaves — two opposite movements at once is what makes it read as motion
/// rather than as a filter being swept.
pub fn missile_launch(seed: u32) -> Buffer {
    let ignition = noise_layer(NoiseSpec {
        duration: 0.05,
        gain: 0.8,
        from_hz: 7_000.0,
        to_hz: 900.0,
        shape: Shape::Band,
        q: 0.8,
        seed: mixed(seed),
        attack: 0.0005,
        curve: 2.4,
        peak: 0.0,
    });

    let mut whoosh = noise_layer(NoiseSpec {
        duration: 1.0,
        gain: 0.75,
        from_hz: 220.0,
        to_hz: 3_200.0,
        shape: Shape::Band,
        q: 0.9,
        seed: mixed(seed ^ 0x11),
        attack: 0.0,
        curve: 1.3,
        peak: 0.45,
    });
    comb_sweep(&mut whoosh, 0.0007, 0.0038, 0.62, 0.45);

    let passing = noise_layer(NoiseSpec {
        duration: 0.6,
        gain: 0.3,
        from_hz: 3_400.0,
        to_hz: 800.0,
        shape: Shape::Band,
        q: 0.8,
        seed: mixed(seed ^ 0x22),
        attack: 0.0,
        curve: 1.5,
        peak: 0.35,
    });

    let roar = noise_layer(NoiseSpec {
        duration: 1.15,
        gain: 0.6,
        from_hz: 700.0,
        to_hz: 110.0,
        shape: Shape::Low,
        q: 0.8,
        seed: mixed(seed ^ 0x33),
        attack: 0.004,
        curve: 1.6,
        peak: 0.0,
    });

    // The motor spooling: a pair of detuned saws climbing, which is the one
    // part of a launch that has a pitch.
    let spool = layered(
        0.9,
        &[
            (tone(Wave::Saw, 58.0, 104.0, 0.9, 0.5), 0.0, 1.0),
            (tone(Wave::Saw, 61.0, 110.0, 0.9, 0.4), 0.0, 1.0),
        ],
    );

    let tail = noise_layer(NoiseSpec {
        duration: 0.5,
        gain: 0.2,
        from_hz: 1_400.0,
        to_hz: 200.0,
        shape: Shape::Band,
        q: 0.7,
        seed: mixed(seed ^ 0x44),
        attack: 0.02,
        curve: 1.8,
        peak: 0.0,
    });

    finish(
        layered(
            1.25,
            &[
                (ignition, 0.0, 0.9),
                (whoosh, 0.06, 1.0),
                (passing, 0.55, 0.8),
                (roar, 0.02, 0.9),
                (spool, 0.02, 0.35),
                (tail, 0.7, 0.7),
            ],
        ),
        PEAK_ONE_SHOT,
    )
}

// ---------------------------------------------------------------------------
// Impacts
// ---------------------------------------------------------------------------

/// A hard impact: steel ringing, with the strike that started it.
///
/// `size` scales the object: below one is a railing, above one is a container.
/// The plate's modes are inharmonic and decay at different rates, so the ring
/// goes hollow as it fades instead of just getting quieter.
pub fn impact_hard(seed: u32, size: f32) -> Buffer {
    let size = size.clamp(0.5, 2.5);
    let ring = 0.62 * size;

    let strike = noise_layer(NoiseSpec {
        duration: 0.016,
        gain: 1.0,
        from_hz: 12_000.0,
        to_hz: 3_000.0,
        shape: Shape::Band,
        q: 0.8,
        seed: mixed(seed),
        attack: 0.0003,
        curve: 3.2,
        peak: 0.0,
    });

    // Higher than a hit on a bulkhead would be, because a prop taking a round
    // is a small thing ringing: 420 Hz for a railing, less for a container.
    let mut metal = plate(ring + 0.12, 520.0 / size, ring, 1.0);
    comb(&mut metal, 0.0013 * size, 0.45, 0.3);

    let weight = thump(180.0 / size, 60.0 / size, 0.12, 0.35, 2.4);

    finish(
        layered(
            ring + 0.14,
            &[(strike, 0.0, 1.0), (metal, 0.0, 1.0), (weight, 0.0, 0.6)],
        ),
        PEAK_ONE_SHOT,
    )
}

/// A soft impact: no ring at all, because soil and plastic do not have modes.
///
/// The difference from the hard impact is not the level, it is the absence of
/// everything above the thud: one low body, one dull cushion of noise, and a
/// texture of the material giving way underneath. A soft impact with any metal
/// in it stops being a soft impact.
pub fn impact_soft(seed: u32) -> Buffer {
    // A thud is a short event and a ring is a long one, and that difference is
    // most of what tells steel from soil at a distance. So this is deliberately
    // over well before the hard impact is.
    let body = thump(140.0, 52.0, 0.13, 0.95, 2.8);
    let cushion = noise_layer(NoiseSpec {
        duration: 0.15,
        gain: 0.6,
        from_hz: 800.0,
        to_hz: 140.0,
        shape: Shape::Low,
        q: 0.8,
        seed: mixed(seed),
        attack: 0.0006,
        curve: 2.2,
        peak: 0.0,
    });
    let texture = noise_layer(NoiseSpec {
        duration: 0.08,
        gain: 0.16,
        from_hz: 1_200.0,
        to_hz: 400.0,
        shape: Shape::Band,
        q: 1.2,
        seed: mixed(seed ^ 0x5a),
        attack: 0.0005,
        curve: 2.6,
        peak: 0.0,
    });

    finish(
        layered(
            0.22,
            &[(body, 0.0, 1.0), (cushion, 0.0, 0.9), (texture, 0.0, 0.7)],
        ),
        PEAK_ONE_SHOT,
    )
}

// ---------------------------------------------------------------------------
// The blade
// ---------------------------------------------------------------------------

/// A blade swing: air moving fast, with the arm that moved it underneath.
///
/// Two noise layers travelling in opposite directions — one brightening as the
/// blade comes through, one darkening after it has gone past — which together
/// are an arc rather than a sweep. The actuator at the front is there because a
/// six-tonne machine cannot swing anything without a motor committing to it
/// first, and that commitment is the part the player is being warned about.
pub fn blade_swing(seed: u32) -> Buffer {
    let approach = noise_layer(NoiseSpec {
        duration: 0.34,
        gain: 0.7,
        from_hz: 500.0,
        to_hz: 3_200.0,
        shape: Shape::Band,
        q: 1.1,
        seed: mixed(seed),
        attack: 0.0,
        curve: 1.4,
        peak: 0.42,
    });
    let departure = noise_layer(NoiseSpec {
        duration: 0.4,
        gain: 0.62,
        from_hz: 3_600.0,
        to_hz: 800.0,
        shape: Shape::Band,
        q: 1.0,
        seed: mixed(seed ^ 0x77),
        attack: 0.0,
        curve: 1.6,
        peak: 0.42,
    });
    let edge = tone(Wave::Sine, 780.0, 1_500.0, 0.26, 0.16);
    let actuator = thump(115.0, 62.0, 0.16, 0.4, 2.2);

    finish(
        layered(
            0.42,
            &[
                (approach, 0.0, 1.0),
                (departure, 0.02, 0.95),
                (edge, 0.03, 0.7),
                (actuator, 0.0, 0.8),
            ],
        ),
        PEAK_ONE_SHOT,
    )
}

/// Where a struck blade's plate rings when nothing else is known about it.
const BLADE_PITCH: f32 = 230.0;

/// How big the thing the blade hit was, for a given take.
///
/// The seed moves it by a few per cent either way: small enough that every hit
/// is recognisably the same event, large enough that two hits in a fight are two
/// hits rather than one hit played twice.
fn blade_size(seed: u32) -> f32 {
    0.93 + (seed % 5) as f32 * 0.035
}

/// A blade hit: struck steel, which is several inharmonic partials with
/// different decay rates rather than one note.
///
/// The fundamental is low enough to carry the mass of the blade and the modes
/// above it are dense enough to be a clang. The strike on the front and the
/// body under it are what make it a hit rather than a bell being rung politely.
pub fn blade_hit(seed: u32) -> Buffer {
    let strike = noise_layer(NoiseSpec {
        duration: 0.02,
        gain: 1.0,
        from_hz: 14_000.0,
        to_hz: 3_500.0,
        shape: Shape::Band,
        q: 0.8,
        seed: mixed(seed),
        attack: 0.0003,
        curve: 3.0,
        peak: 0.0,
    });

    // Two rings rather than one: the blade is a plate and the thing it hit is
    // another, and the second one is smaller, higher and shorter. It is what
    // puts the edge on the clang — a single plate rings like a gong.
    let size = blade_size(seed);
    let mut metal = plate(1.05, BLADE_PITCH / size, 0.75 * size, 1.0);
    comb(&mut metal, 0.0013, 0.5, 0.35);
    let mut edge = plate(0.5, 640.0 / size, 0.3 * size, 0.9);
    comb(&mut edge, 0.0008, 0.4, 0.3);

    let body = thump(95.0, 42.0, 0.09, 0.35, 2.6);
    let scrape = noise_layer(NoiseSpec {
        duration: 0.14,
        gain: 0.3,
        from_hz: 2_600.0,
        to_hz: 700.0,
        shape: Shape::Band,
        q: 1.4,
        seed: mixed(seed ^ 0x6b),
        attack: 0.002,
        curve: 2.4,
        peak: 0.0,
    });

    finish(
        layered(
            1.0,
            &[
                (strike, 0.0, 1.0),
                (metal, 0.0, 1.0),
                (edge, 0.0, 0.8),
                (body, 0.0, 0.8),
                (scrape, 0.01, 0.6),
            ],
        ),
        PEAK_ONE_SHOT,
    )
}

// ---------------------------------------------------------------------------
// Movement
// ---------------------------------------------------------------------------

/// One foot of a multi-tonne machine coming down.
///
/// The thud is the mass; the servo is the leg that put it there, still winding
/// down; the scatter is what was on the ground. The two steps are rendered
/// differently — the left is heavier and the right lands a little sooner — so
/// that walking is not one sample repeated, which the ear picks out almost
/// immediately.
pub fn footstep(step: u8) -> Buffer {
    let seed = 0x5700 + step as u32 * 0x3d;
    let heavy = if step.is_multiple_of(2) { 1.0 } else { 0.88 };

    let body = thump(105.0 * heavy, 36.0, 0.24, 0.95, 2.6);
    let ground = noise_layer(NoiseSpec {
        duration: 0.26,
        gain: 0.5,
        from_hz: 620.0,
        to_hz: 90.0,
        shape: Shape::Low,
        q: 0.9,
        seed: mixed(seed),
        attack: 0.0008,
        curve: 2.4,
        peak: 0.0,
    });
    let pad = plate(0.26, 430.0 * heavy, 0.22, 0.16);
    // The servo is quiet on its own and has to be pushed to be heard under a
    // thud that is four times its level: a machine that puts a foot down is
    // heard as hydraulics as much as it is heard as weight.
    let leg = servo(0.18, 0.5, 2_400.0 * heavy, 1_500.0, seed ^ 0x2f);
    let scatter = debris(0.34, 0.14, 4, 1_400.0, 0.9, seed ^ 0x81);

    finish(
        layered(
            0.42,
            &[
                (body, 0.0, 1.0),
                (ground, 0.0, 0.85),
                (pad, 0.0, 0.7),
                (leg, 0.0, 0.75),
                (scatter, 0.05, 0.8),
            ],
        ),
        PEAK_ONE_SHOT,
    )
}

/// A heavy landing: a footstep with the whole machine behind it.
pub fn landing(seed: u32) -> Buffer {
    let body = thump(120.0, 34.0, 0.34, 1.0, 2.2);
    let ground = noise_layer(NoiseSpec {
        duration: 0.36,
        gain: 0.55,
        from_hz: 700.0,
        to_hz: 80.0,
        shape: Shape::Low,
        q: 0.9,
        seed: mixed(seed),
        attack: 0.001,
        curve: 2.0,
        peak: 0.0,
    });
    let mut groan = plate(0.72, 240.0, 0.6, 0.28);
    comb(&mut groan, 0.0021, 0.4, 0.25);
    let leg = servo(0.34, 0.4, 1_900.0, 900.0, seed ^ 0x19);
    let scatter = debris(0.5, 0.2, 7, 1_800.0, 1.1, seed ^ 0x91);

    finish(
        layered(
            0.62,
            &[
                (body, 0.0, 1.0),
                (ground, 0.0, 0.9),
                (groan, 0.0, 0.7),
                (leg, 0.0, 0.5),
                (scatter, 0.06, 0.85),
            ],
        ),
        PEAK_ONE_SHOT,
    )
}

/// A boost lighting: a short rising whoosh with a shove under it.
pub fn boost(seed: u32) -> Buffer {
    let mut whoosh = noise_layer(NoiseSpec {
        duration: 0.34,
        gain: 0.8,
        from_hz: 400.0,
        to_hz: 4_200.0,
        shape: Shape::Band,
        q: 0.9,
        seed: mixed(seed),
        attack: 0.0,
        curve: 1.8,
        peak: 0.28,
    });
    comb_sweep(&mut whoosh, 0.0025, 0.0009, 0.55, 0.35);
    let shove = thump(90.0, 140.0, 0.2, 0.6, 2.0);
    let ignite = noise_layer(NoiseSpec {
        duration: 0.03,
        gain: 0.7,
        from_hz: 8_000.0,
        to_hz: 2_000.0,
        shape: Shape::Band,
        q: 0.9,
        seed: mixed(seed ^ 0x2b),
        attack: 0.0003,
        curve: 3.0,
        peak: 0.0,
    });

    finish(
        layered(
            0.42,
            &[(ignite, 0.0, 0.8), (whoosh, 0.0, 1.0), (shove, 0.0, 0.5)],
        ),
        PEAK_ONE_SHOT,
    )
}

// ---------------------------------------------------------------------------
// Explosions and damage
// ---------------------------------------------------------------------------

/// An explosion: a deep boom, a noise tail that decays over about a second, and
/// debris coming down through it.
///
/// The boom is three low sines falling at different rates, because one sine is
/// a bass note. The tail is the part that carries the distance: it outlives
/// everything else by half a second, which is what a listener uses to tell a
/// large explosion from a small one at the same level.
pub fn explosion(seed: u32) -> Buffer {
    let crack = noise_layer(NoiseSpec {
        duration: 0.05,
        gain: 0.9,
        from_hz: 10_000.0,
        to_hz: 1_600.0,
        shape: Shape::Band,
        q: 0.7,
        seed: mixed(seed),
        attack: 0.0004,
        curve: 2.8,
        peak: 0.0,
    });

    let deep = thump(115.0, 28.0, 0.85, 1.0, 1.5);
    let upper = thump(172.0, 42.0, 0.5, 0.5, 2.2);
    let sub = thump(46.0, 30.0, 1.0, 0.6, 0.9);

    let mut tail = noise_layer(NoiseSpec {
        duration: 1.05,
        gain: 0.65,
        from_hz: 1_500.0,
        to_hz: 70.0,
        shape: Shape::Low,
        q: 0.7,
        seed: mixed(seed ^ 0x13),
        attack: 0.002,
        curve: 1.3,
        peak: 0.0,
    });
    comb(&mut tail, 0.004, 0.3, 0.2);

    let crackle = noise_layer(NoiseSpec {
        duration: 0.6,
        gain: 0.28,
        from_hz: 2_600.0,
        to_hz: 500.0,
        shape: Shape::Band,
        q: 0.8,
        seed: mixed(seed ^ 0x27),
        attack: 0.01,
        curve: 1.6,
        peak: 0.0,
    });

    let debris = debris(1.0, 0.3, 16, 2_600.0, 1.5, seed ^ 0x35);

    finish(
        layered(
            1.25,
            &[
                (crack, 0.0, 0.9),
                (deep, 0.0, 1.0),
                (upper, 0.0, 0.5),
                (sub, 0.0, 0.8),
                (tail, 0.02, 0.9),
                (crackle, 0.02, 0.7),
                (debris, 0.06, 0.9),
            ],
        ),
        PEAK_ONE_SHOT,
    )
}

/// Taking a hit: the mech's own hull, plus the alarm that says so.
pub fn player_damage(seed: u32) -> Buffer {
    let mut hull = plate(0.6, 300.0, 0.45, 0.9);
    comb(&mut hull, 0.0017, 0.5, 0.3);
    let body = thump(150.0, 55.0, 0.18, 0.7, 2.4);
    let alarm = tone(Wave::Triangle, 880.0, 620.0, 0.22, 0.2);
    let strike = noise_layer(NoiseSpec {
        duration: 0.02,
        gain: 0.7,
        from_hz: 9_000.0,
        to_hz: 2_400.0,
        shape: Shape::Band,
        q: 0.9,
        seed: mixed(seed),
        attack: 0.0003,
        curve: 3.0,
        peak: 0.0,
    });

    finish(
        layered(
            0.5,
            &[
                (strike, 0.0, 0.8),
                (hull, 0.0, 1.0),
                (body, 0.0, 0.7),
                (alarm, 0.03, 0.55),
            ],
        ),
        PEAK_ONE_SHOT,
    )
}

/// Losing balance: a long fall in pitch with the servos grinding against it.
pub fn player_stagger(seed: u32) -> Buffer {
    let fall = layered(
        0.75,
        &[
            (tone(Wave::Saw, 300.0, 58.0, 0.72, 0.6), 0.0, 1.0),
            (tone(Wave::Saw, 306.0, 61.0, 0.72, 0.45), 0.0, 1.0),
        ],
    );
    let grind = noise_layer(NoiseSpec {
        duration: 0.66,
        gain: 0.4,
        from_hz: 1_600.0,
        to_hz: 180.0,
        shape: Shape::Band,
        q: 1.3,
        seed: mixed(seed),
        attack: 0.01,
        curve: 1.4,
        peak: 0.0,
    });
    let leg = servo(0.3, 0.45, 1_400.0, 700.0, seed ^ 0x4d);
    let drop = thump(70.0, 30.0, 0.5, 0.7, 1.4);

    finish(
        layered(
            0.8,
            &[
                (fall, 0.0, 0.9),
                (grind, 0.02, 0.8),
                (leg, 0.0, 0.5),
                (drop, 0.08, 0.8),
            ],
        ),
        PEAK_ONE_SHOT,
    )
}

// ---------------------------------------------------------------------------
// The HUD
// ---------------------------------------------------------------------------

/// The sounds the player will hear more than any others, which is the whole
/// design brief: short, clean, and quiet enough to survive the hundredth
/// hearing. Every one of them is a triangle or a sine — a square wave at the
/// same level is a much more tiring sound — and every one of them is over
/// before it can become a melody.
pub fn energy_warning() -> Buffer {
    let first = tone(Wave::Triangle, 760.0, 700.0, 0.11, 0.5);
    let second = tone(Wave::Triangle, 570.0, 520.0, 0.13, 0.45);
    finish(
        layered(0.26, &[(first, 0.0, 1.0), (second, 0.13, 1.0)]),
        PEAK_UI,
    )
}

/// Lock acquired: two rising notes, which is the shortest way to say it.
pub fn lock_acquired() -> Buffer {
    let first = tone(Wave::Sine, 1_046.0, 1_046.0, 0.08, 0.5);
    let second = tone(Wave::Sine, 1_568.0, 1_568.0, 0.11, 0.45);
    finish(
        layered(0.22, &[(first, 0.0, 1.0), (second, 0.08, 1.0)]),
        PEAK_UI,
    )
}

/// Lock lost: the same two notes, falling, and quieter.
pub fn lock_lost() -> Buffer {
    let first = tone(Wave::Sine, 880.0, 880.0, 0.08, 0.45);
    let second = tone(Wave::Sine, 587.0, 587.0, 0.11, 0.4);
    finish(
        layered(0.2, &[(first, 0.0, 1.0), (second, 0.08, 1.0)]),
        PEAK_UI,
    )
}

/// A button: one soft blip, nothing more.
pub fn ui_click() -> Buffer {
    finish(tone(Wave::Triangle, 1_180.0, 940.0, 0.06, 0.5), PEAK_UI)
}

/// A confirmation: two notes going up and out of the way.
pub fn ui_confirm() -> Buffer {
    let first = tone(Wave::Triangle, 523.0, 523.0, 0.1, 0.45);
    let second = tone(Wave::Triangle, 784.0, 784.0, 0.14, 0.4);
    finish(
        layered(0.24, &[(first, 0.0, 1.0), (second, 0.08, 1.0)]),
        PEAK_UI,
    )
}

/// A repair starting: a ratchet turning, which is a servo with a rhythm.
pub fn repair(seed: u32) -> Buffer {
    let mut mix = Buffer::silence(0.75);
    for i in 0..4 {
        let tick = servo(0.12, 0.5, 1_900.0, 1_300.0, seed + i);
        mix.mix_at(&tick, i as f32 * 0.14, 1.0 - i as f32 * 0.08);
    }
    let rise = tone(Wave::Sine, 320.0, 700.0, 0.6, 0.3);
    mix.mix_at(&rise, 0.05, 1.0);
    finish(mix, PEAK_UI)
}

/// A heavy weapon winding up, somewhere else on the field. Positional, so it
/// has to be audible through distance and still read as a warning — which is
/// the one place in the set where a square wave earns its harshness.
pub fn telegraph() -> Buffer {
    let rise = tone(Wave::Square, 150.0, 240.0, 0.5, 0.4);
    let air = noise_layer(NoiseSpec {
        duration: 0.55,
        gain: 0.3,
        from_hz: 400.0,
        to_hz: 1_800.0,
        shape: Shape::Band,
        q: 1.2,
        seed: 0x7e01,
        attack: 0.05,
        curve: 0.9,
        peak: 0.0,
    });
    let pulse = noise_layer(NoiseSpec {
        duration: 0.55,
        gain: 0.25,
        from_hz: 2_200.0,
        to_hz: 2_200.0,
        shape: Shape::Band,
        q: 4.0,
        seed: 0x7e02,
        attack: 0.02,
        curve: 1.1,
        peak: 0.0,
    });
    finish(
        layered(
            0.6,
            &[(rise, 0.0, 1.0), (air, 0.0, 0.8), (pulse, 0.05, 0.5)],
        ),
        PEAK_ONE_SHOT,
    )
}

/// The mission won: a rising figure with a shimmer over it.
pub fn mission_complete() -> Buffer {
    let mut mix = Buffer::silence(1.2);
    for (i, hz) in [523.0, 659.0, 784.0, 1_046.0].into_iter().enumerate() {
        mix.mix_at(
            &tone(Wave::Triangle, hz, hz, 0.6, 0.5),
            i as f32 * 0.13,
            1.0,
        );
        // A quiet octave above each note: the difference between a chime and a
        // test tone is that a chime has more than one thing ringing.
        mix.mix_at(
            &tone(Wave::Sine, hz * 2.0, hz * 2.0, 0.4, 0.16),
            i as f32 * 0.13,
            1.0,
        );
    }
    let pad = tone(Wave::Sine, 262.0, 262.0, 1.0, 0.22);
    mix.mix_at(&pad, 0.2, 1.0);
    finish(mix, PEAK_UI)
}

/// The mission lost: the same figure falling, with a drone underneath it.
pub fn mission_failed() -> Buffer {
    let mut mix = Buffer::silence(1.4);
    for (i, hz) in [392.0, 330.0, 262.0, 196.0].into_iter().enumerate() {
        mix.mix_at(
            &tone(Wave::Triangle, hz, hz * 0.985, 0.7, 0.45),
            i as f32 * 0.18,
            1.0,
        );
    }
    let drone = thump(98.0, 62.0, 1.2, 0.3, 0.8);
    mix.mix_at(&drone, 0.1, 1.0);
    finish(mix, PEAK_UI)
}

/// The measurements that stand in for ears.
///
/// Nothing here is used by the game: these are the instruments the tests listen
/// with, and they live behind `cfg(test)` so that the shipped synthesiser stays
/// a pure renderer with no analysis in it. A sound design that cannot be
/// measured is one that cannot be changed without fear, so the tools for
/// measuring it are part of the design rather than an afterthought.
#[cfg(test)]
pub(crate) mod ears {
    use super::{Buffer, SAMPLE_RATE};

    /// Magnitude at one frequency, by Goertzel, Hann-windowed.
    ///
    /// Windowing matters: a rectangular window leaks a strong low partial into
    /// every probe above it at about -13 dB, which is enough to make "the
    /// machine gun is brighter than the autocannon" unprovable. Hann takes the
    /// leakage down to about -31 dB.
    ///
    /// The accumulator is `f64` and not for precision's sake. The Goertzel
    /// recurrence has its poles on the unit circle, so in `f32` the round-off
    /// grows across twenty thousand samples until it is larger than the quiet
    /// high-frequency content a brightness measurement is looking for — and a
    /// probe that returns round-off instead of silence reports every sound as
    /// being brightest at the top of the range.
    pub fn energy_at(samples: &[f32], hz: f32) -> f32 {
        let len = samples.len();
        if len < 4 {
            return 0.0;
        }
        let omega = std::f64::consts::TAU * hz as f64 / SAMPLE_RATE as f64;
        let coeff = 2.0 * omega.cos();
        let (mut s1, mut s2) = (0.0f64, 0.0f64);
        let denominator = (len - 1) as f64;
        for (i, sample) in samples.iter().enumerate() {
            let window = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / denominator).cos();
            let s0 = *sample as f64 * window + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        let magnitude = (s1 * s1 + s2 * s2 - coeff * s1 * s2).max(0.0).sqrt();
        (magnitude / len as f64) as f32
    }

    /// The number of bands the brightness of a sound is measured in.
    const LOWEST: f32 = 40.0;
    const HIGHEST: f32 = 16_000.0;
    /// The longest stretch of a sound a spectrum is taken of, in samples.
    /// A power of two, because the transform below is radix-2.
    const MAX_FFT: usize = 32_768;

    /// The largest power of two that fits inside the buffer.
    fn fft_size(len: usize) -> usize {
        let capped = len.min(MAX_FFT);
        if capped.is_power_of_two() {
            capped
        } else {
            capped.next_power_of_two() / 2
        }
    }

    /// An in-place radix-2 Cooley-Tukey transform.
    ///
    /// Fifty lines of arithmetic rather than an FFT dependency, because this is
    /// test-only instrumentation and the crate has no dependencies to spend.
    fn fft(re: &mut [f64], im: &mut [f64]) {
        let n = re.len();
        let mut j = 0usize;
        for i in 1..n {
            let mut bit = n >> 1;
            while j & bit != 0 {
                j ^= bit;
                bit >>= 1;
            }
            j |= bit;
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let angle = -std::f64::consts::TAU / len as f64;
            let (wr, wi) = (angle.cos(), angle.sin());
            let mut start = 0;
            while start < n {
                let (mut cr, mut ci) = (1.0f64, 0.0f64);
                for k in 0..len / 2 {
                    let (ur, ui) = (re[start + k], im[start + k]);
                    let (xr, xi) = (re[start + k + len / 2], im[start + k + len / 2]);
                    let (vr, vi) = (xr * cr - xi * ci, xr * ci + xi * cr);
                    re[start + k] = ur + vr;
                    im[start + k] = ui + vi;
                    re[start + k + len / 2] = ur - vr;
                    im[start + k + len / 2] = ui - vi;
                    let next = cr * wr - ci * wi;
                    ci = cr * wi + ci * wr;
                    cr = next;
                }
                start += len;
            }
            len <<= 1;
        }
    }

    /// A Hann-windowed magnitude spectrum, one value per bin below Nyquist.
    ///
    /// Normalised so that a full-scale sine reads 1.0 at its bin, which makes
    /// the numbers comparable with amplitudes rather than with arbitrary units.
    /// Only the first `MAX_FFT` samples are used: a window long enough to
    /// resolve the bass of an explosion is a window that cannot also be short
    /// enough to see its crack, and the crack is what the brightness of an
    /// explosion is about.
    pub fn spectrum(samples: &[f32]) -> Vec<f32> {
        let size = fft_size(samples.len());
        if size < 8 {
            return Vec::new();
        }
        let mut re = vec![0.0f64; size];
        let mut im = vec![0.0f64; size];
        let last = (size - 1) as f64;
        for (i, slot) in re.iter_mut().enumerate() {
            let window = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / last).cos();
            *slot = samples[i] as f64 * window;
        }
        fft(&mut re, &mut im);
        // Hann's coherent gain is a half, and a real transform splits a sine
        // between the positive and negative frequencies, so a sine of amplitude
        // one comes out as `size / 2` before the division below.
        let scale = 2.0 / size as f64;
        (0..size / 2)
            .map(|k| ((re[k] * re[k] + im[k] * im[k]).sqrt() * scale) as f32)
            .collect()
    }

    /// A spectral centroid: the frequency the sound's energy is balanced around.
    ///
    /// The standard definition, over the audible band only — bins below 40 Hz
    /// are rumble the player's speakers will not reproduce and bins above
    /// 16 kHz are the noise floor of a 44.1 kHz render. A pure tone measures at
    /// its own frequency; broadband noise measures high, because it is bright.
    pub fn centroid_hz(samples: &[f32]) -> f32 {
        let spectrum = spectrum(samples);
        if spectrum.is_empty() {
            return 0.0;
        }
        let bin_hz = SAMPLE_RATE as f32 / (spectrum.len() * 2) as f32;
        let (mut weighted, mut total) = (0.0f32, 0.0f32);
        for (bin, magnitude) in spectrum.iter().enumerate() {
            let hz = (bin as f32 + 0.5) * bin_hz;
            if !(LOWEST..=HIGHEST).contains(&hz) {
                continue;
            }
            let energy = magnitude * magnitude;
            weighted += hz * energy;
            total += energy;
        }
        if total <= 1e-12 {
            0.0
        } else {
            weighted / total
        }
    }

    /// The share of a sound's energy that sits above `hz`.
    ///
    /// The other half of the brightness claim, and a more forgiving one than the
    /// centroid for sounds with a lot of bass in them: "a tenth of this impact
    /// is above 2 kHz and a hundredth of that one is" is a statement about
    /// character, where a centroid is a statement about a single number.
    pub fn high_share(samples: &[f32], hz: f32) -> f32 {
        let spectrum = spectrum(samples);
        if spectrum.is_empty() {
            return 0.0;
        }
        let bin_hz = SAMPLE_RATE as f32 / (spectrum.len() * 2) as f32;
        let (mut high, mut total) = (0.0f32, 0.0f32);
        for (bin, magnitude) in spectrum.iter().enumerate() {
            let frequency = (bin as f32 + 0.5) * bin_hz;
            if frequency < LOWEST || frequency > HIGHEST {
                continue;
            }
            let energy = magnitude * magnitude;
            total += energy;
            if frequency >= hz {
                high += energy;
            }
        }
        if total <= 1e-12 {
            0.0
        } else {
            high / total
        }
    }

    pub fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    /// RMS of the window between two fractions of the buffer.
    pub fn window_rms(samples: &[f32], from: f32, to: f32) -> f32 {
        let len = samples.len();
        let start = ((from.clamp(0.0, 1.0)) * len as f32) as usize;
        let end = ((to.clamp(0.0, 1.0)) * len as f32) as usize;
        if start >= end || end > len {
            return 0.0;
        }
        rms(&samples[start..end])
    }

    /// RMS between two moments in seconds.
    ///
    /// The window is clamped to the buffer, so comparing a long sound with a
    /// short one asks "how loud is each of them half a second in" rather than
    /// "how loud is each of them at the same fraction of itself", which is a
    /// question about the lengths rather than about the decay.
    pub fn rms_between(samples: &[f32], from: f32, to: f32) -> f32 {
        let len = samples.len();
        let start = ((from * SAMPLE_RATE as f32) as usize).min(len);
        let end = ((to * SAMPLE_RATE as f32) as usize).min(len);
        if start >= end {
            return 0.0;
        }
        rms(&samples[start..end])
    }

    /// RMS inside a frequency band, by cascading one-pole filters.
    ///
    /// One-poles rather than the state-variable filter: this has to be
    /// unconditionally stable at any cutoff, because a measurement instrument
    /// that rings is a measurement of itself. Twelve decibels an octave of
    /// slope is plenty to ask "is there anything up here".
    pub fn band_rms(samples: &[f32], low_hz: f32, high_hz: f32) -> f32 {
        // Two one-pole lowpasses and two one-pole highpasses, each pair giving
        // twelve decibels an octave.
        let low_alpha = 1.0 - (-std::f32::consts::TAU * high_hz / SAMPLE_RATE as f32).exp();
        let high_alpha = 1.0 - (-std::f32::consts::TAU * low_hz / SAMPLE_RATE as f32).exp();
        let (mut low1, mut low2, mut high1, mut high2) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let mut sum = 0.0f64;
        for sample in samples {
            low1 += low_alpha * (*sample - low1);
            low2 += low_alpha * (low1 - low2);
            // A highpass is what a lowpass leaves behind.
            high1 += high_alpha * (low2 - high1);
            high2 += high_alpha * (high1 - high2);
            let out = low2 - high2;
            sum += (out as f64) * (out as f64);
        }
        (sum / samples.len().max(1) as f64).sqrt() as f32
    }

    /// Zero crossings per second: a crude pitch, and enough to prove a sweep
    /// goes the way it was asked to.
    pub fn zero_crossing_rate(samples: &[f32]) -> f32 {
        if samples.len() < 2 {
            return 0.0;
        }
        let crossings = samples
            .windows(2)
            .filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0))
            .count();
        crossings as f32 * SAMPLE_RATE as f32 / samples.len() as f32
    }

    /// How many separate bursts of sound the buffer contains.
    ///
    /// A transient is a window whose level is `ratio` times the window before
    /// it and above `floor`. Counting crossings of an absolute threshold needs
    /// the signal to fall back below it between hits, which a scatter of ticks
    /// landing on top of a decaying boom never does — so the test measures the
    /// rise instead.
    pub fn transient_count(samples: &[f32], window: usize, floor: f32, ratio: f32) -> usize {
        let window = window.max(1);
        let mut count = 0;
        let mut previous = 0.0f32;
        for chunk in samples.chunks(window) {
            let level = rms(chunk);
            if level > floor && level > previous * ratio {
                count += 1;
            }
            previous = level;
        }
        count
    }

    /// Normalised autocorrelation at a lag in samples.
    ///
    /// Used to find the spacing of an echo train: an echo every `d` samples
    /// makes a signal correlate with itself at every multiple of `d`, and
    /// nothing else in a buffer that started life as one impulse does.
    pub fn autocorrelation(samples: &[f32], lag: usize) -> f32 {
        if lag == 0 || lag >= samples.len() {
            return 0.0;
        }
        let (mut sum, mut a, mut b) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..samples.len() - lag {
            let (x, y) = (samples[i] as f64, samples[i + lag] as f64);
            sum += x * y;
            a += x * x;
            b += y * y;
        }
        let scale = (a * b).sqrt();
        if scale <= 1e-18 {
            0.0
        } else {
            (sum / scale) as f32
        }
    }

    /// How different two buffers are, relative to how loud they are.
    ///
    /// The mean absolute difference is not usable for this: two takes of a
    /// gunshot are mostly silence by sample count, and the mean over the whole
    /// buffer is small however different the noisy part is. Dividing by the
    /// louder of the two makes the number a proportion, and a proportion is
    /// something a threshold can be set against.
    pub fn difference(a: &[f32], b: &[f32]) -> f32 {
        let len = a.len().min(b.len());
        if len == 0 {
            return 0.0;
        }
        let sum: f32 = a[..len]
            .iter()
            .zip(b[..len].iter())
            .map(|(x, y)| (x - y) * (x - y))
            .sum();
        let difference = (sum / len as f32).sqrt();
        let scale = rms(&a[..len]).max(rms(&b[..len]));
        if scale <= 1e-9 {
            0.0
        } else {
            difference / scale
        }
    }

    /// The largest step between neighbouring samples.
    ///
    /// This is the measurement a click is caught by: a sound whose seam steps
    /// further than any step inside it has a discontinuity in it, whatever it
    /// looks like on a plot.
    pub fn max_step(samples: &[f32]) -> f32 {
        samples
            .windows(2)
            .fold(0.0f32, |acc, pair| acc.max((pair[1] - pair[0]).abs()))
    }

    /// A RIFF/WAVE file around the buffer's samples, so a person can listen to
    /// what the tests measured.
    pub fn wav_bytes(buffer: &Buffer) -> Vec<u8> {
        let data = buffer.to_pcm16();
        let mut out = Vec::with_capacity(data.len() + 44);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((data.len() + 36) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&1u16.to_le_bytes()); // mono
        out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::ears;

    /// Every one-shot in the set, named, for the tests that have to make the
    /// same claim about all of them.
    fn one_shots() -> Vec<(&'static str, Buffer)> {
        vec![
            ("autocannon", autocannon(1)),
            ("machine_gun", machine_gun(2)),
            ("missile_launch", missile_launch(3)),
            ("explosion", explosion(4)),
            ("impact_hard", impact_hard(5, 1.0)),
            ("impact_soft", impact_soft(6)),
            ("blade_swing", blade_swing(7)),
            ("blade_hit", blade_hit(8)),
            ("footstep", footstep(0)),
            ("landing", landing(9)),
            ("boost", boost(10)),
            ("player_damage", player_damage(11)),
            ("player_stagger", player_stagger(12)),
            ("telegraph", telegraph()),
        ]
    }

    fn hud_sounds() -> Vec<(&'static str, Buffer)> {
        vec![
            ("energy_warning", energy_warning()),
            ("lock_acquired", lock_acquired()),
            ("lock_lost", lock_lost()),
            ("ui_click", ui_click()),
            ("ui_confirm", ui_confirm()),
            ("repair", repair(13)),
            ("mission_complete", mission_complete()),
            ("mission_failed", mission_failed()),
        ]
    }

    fn loops() -> Vec<(&'static str, Buffer)> {
        vec![
            ("engine_bed", engine_bed()),
            ("engine_load", engine_load()),
            ("thruster_roar", thruster_roar()),
            ("thruster_hiss", thruster_hiss()),
        ]
    }

    // -- the primitives -----------------------------------------------------

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
    fn the_whole_set_reports_its_own_measurements() {
        // Not a test so much as the instrument the other tests are written
        // against: it renders everything and prints what each sound measures.
        // Run it with `cargo test -- --nocapture the_whole_set_reports` when
        // changing a recipe and read the columns rather than trusting a guess
        // about which number moved.
        let report = |name: &str, buffer: &Buffer| {
            let mut bands = String::new();
            for (lo, hi) in [
                (20.0f32, 120.0f32),
                (120.0, 400.0),
                (400.0, 1_200.0),
                (1_200.0, 3_500.0),
                (3_500.0, 12_000.0),
            ] {
                let rms = ears::band_rms(&buffer.samples, lo, hi);
                bands.push_str(&format!(" {lo:.0}-{hi:.0}:{rms:.3}"));
            }
            println!(
                "{name:>16} centroid {:>7.1} attack {:>7.1} high {:.3} peak {:.3} len {:.2}{bands}",
                ears::centroid_hz(&buffer.samples),
                ears::centroid_hz(&buffer.samples[..buffer.len().min(2_205)]),
                ears::high_share(&buffer.samples, 2_000.0),
                buffer.peak(),
                buffer.len() as f32 / SAMPLE_RATE as f32
            );
        };
        for (name, buffer) in one_shots().into_iter().chain(hud_sounds()).chain(loops()) {
            report(name, &buffer);
        }
        // A pure tone has to measure at its own frequency, or the instrument is
        // measuring itself.
        for hz in [200.0f32, 1_000.0, 4_000.0] {
            let sine = tone(Wave::Sine, hz, hz, 0.5, 0.8);
            let measured = ears::centroid_hz(&sine.samples);
            assert!(
                (measured / hz - 1.0).abs() < 0.05,
                "a {hz} Hz tone measured at {measured} Hz"
            );
            report(&format!("sine {hz:.0}"), &sine);
        }
    }

    #[test]
    fn an_exact_normalise_lifts_a_quiet_mix_as_well_as_lowering_a_loud_one() {
        let mut quiet = Buffer {
            samples: vec![0.0, 0.05, -0.1],
        };
        quiet.normalise_exact(0.5);
        assert!((quiet.peak() - 0.5).abs() < 1e-6, "peak {}", quiet.peak());

        let mut loud = Buffer {
            samples: vec![0.0, 1.4, -0.2],
        };
        loud.normalise_exact(0.5);
        assert!((loud.peak() - 0.5).abs() < 1e-6, "peak {}", loud.peak());

        let mut silent = Buffer::silence(0.01);
        silent.normalise_exact(0.5);
        assert_eq!(silent.peak(), 0.0, "silence has nothing to scale");
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

    #[test]
    fn a_unit_draw_stays_inside_the_unit_interval() {
        let mut noise = Noise::new(0x51ce);
        for _ in 0..10_000 {
            let value = noise.unit();
            assert!((0.0..=1.0).contains(&value), "drew {value}");
        }
    }

    // -- envelopes ----------------------------------------------------------

    #[test]
    fn an_envelope_rises_to_its_peak_and_falls_to_nothing() {
        let env = Envelope::new(0.2, 0.01, 2.0);
        assert_eq!(env.gain(0.0), 0.0, "a sound must start from nothing");
        assert!(env.gain(0.01) > 0.99, "the attack should reach the peak");
        assert!(
            env.gain(0.12) < env.gain(0.03),
            "the decay should be falling"
        );
        assert_eq!(env.gain(0.2), 0.0);
        assert_eq!(env.gain(0.5), 0.0, "past the end is silence, not negative");
    }

    #[test]
    fn an_envelope_decays_without_ever_rising_again() {
        let env = Envelope::new(0.3, 0.005, 1.7);
        let mut previous = env.gain(0.005);
        for step in 1..300 {
            let t = 0.005 + step as f32 * 0.001;
            let gain = env.gain(t);
            assert!(gain <= previous + 1e-6, "rose at {t}: {previous} -> {gain}");
            previous = gain;
        }
    }

    #[test]
    fn a_steeper_decay_curve_is_quieter_sooner() {
        let fast = Envelope::new(0.4, 0.005, 3.0);
        let slow = Envelope::new(0.4, 0.005, 1.0);
        assert!(fast.gain(0.1) < slow.gain(0.1));
        assert!(fast.gain(0.3) < slow.gain(0.3));
    }

    #[test]
    fn a_bell_envelope_peaks_where_it_was_told_to() {
        let env = Envelope::bell(0.4, 0.45, 1.5);
        let mut loudest = (0.0f32, 0.0f32);
        for step in 0..=200 {
            let t = step as f32 * 0.002;
            let gain = env.gain(t);
            if gain > loudest.1 {
                loudest = (t, gain);
            }
        }
        assert!(
            (loudest.0 - 0.18).abs() < 0.03,
            "peaked at {} rather than at 45 % of the buffer",
            loudest.0
        );
        assert!(
            env.gain(0.36) < 0.2,
            "a bell has to be nearly gone by the end"
        );
    }

    // -- the building blocks ------------------------------------------------

    #[test]
    fn a_comb_repeats_a_click_at_its_delay() {
        // The impulse response of a feedback comb is the impulse, then the
        // impulse again at every multiple of the delay, quieter each time. That
        // is the whole reason it sounds like a room with a hard wall in it.
        let mut buffer = Buffer::silence(0.1);
        buffer.samples[0] = 1.0;
        comb(&mut buffer, 0.001, 0.5, 1.0);
        let d = (0.001 * SAMPLE_RATE as f32).round() as usize;
        assert!((buffer.samples[0] - 1.0).abs() < 1e-6);
        assert!((buffer.samples[d] - 0.5).abs() < 1e-6);
        assert!((buffer.samples[d * 2] - 0.25).abs() < 1e-6);
        assert!((buffer.samples[d * 3] - 0.125).abs() < 1e-6);
        assert!(buffer.samples[d / 2].abs() < 1e-9, "nothing between echoes");
    }

    #[test]
    fn a_comb_with_absurd_feedback_still_decays() {
        // A comb that reaches unity never stops, and a buffer that never decays
        // keeps ringing after the sound has finished.
        let mut buffer = noise_burst(0.2, 0.8, 3000.0, 500.0, Shape::Band, 1.0);
        comb(&mut buffer, 0.0007, 12.0, 1.0);
        assert!(buffer.samples.iter().all(|s| s.is_finite()));
        assert!(
            ears::window_rms(&buffer.samples, 0.0, 0.1)
                > ears::window_rms(&buffer.samples, 0.9, 1.0) * 10.0,
            "a comb with clamped feedback should still be a decay"
        );
    }

    #[test]
    fn a_swept_comb_moves_its_resonance_down() {
        // The delay lengthening is heard as a source going away: the resonance
        // at 1/delay falls as the source recedes. The control is the same sweep
        // run backwards, which has the same impulse, the same feedback and the
        // same length, and which therefore has to move the other way. Without
        // it this would pass on any decaying sound, which is not the claim.
        let sweep = |from: f32, to: f32| {
            let mut buffer = Buffer::silence(0.2);
            buffer.samples[0] = 1.0;
            comb_sweep(&mut buffer, from, to, 0.85, 1.0);
            buffer
        };
        let spacing = |window: &[f32]| {
            (30..700)
                .max_by(|a, b| {
                    let (one, two) = (
                        ears::autocorrelation(window, *a),
                        ears::autocorrelation(window, *b),
                    );
                    one.partial_cmp(&two).unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap_or(0)
        };
        let receding = sweep(0.001, 0.006);
        let approaching = sweep(0.006, 0.001);
        let early = 0..2_000;
        let late = 6_000..8_000;
        let receding_early = spacing(&receding.samples[early.clone()]);
        let receding_late = spacing(&receding.samples[late.clone()]);
        let approaching_early = spacing(&approaching.samples[early]);
        let approaching_late = spacing(&approaching.samples[late]);
        assert!(
            receding_late as f32 > receding_early as f32 * 1.5,
            "the receding echoes are not further apart: {receding_early} then {receding_late}"
        );
        assert!(
            approaching_late < approaching_early,
            "the approaching echoes are not closer together: \
             {approaching_early} then {approaching_late}"
        );
        assert!(receding.samples.iter().all(|s| s.is_finite()));
        // And the train dies away rather than growing, whichever way it moves.
        assert!(
            ears::rms_between(&receding.samples, 0.15, 0.2)
                < ears::rms_between(&receding.samples, 0.0, 0.05),
            "the echoes should be dying away"
        );
    }

    #[test]
    fn a_struck_plate_rings_at_inharmonic_ratios() {
        let modes = [Partial::new(1.0, 1.0, 0.5), Partial::new(2.76, 0.6, 0.35)];
        let ring = struck(0.6, 200.0, &modes);
        // Both partials are where they were asked to be, and the octave between
        // them is empty: that gap is what makes it metal rather than a note.
        let fundamental = ears::energy_at(&ring.samples, 200.0);
        let second = ears::energy_at(&ring.samples, 552.0);
        assert!(fundamental > 0.005, "no fundamental: {fundamental}");
        assert!(second > 0.001, "no second mode: {second}");
        assert!(
            ears::energy_at(&ring.samples, 400.0) < second * 0.05,
            "an inharmonic partial must not have an octave under it"
        );
    }

    #[test]
    fn a_mode_with_a_shorter_decay_is_gone_sooner() {
        // This is the property the whole metallic layer rests on: the modes of
        // a struck plate do not decay together, so the timbre changes as it
        // fades. If this fails, every clang in the game is a chord being faded.
        let modes = [Partial::new(1.0, 1.0, 1.0), Partial::new(3.0, 1.0, 0.15)];
        let ring = struck(0.8, 150.0, &modes);
        let early = ears::energy_at(&ring.samples[..4_000], 450.0);
        let late = ears::energy_at(&ring.samples[30_000..], 450.0);
        let low = ears::energy_at(&ring.samples[30_000..], 150.0);
        assert!(early > 0.01, "the fast mode never sounded: {early}");
        assert!(
            late < early * 0.02,
            "the third mode should be long gone by 0.7 s: {early} then {late}"
        );
        assert!(
            low > late * 10.0,
            "the slow mode should still be ringing where the fast one has gone: \
             {low} against {late}"
        );
    }

    #[test]
    fn debris_is_a_scatter_of_ticks_rather_than_one_burst() {
        let scatter = debris(0.9, 0.8, 12, 2_600.0, 1.5, 0x1234);
        assert_eq!(scatter.len(), (0.9 * SAMPLE_RATE as f32).ceil() as usize);
        let ticks = ears::transient_count(&scatter.samples, 256, 0.002, 2.0);
        assert!(ticks >= 6, "only {ticks} ticks came out of twelve");
        // And it thins out: the big pieces land first.
        assert!(
            ears::window_rms(&scatter.samples, 0.0, 0.4)
                > ears::window_rms(&scatter.samples, 0.6, 1.0),
            "a scatter should decay across its length"
        );
    }

    #[test]
    fn a_servo_winds_down() {
        let whine = servo(0.3, 0.8, 2_400.0, 1_200.0, 9);
        let early = ears::zero_crossing_rate(&whine.samples[..2_000]);
        let late = ears::zero_crossing_rate(&whine.samples[10_000..12_000]);
        assert!(
            early > late * 1.3,
            "a servo under load should slow down: {early} then {late}"
        );
        assert!(whine.samples.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn a_thump_drops_in_pitch() {
        let body = thump(180.0, 45.0, 0.2, 1.0, 2.0);
        let early = ears::zero_crossing_rate(&body.samples[..1_000]);
        let late = ears::zero_crossing_rate(&body.samples[7_000..8_000]);
        assert!(
            early > late * 1.8,
            "the body of an impact has to fall: {early} then {late}"
        );
        assert_eq!(body.samples[0], 0.0, "no click at the start");
    }

    #[test]
    fn layering_sums_its_layers_where_it_was_told_to() {
        let first = Buffer {
            samples: vec![1.0; 100],
        };
        let second = Buffer {
            samples: vec![1.0; 100],
        };
        let mix = layered(0.03, &[(first, 0.0, 0.5), (second, 0.01, 1.0)]);
        assert_eq!(mix.samples[0], 0.5);
        assert_eq!(mix.samples[99], 0.5, "the first layer is still going");
        assert_eq!(mix.samples[100], 0.0, "and then it has stopped");
        assert_eq!(mix.samples[441], 1.0, "the second layer starts at 10 ms");
        assert_eq!(mix.samples[540], 1.0);
        assert_eq!(mix.samples[541], 0.0);

        // A layer that runs past the stated duration makes the mix longer
        // rather than being truncated: a tail that is cut off is a click.
        let long = Buffer {
            samples: vec![1.0; 4_410],
        };
        let grown = layered(0.01, &[(long, 0.0, 1.0)]);
        assert_eq!(grown.len(), 4_410);
    }

    // -- the loops ----------------------------------------------------------

    #[test]
    fn a_closed_loop_is_the_loop_it_started_as() {
        // One second of a tone whose period divides the second exactly, handed
        // to `make_loopable` with a copy of its own head appended: the
        // cross-fade blends the signal with itself and must be an identity, or
        // the seam is a phase step in the middle of a sustained note.
        let mut body = tone(Wave::Sine, 8.0, 8.0, 1.0, 1.0);
        let head = body.samples[..441].to_vec();
        body.samples.extend_from_slice(&head);
        let before = body.samples[..44_100].to_vec();
        make_loopable(&mut body, 0.01);
        assert_eq!(body.len(), 44_100, "the fade is consumed, not added");
        for (i, sample) in before.iter().enumerate() {
            assert!(
                (sample - body.samples[i]).abs() < 1e-5,
                "sample {i} moved: {sample} -> {}",
                body.samples[i]
            );
        }
    }

    #[test]
    fn closing_a_loop_does_not_dip_its_noise() {
        // The reason `make_loopable` is equal-power rather than equal-gain: two
        // unrelated halves of a noise bed summed with weights that add to one
        // are 3 dB down in the middle of the fade, which is heard as a flutter
        // once a second for the whole mission.
        let mut bed = noise_bed(46_410, 0.5, 2_000.0, Shape::Band, 0.8, 0x99);
        let quiet = ears::window_rms(&bed.samples[20_000..25_000], 0.0, 1.0);
        make_loopable(&mut bed, 0.02);
        let seam = ears::window_rms(&bed.samples[..882], 0.0, 1.0);
        let elsewhere = ears::window_rms(&bed.samples[20_000..25_000], 0.0, 1.0);
        assert!(
            seam > elsewhere * 0.85,
            "the seam dips: {elsewhere} elsewhere, {seam} across the fade"
        );
        assert!(seam < elsewhere * 1.15, "and it must not swell either");
        assert!((quiet - elsewhere).abs() < quiet * 0.1, "sanity");
    }

    #[test]
    fn a_sustained_loop_has_no_step_at_its_seam() {
        for (name, looped) in loops() {
            let samples = &looped.samples;
            let wrap = (samples[0] - samples[samples.len() - 1]).abs();
            let internal = ears::max_step(samples);
            assert!(
                wrap <= internal * 1.5,
                "{name} jumps {wrap} at the seam, against {internal} anywhere else"
            );
        }
    }

    #[test]
    fn a_tone_that_does_not_divide_the_loop_cancels_at_the_seam() {
        // The negative control for the test below, and the reason the loops are
        // built out of whole-hertz partials: play a tone twice and the seam is
        // either continuous or it is a phase step, and a phase step shows up as
        // a tone that gets quieter the longer it is played.
        let raw = |hz: f32| {
            let mut buffer = Buffer {
                samples: vec![0.0; 44_100],
            };
            for (i, sample) in buffer.samples.iter_mut().enumerate() {
                let t = i as f32 / SAMPLE_RATE as f32;
                *sample = (TAU * hz * t).sin() * 0.8;
            }
            buffer
        };
        let coherence = |buffer: &Buffer, hz: f32| {
            let mut twice = buffer.clone();
            twice.samples.extend_from_slice(&buffer.samples);
            ears::energy_at(&twice.samples, hz) / ears::energy_at(&buffer.samples, hz)
        };
        // 57 cycles in a one-second loop: continuous.
        assert!(
            coherence(&raw(57.0), 57.0) > 0.95,
            "a tone that divides the loop should survive being looped"
        );
        // 57.4: the second playing starts 144 degrees out of phase with the
        // first, and the two partly cancel.
        assert!(
            coherence(&raw(57.4), 57.4) < 0.6,
            "a tone that does not divide the loop should visibly cancel"
        );
    }

    #[test]
    fn a_sustained_loop_holds_its_tone_across_the_seam() {
        // Playing a loop twice has to be the same as playing it once, for the
        // tonal layer. A partial that does not divide the loop length arrives
        // at the seam out of phase with itself, and the two halves then
        // interfere: the tone measured over two loops comes out *quieter* than
        // over one. That is a click in the middle of a note, and this is the
        // only test that catches it.
        //
        // The threshold is loose because the noise beds put their own energy in
        // the same bin, which moves the measurement by a few per cent in either
        // direction; the failure this catches moves it by seventy.
        let tones = [
            ("engine_bed", engine_bed(), 38.0),
            ("engine_load", engine_load(), 57.0),
            ("thruster_roar", thruster_roar(), 62.0),
            ("thruster_hiss", thruster_hiss(), 1_900.0),
        ];
        for (name, once, hz) in tones {
            let mut twice = once.clone();
            twice.samples.extend_from_slice(&once.samples);
            let single = ears::energy_at(&once.samples, hz);
            let doubled = ears::energy_at(&twice.samples, hz);
            assert!(
                doubled > single * 0.7,
                "{name}: the tone at {hz} Hz cancels across the seam ({single} then {doubled})"
            );
        }
    }

    #[test]
    fn the_engine_is_a_machine_and_not_a_waveform() {
        // The first pass at the engine was a sawtooth, and the way to say what
        // is wrong with that in a test is not "the harmonics are wrong" — a
        // stack of harmonics is a sawtooth — but that a sawtooth has *nothing
        // between* its harmonics and this has a great deal. The lope, the
        // detuned partners and the noise bed all put energy where a waveform
        // has none, and that is what a machine working sounds like.
        let engine = engine_bed();
        let saw = tone(Wave::Saw, 38.0, 38.0, 1.0, 0.8);
        let between = |buffer: &Buffer, hz: f32| {
            ears::energy_at(&buffer.samples, hz) / ears::energy_at(&buffer.samples, 38.0)
        };
        for hz in [31.0f32, 53.0] {
            let engine_between = between(&engine, hz);
            let saw_between = between(&saw, hz);
            assert!(
                engine_between > saw_between * 8.0,
                "at {hz} Hz the engine reads {engine_between} against a sawtooth's \
                 {saw_between}: there is nothing in it but the harmonic series"
            );
        }
        // And the stack is genuinely a stack: the second partial is comparable
        // with the first rather than an artefact of it.
        let fundamental = ears::energy_at(&engine.samples, 38.0);
        let second = ears::energy_at(&engine.samples, 76.0);
        assert!(
            second > fundamental * 0.25,
            "the second partial is missing: {second} against {fundamental}"
        );
    }

    // -- the sound of the set -----------------------------------------------

    #[test]
    fn every_sound_starts_and_ends_at_silence() {
        for (name, buffer) in one_shots().into_iter().chain(hud_sounds()) {
            assert_eq!(buffer.samples[0], 0.0, "{name} starts at a step");
            assert_eq!(
                buffer.samples[buffer.len() - 1],
                0.0,
                "{name} ends at a step"
            );
        }
    }

    #[test]
    fn nothing_in_the_set_clips_or_is_silent() {
        for (name, buffer) in one_shots().into_iter().chain(hud_sounds()).chain(loops()) {
            let peak = buffer.peak();
            assert!(
                buffer.samples.iter().all(|s| s.is_finite()),
                "{name} has a non-finite sample"
            );
            assert!(peak <= 0.95, "{name} peaks at {peak}");
            assert!(peak > 0.4, "{name} is quiet at {peak} for no reason");
            let mean = buffer.samples.iter().sum::<f32>() / buffer.len() as f32;
            assert!(mean.abs() < 0.01, "{name} has a DC offset of {mean}");
        }
    }

    #[test]
    fn a_crack_is_bright_and_an_explosion_is_dark() {
        // The two ends of the set, and the reason the set needs both: an
        // autocannon and an explosion that measure the same are two sounds the
        // player cannot tell apart in a firefight.
        let crack = autocannon(1);
        let boom = explosion(2);
        let crack_hz = ears::centroid_hz(&crack.samples);
        let boom_hz = ears::centroid_hz(&boom.samples);
        assert!(
            crack_hz > 1_200.0,
            "the autocannon is dull at {crack_hz} Hz"
        );
        assert!(boom_hz < 250.0, "the explosion is bright at {boom_hz} Hz");
        assert!(
            crack_hz > boom_hz * 5.0,
            "a crack at {crack_hz} Hz against a boom at {boom_hz} Hz is not a contrast"
        );
        // And the same claim as a proportion rather than as a weighted average:
        // a quarter of the gun is above 2 kHz and a five-hundredth of the
        // explosion is.
        let crack_high = ears::high_share(&crack.samples, 2_000.0);
        let boom_high = ears::high_share(&boom.samples, 2_000.0);
        assert!(crack_high > 0.15, "the gun has no top end: {crack_high}");
        assert!(
            boom_high < 0.02,
            "the explosion has too much top end: {boom_high}"
        );
    }

    #[test]
    fn the_two_guns_are_tellable_apart() {
        // The enemy's gun has to be distinguishable from the player's by ear
        // alone: lighter means higher, and shorter, and with less weight under
        // it. All three are measurable, and all three have to hold — a machine
        // gun that is only brighter is a machine gun that is only quieter.
        let mine = autocannon(1);
        let theirs = machine_gun(2);
        let mine_hz = ears::centroid_hz(&mine.samples);
        let theirs_hz = ears::centroid_hz(&theirs.samples);
        assert!(
            theirs_hz > mine_hz * 1.5,
            "the machine gun is not brighter than the autocannon: {theirs_hz} against {mine_hz}"
        );
        let mine_low = ears::band_rms(&mine.samples, 20.0, 120.0);
        let theirs_low = ears::band_rms(&theirs.samples, 20.0, 120.0);
        assert!(
            theirs_low < mine_low * 0.5,
            "the machine gun has as much weight as the autocannon: \
             {theirs_low} against {mine_low}"
        );
        assert!(
            ears::high_share(&theirs.samples, 2_000.0)
                > ears::high_share(&mine.samples, 2_000.0) * 1.5,
            "the lighter round does not have the sharper crack"
        );
        assert!(
            theirs.len() < mine.len(),
            "a lighter round is a shorter sound"
        );
    }

    #[test]
    fn a_hard_impact_rings_and_a_soft_one_does_not() {
        let hard = impact_hard(5, 1.0);
        let soft = impact_soft(6);
        // The ring is at the plate's fundamental, and the soft impact has
        // nothing there: no modes, no ring, no metal.
        let hard_ring = ears::energy_at(&hard.samples, 520.0);
        let soft_ring = ears::energy_at(&soft.samples, 520.0);
        assert!(
            hard_ring > 0.0005,
            "the hard impact does not ring: {hard_ring}"
        );
        assert!(
            hard_ring > soft_ring * 3.0,
            "the soft impact rings like a bell: {hard_ring} against {soft_ring}"
        );
        assert!(
            ears::centroid_hz(&hard.samples) > ears::centroid_hz(&soft.samples) * 2.0,
            "a hard surface is not brighter than a soft one"
        );
        // A ring outlives a thud, which is most of what the ear uses to tell
        // steel from soil at a distance.
        let hard_body = ears::rms_between(&hard.samples, 0.1, 0.2);
        let soft_body = ears::rms_between(&soft.samples, 0.1, 0.2);
        assert!(
            hard_body > soft_body * 2.0,
            "the hard impact should still be ringing where the soft one has stopped: \
             {hard_body} against {soft_body}"
        );
        assert!(
            ears::rms_between(&hard.samples, 0.3, 0.4) > 0.0005,
            "the ring should still be there at a third of a second"
        );
        assert!(hard.len() > soft.len() * 2, "a ring is longer than a thud");
    }

    #[test]
    fn the_two_hard_impacts_are_two_sizes_of_thing() {
        // The two takes of a hard impact are a railing and a container. The
        // difference has to be one the ear can name — a bigger thing rings lower
        // and longer — rather than a different sound at the same pitch.
        let small = impact_hard(5, 0.8);
        let large = impact_hard(5, 1.5);
        assert!(
            ears::centroid_hz(&small.samples) > ears::centroid_hz(&large.samples),
            "the small one is not the brighter one"
        );
        assert!(
            large.len() > small.len(),
            "the large one should ring longer"
        );
        assert_eq!(large.peak(), small.peak(), "takes arrive at the same level");
    }

    #[test]
    fn an_explosion_decays_over_about_a_second() {
        let boom = explosion(4);
        let first = ears::window_rms(&boom.samples, 0.0, 0.15);
        let middle = ears::window_rms(&boom.samples, 0.4, 0.55);
        let last = ears::window_rms(&boom.samples, 0.85, 1.0);
        assert!(
            boom.len() >= (1.0 * SAMPLE_RATE as f32) as usize,
            "the tail has to last about a second"
        );
        assert!(middle < first * 0.5, "the tail is not decaying");
        assert!(last < middle * 0.5, "the tail is not decaying at the end");
        assert!(
            last > first * 0.001,
            "the tail should still be there at the end, not cut off"
        );
        // Debris comes down through the tail rather than ending with the boom.
        // The counter looks for the level rising from one window to the next,
        // which is what a fragment landing on a decaying bed does.
        let ticks = ears::transient_count(&boom.samples, 256, 0.002, 2.5);
        assert!(ticks >= 6, "only {ticks} transients in the boom");
    }

    #[test]
    fn a_swing_is_air_and_a_hit_is_metal() {
        let swing = blade_swing(7);
        let hit = blade_hit(8);
        // The swing brightens as it comes through and darkens after it passes.
        let third = swing.len() / 3;
        assert!(
            ears::centroid_hz(&swing.samples[third..third * 2])
                > ears::centroid_hz(&swing.samples[..third]) * 3.0,
            "a swing should not start dark and stay dark"
        );
        // The hit rings at its modes: the plate's fundamental and its
        // inharmonic neighbours, and nothing at all in between. This is the
        // difference between a clang and a noise burst, and it is entirely
        // measurable.
        let pitch = BLADE_PITCH / blade_size(8);
        let fundamental = ears::energy_at(&hit.samples, pitch);
        let mode = ears::energy_at(&hit.samples, pitch * 1.59);
        let gap = ears::energy_at(&hit.samples, pitch * 1.35);
        assert!(fundamental > 0.0005, "no fundamental: {fundamental}");
        assert!(mode > 0.0001, "no second mode: {mode}");
        assert!(
            gap < fundamental * 0.05,
            "the gap between the modes is not empty: {gap} against {fundamental}"
        );
        // Air is broadband: the same probe that finds nothing in the clang finds
        // as much as its neighbours in the whoosh.
        let swing_gap =
            ears::energy_at(&swing.samples, pitch) / ears::energy_at(&swing.samples, pitch * 1.35);
        assert!(
            swing_gap < 10.0,
            "a whoosh should have no partials in it, but {pitch} Hz stands out by {swing_gap}"
        );
        // A struck plate goes on ringing long after the air has stopped moving.
        assert!(
            hit.len() > swing.len() * 2,
            "the ring should outlast the swing by more than its length"
        );
        assert!(
            ears::rms_between(&hit.samples, 0.5, 0.9) > 0.0002,
            "the hit has stopped ringing before the blade has stopped moving"
        );
    }

    #[test]
    fn a_footstep_has_a_thud_a_servo_and_a_scatter() {
        let step = footstep(0);
        assert!(
            ears::centroid_hz(&step.samples) < 900.0,
            "a step is not bright"
        );
        // The thud: the bottom octave is where the mass is.
        let weight = ears::band_rms(&step.samples, 20.0, 120.0);
        let metal = ears::band_rms(&step.samples, 3_500.0, 12_000.0);
        assert!(
            weight > metal * 2.5,
            "a footstep should be heavier than it is metallic: {weight} against {metal}"
        );
        // The servo: measured as the share of the sound that is above 1.5 kHz
        // against the same share of a bare thud, so the claim is about the leg
        // rather than about the level.
        let legs = ears::high_share(&step.samples, 1_500.0);
        let bare = ears::high_share(&thump(105.0, 36.0, 0.24, 0.95, 2.6).samples, 1_500.0);
        assert!(
            legs > 0.004 && legs > bare * 5.0,
            "the leg servo is inaudible above the thud: {legs} against {bare}"
        );
        // The scatter: more than one tick after the foot has landed.
        let ticks = ears::transient_count(&step.samples, 256, 0.002, 2.0);
        assert!(ticks >= 3, "only {ticks} transients in a footstep");
    }

    #[test]
    fn the_two_feet_are_not_the_same_foot() {
        let left = footstep(0);
        let right = footstep(1);
        assert_eq!(left.len(), right.len());
        let difference = ears::difference(&left.samples, &right.samples);
        assert!(
            difference > 0.3,
            "the two steps are nearly identical: {difference}"
        );
        assert!(
            (left.peak() - right.peak()).abs() < 0.01,
            "both feet should be as heavy as each other"
        );
    }

    #[test]
    fn the_hud_is_short_clean_and_quiet() {
        for (name, buffer) in hud_sounds() {
            assert!(
                buffer.len() <= (1.5 * SAMPLE_RATE as f32) as usize,
                "{name} is {} seconds long, which is a tune",
                buffer.len() as f32 / SAMPLE_RATE as f32
            );
            assert!(
                ears::centroid_hz(&buffer.samples) < 2_500.0,
                "{name} is bright enough to be tiring"
            );
            assert!(
                buffer.peak() <= PEAK_UI + 1e-3,
                "{name} is as loud as a weapon"
            );
        }
        for (name, buffer) in [
            ("ui_click", ui_click()),
            ("energy_warning", energy_warning()),
            ("lock_acquired", lock_acquired()),
            ("lock_lost", lock_lost()),
            ("ui_confirm", ui_confirm()),
        ] {
            assert!(
                buffer.len() <= (0.3 * SAMPLE_RATE as f32) as usize,
                "{name} is {} ms long",
                buffer.len() as f32 / SAMPLE_RATE as f32 * 1000.0
            );
        }
    }

    #[test]
    fn sounds_that_repeat_come_in_different_takes() {
        // Two renders of the same recipe with different seeds: the burst of
        // fire that plays the identical buffer six times a second is heard as a
        // loop, and the fix is a take rather than a volume.
        for (name, first, second) in [
            ("autocannon", autocannon(1), autocannon(2)),
            ("machine_gun", machine_gun(1), machine_gun(2)),
            ("blade_hit", blade_hit(8), blade_hit(9)),
            ("footstep", footstep(0), footstep(1)),
        ] {
            assert_eq!(
                first.len(),
                second.len(),
                "{name}: takes must be the same length"
            );
            assert!(
                (first.peak() - second.peak()).abs() < 1e-4,
                "{name}: takes arrive at different levels: {} against {}",
                first.peak(),
                second.peak()
            );
            let difference = ears::difference(&first.samples, &second.samples);
            assert!(
                difference > 0.15,
                "{name}: the takes are the same take: {difference}"
            );
            // And they are the same *kind* of sound: a take is a different
            // performance, not a different weapon.
            let (one, two) = (
                ears::centroid_hz(&first.samples),
                ears::centroid_hz(&second.samples),
            );
            assert!(
                (one / two - 1.0).abs() < 0.5,
                "{name}: one take is a different sound rather than the same sound again \
                 ({one} against {two})"
            );
        }
    }

    #[test]
    fn the_same_seed_renders_the_same_sound() {
        // The property the original could not have: `Math.random` meant no
        // assertion about a rifle shot was possible from one run to the next.
        assert_eq!(autocannon(3).samples, autocannon(3).samples);
        assert_eq!(explosion(4).samples, explosion(4).samples);
        assert_eq!(footstep(1).samples, footstep(1).samples);
        assert_ne!(autocannon(3).samples, autocannon(4).samples);
    }

    #[test]
    fn the_level_each_family_arrives_at_is_the_level_it_was_designed_for() {
        // Every one-shot peaks at the same level, and every HUD sound at a
        // quieter one. Without this, the loudest recipe in the set sets the
        // volume of the whole game.
        for (name, buffer) in one_shots() {
            let peak = buffer.peak();
            assert!(
                (peak - PEAK_ONE_SHOT).abs() < 0.02,
                "{name} peaks at {peak} rather than at {PEAK_ONE_SHOT}"
            );
        }
        for (name, buffer) in hud_sounds() {
            let peak = buffer.peak();
            assert!(
                (peak - PEAK_UI).abs() < 0.02,
                "{name} peaks at {peak} rather than at {PEAK_UI}"
            );
        }
    }

    #[test]
    fn only_the_loops_are_loopable() {
        // A one-shot that is normalised to the loop level would be quieter than
        // everything else, and a loop normalised to the one-shot level would
        // sit over the top of the mix for the whole mission.
        for (name, buffer) in loops() {
            assert!(
                (buffer.peak() - PEAK_LOOP).abs() < 0.02,
                "{name} peaks at {} rather than at {PEAK_LOOP}",
                buffer.peak()
            );
            assert_eq!(
                buffer.len(),
                (LOOP_SECONDS * SAMPLE_RATE as f32) as usize,
                "{name} is not exactly one second long"
            );
        }
    }

    #[test]
    fn the_whole_set_fits_the_budget() {
        // Load time is a real constraint: everything here is rendered before
        // the first frame, so the set has a ceiling in seconds of audio, and
        // the ceiling is asserted rather than remembered.
        let set: Vec<(&'static str, Buffer)> = one_shots()
            .into_iter()
            .chain(hud_sounds())
            .chain(loops())
            .collect();
        let seconds: f32 = set
            .iter()
            .map(|(_, buffer)| buffer.len() as f32 / SAMPLE_RATE as f32)
            .sum();
        let bytes: usize = set.iter().map(|(_, buffer)| buffer.to_pcm16().len()).sum();
        println!(
            "the set is {seconds:.2} s of audio, {} kB of PCM, at {} Hz",
            bytes / 1024,
            SAMPLE_RATE
        );
        assert!(
            seconds < 24.0,
            "the set is {seconds} seconds of audio to render at load"
        );
        assert!(
            bytes < 2_200_000,
            "the set is {} kB of PCM to hold for the session",
            bytes / 1024
        );
    }

    #[test]
    fn the_set_renders_in_the_time_a_load_can_afford() {
        // The budget above is about audio; this is about the clock. A debug
        // build is the slow case, and even a very slow machine renders the whole
        // set in well under a second — the assertion is loose on purpose, since
        // it is here to catch a recipe that has become accidentally quadratic,
        // not to measure the machine.
        let start = std::time::Instant::now();
        let set: Vec<Buffer> = one_shots()
            .into_iter()
            .chain(hud_sounds())
            .chain(loops())
            .map(|(_, buffer)| buffer)
            .collect();
        let elapsed = start.elapsed();
        println!(
            "the set renders in {:.0} ms",
            elapsed.as_secs_f32() * 1_000.0
        );
        assert!(!set.is_empty());
        assert!(
            elapsed.as_secs_f32() < 10.0,
            "rendering the set took {elapsed:?}"
        );
    }

    #[test]
    fn a_wav_file_can_be_written_for_a_person_to_listen_to() {
        let buffer = autocannon(1);
        let bytes = ears::wav_bytes(&buffer);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(bytes.len(), buffer.len() * 2 + 44);
        let declared = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        assert_eq!(declared, bytes.len() - 8, "the header must describe itself");
    }

    #[test]
    fn the_set_can_be_written_to_disk_for_listening() {
        // Set ASHFRAME_DUMP_SOUNDS=1 to render every sound into `target/sounds`
        // as a WAV file, or set it to a directory to put them somewhere else.
        // This is what stands in for ears: the measurements above say a sound is
        // bright and decays, and a person listening to the file says whether it
        // is any good.
        let Ok(setting) = std::env::var("ASHFRAME_DUMP_SOUNDS") else {
            return;
        };
        // A test runs with the package directory as its working directory, so
        // "target" here would mean `crates/ashframe/target` and leave a
        // directory of WAV files in the source tree. The workspace's own target
        // directory is two levels up and already ignored by git.
        let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let directory = if setting.is_empty() || setting == "1" {
            workspace.join("target/sounds")
        } else {
            std::path::PathBuf::from(setting)
        };
        std::fs::create_dir_all(&directory).expect("a directory to write into");
        let mut written = 0;
        for (name, buffer) in one_shots().into_iter().chain(hud_sounds()).chain(loops()) {
            let path = directory.join(format!("{name}.wav"));
            std::fs::write(&path, ears::wav_bytes(&buffer)).expect("a writable file");
            written += 1;
        }
        println!("wrote {written} sounds to {}", directory.display());
        assert!(written > 20);
    }
}
