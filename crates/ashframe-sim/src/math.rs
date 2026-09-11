//! Small numeric helpers, shared by every part of the simulation.
//!
//! Ported from the original's `core/math.ts`. The behaviour is deliberately
//! identical, including the places where that behaviour is a little unusual —
//! a port that quietly "improves" the numbers produces a game that feels
//! different and gives no clue why.
//!
//! Every function here is `f32`, where the original used JavaScript's `f64`.
//! That is the one deliberate divergence, and it is safe for these: they are
//! all bounded interpolations whose results feed positions and timers, not
//! accumulations where a difference would compound. The random number
//! generator is the exception — see [`Rng`] — and it keeps `f64` internally so
//! that a seed produces the same stream it always did.

/// Hold `v` inside `lo..=hi`.
///
/// Order matters and is preserved from the original: a value below `lo` clamps
/// to `lo` even when `lo > hi`, which a `clamp` that checked both bounds at
/// once would not do. Nothing passes an inverted range today; matching the
/// original means nothing starts to.
pub fn clamp(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// Linear interpolation. `t` is not clamped; callers that need it clamped use
/// [`smoothstep`].
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Frame-rate independent exponential approach. `rate` is in 1/seconds.
///
/// This is the workhorse of the whole simulation: every cooldown, camera
/// follow and steering response goes through it, which is what makes the game
/// behave the same at 30 fps and 240.
pub fn damp(current: f32, target: f32, rate: f32, dt: f32) -> f32 {
    target + (current - target) * (-rate * dt).exp()
}

/// [`damp`] for angles, taking the short way round.
pub fn damp_angle(current: f32, target: f32, rate: f32, dt: f32) -> f32 {
    let mut delta = target - current;
    while delta > std::f32::consts::PI {
        delta -= std::f32::consts::PI * 2.0;
    }
    while delta < -std::f32::consts::PI {
        delta += std::f32::consts::PI * 2.0;
    }
    current + delta * (1.0 - (-rate * dt).exp())
}

/// Move `current` toward `target` by at most `max_delta`.
///
/// Reaches the target exactly rather than near it, which matters for timers
/// that are compared against zero.
pub fn approach(current: f32, target: f32, max_delta: f32) -> f32 {
    let d = target - current;
    if d.abs() <= max_delta {
        return target;
    }
    current + d.signum() * max_delta
}

/// Ease in and out across `0..=1`, clamped at both ends.
pub fn smoothstep(t: f32) -> f32 {
    let x = clamp(t, 0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// The deterministic generator the whole game draws from.
///
/// xorshift32, exactly as the original wrote it, because "deterministic" is a
/// promise about a *sequence*: any change to the algorithm changes every
/// mission that has ever been recorded, and the recorded ones are how a port
/// is checked.
///
/// The state is `u32` and the operations are Rust's, which are logical shifts
/// and wrapping arithmetic. Those match JavaScript's `x << 13` followed by
/// `x >>> 0` bit for bit — JavaScript shifts on a 32-bit value and discards
/// what falls off the top, which is what a `u32` shift does — so the two
/// produce the same stream rather than merely similar ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rng {
    state: u32,
}

/// The seed the original uses when none is given.
pub const DEFAULT_SEED: u32 = 0x9e37_79b9;

impl Default for Rng {
    fn default() -> Self {
        Self::new(DEFAULT_SEED)
    }
}

impl Rng {
    pub fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    /// The next value in `0.0..=1.0`.
    ///
    /// Divided in `f64` even though the result is `f32`. The original divides
    /// a 32-bit integer by `0xffffffff` in double precision, and doing that
    /// division in `f32` would round the numerator to 24 bits and collapse
    /// neighbouring values together — a generator that returns the same number
    /// twice as often as it should, in a way no test would notice.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        (f64::from(x) / f64::from(u32::MAX)) as f32
    }

    /// A value in `min..max`.
    pub fn range(&mut self, min: f32, max: f32) -> f32 {
        min + self.next() * (max - min)
    }

    /// A value in `-1.0..=1.0`.
    pub fn signed(&mut self) -> f32 {
        self.next() * 2.0 - 1.0
    }

    /// Pick one of `items`.
    ///
    /// # Panics
    ///
    /// If `items` is empty. The original would return `undefined` and let the
    /// caller fail somewhere else; failing here names the actual mistake.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        let index = ((self.next() as f64 * items.len() as f64) as usize).min(items.len() - 1);
        &items[index]
    }

    /// Restart the sequence from `seed`.
    pub fn reseed(&mut self, seed: u32) {
        self.state = seed;
    }

    /// The current state, so a run can be recorded and replayed.
    pub fn state(&self) -> u32 {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_holds_a_value_inside_its_range() {
        assert_eq!(clamp(5.0, 0.0, 10.0), 5.0);
        assert_eq!(clamp(-1.0, 0.0, 10.0), 0.0);
        assert_eq!(clamp(11.0, 0.0, 10.0), 10.0);
        assert_eq!(clamp(0.0, 0.0, 10.0), 0.0);
        assert_eq!(clamp(10.0, 0.0, 10.0), 10.0);
    }

    #[test]
    fn clamp_keeps_the_originals_bound_order() {
        // The low bound is checked first, so an inverted range clamps up
        // rather than down. Preserved rather than corrected: nothing passes an
        // inverted range, and matching the original means nothing starts to.
        assert_eq!(clamp(5.0, 10.0, 0.0), 10.0);
    }

    #[test]
    fn lerp_hits_both_ends_exactly() {
        assert_eq!(lerp(0.0, 10.0, 0.0), 0.0);
        assert_eq!(lerp(0.0, 10.0, 1.0), 10.0);
        assert_eq!(lerp(0.0, 10.0, 0.5), 5.0);
        assert_eq!(lerp(-5.0, 5.0, 0.5), 0.0);
    }

    #[test]
    fn damp_approaches_the_target_and_never_overshoots() {
        let mut value = 0.0f32;
        let mut last = value;
        for _ in 0..240 {
            value = damp(value, 10.0, 8.0, 1.0 / 120.0);
            assert!(
                value >= last,
                "damp must move monotonically toward the target"
            );
            assert!(value <= 10.0, "damp must not overshoot, got {value}");
            last = value;
        }
        assert!(
            (value - 10.0).abs() < 0.01,
            "should have arrived, got {value}"
        );
    }

    #[test]
    fn damp_is_frame_rate_independent() {
        // The property the whole simulation rests on: the same elapsed time
        // produces the same result however it was divided up.
        let coarse = {
            let mut v = 0.0f32;
            for _ in 0..10 {
                v = damp(v, 10.0, 6.0, 0.1);
            }
            v
        };
        let fine = {
            let mut v = 0.0f32;
            for _ in 0..100 {
                v = damp(v, 10.0, 6.0, 0.01);
            }
            v
        };
        assert!(
            (coarse - fine).abs() < 0.05,
            "240 fps and 24 fps should agree: {coarse} vs {fine}"
        );
    }

    #[test]
    fn damp_angle_takes_the_short_way_round() {
        // From just under pi to just over minus pi is a small step forward, not
        // a full turn back.
        let current = std::f32::consts::PI - 0.1;
        let target = -std::f32::consts::PI + 0.1;
        let result = damp_angle(current, target, 10.0, 0.1);
        assert!(
            result > current,
            "should have continued forward past pi, got {result}"
        );
    }

    #[test]
    fn damp_angle_stays_within_one_turn_of_the_target() {
        for target in [-10.0f32, -3.0, 0.0, 3.0, 10.0] {
            let result = damp_angle(0.0, target, 5.0, 0.5);
            assert!(
                result.abs() < 20.0,
                "unbounded result {result} for {target}"
            );
        }
    }

    #[test]
    fn approach_lands_exactly_on_the_target() {
        assert_eq!(approach(0.0, 10.0, 1.0), 1.0);
        assert_eq!(approach(9.5, 10.0, 1.0), 10.0);
        assert_eq!(approach(10.5, 10.0, 1.0), 10.0);
        assert_eq!(approach(0.0, 0.0, 1.0), 0.0);
    }

    #[test]
    fn approach_works_with_a_negative_target() {
        assert_eq!(approach(0.0, -10.0, 1.0), -1.0);
        assert_eq!(approach(-9.5, -10.0, 1.0), -10.0);
    }

    #[test]
    fn smoothstep_is_clamped_and_symmetric() {
        assert_eq!(smoothstep(-1.0), 0.0);
        assert_eq!(smoothstep(0.0), 0.0);
        assert_eq!(smoothstep(1.0), 1.0);
        assert_eq!(smoothstep(2.0), 1.0);
        assert!((smoothstep(0.5) - 0.5).abs() < 1e-6);
        for t in [0.1f32, 0.25, 0.4] {
            let a = smoothstep(t);
            let b = smoothstep(1.0 - t);
            assert!((a + b - 1.0).abs() < 1e-5, "not symmetric at {t}");
        }
    }

    // -- the generator ----------------------------------------------------

    #[test]
    fn the_same_seed_gives_the_same_sequence() {
        // The promise everything else depends on. Recorded runs are how a port
        // is checked, and they only replay if this holds.
        let mut a = Rng::new(12345);
        let mut b = Rng::new(12345);
        for step in 0..1000 {
            assert_eq!(a.next(), b.next(), "diverged at step {step}");
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = Rng::new(1);
        let mut b = Rng::new(2);
        let differ = (0..100).any(|_| a.next() != b.next());
        assert!(differ, "two seeds produced identical streams");
    }

    #[test]
    fn reseeding_restarts_the_sequence() {
        let mut rng = Rng::new(7);
        let first: Vec<f32> = (0..10).map(|_| rng.next()).collect();
        let _ = rng.next();
        rng.reseed(7);
        let again: Vec<f32> = (0..10).map(|_| rng.next()).collect();
        assert_eq!(first, again);
    }

    #[test]
    fn values_stay_inside_the_unit_interval() {
        let mut rng = Rng::new(DEFAULT_SEED);
        for _ in 0..100_000 {
            let value = rng.next();
            assert!((0.0..=1.0).contains(&value), "out of range: {value}");
        }
    }

    #[test]
    fn the_sequence_does_not_settle_on_one_value() {
        // xorshift32 has a single dead state at zero, and it is reachable from
        // a seed of zero. This records what happens so the behaviour is known
        // rather than discovered in a mission that mysteriously stops moving.
        let mut rng = Rng::new(0);
        let first = rng.next();
        assert!(
            (0.0..=1.0).contains(&first),
            "a zero seed still returns a value in range"
        );

        // From the default seed, values must keep varying.
        let mut rng = Rng::new(DEFAULT_SEED);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            seen.insert(rng.next().to_bits());
        }
        assert!(seen.len() > 990, "the stream is repeating too early");
    }

    #[test]
    fn range_and_signed_respect_their_bounds() {
        let mut rng = Rng::new(99);
        for _ in 0..10_000 {
            let r = rng.range(-3.0, 7.0);
            assert!((-3.0..=7.0).contains(&r), "range out of bounds: {r}");
            let s = rng.signed();
            assert!((-1.0..=1.0).contains(&s), "signed out of bounds: {s}");
        }
    }

    #[test]
    fn ranged_values_are_spread_across_the_whole_interval() {
        // A generator that only touched the low end would still satisfy the
        // bounds check above.
        let mut rng = Rng::new(4242);
        let mut low = 0;
        let mut high = 0;
        for _ in 0..10_000 {
            if rng.next() < 0.5 {
                low += 1;
            } else {
                high += 1;
            }
        }
        let ratio = f64::from(low) / f64::from(low + high);
        assert!(
            (0.45..0.55).contains(&ratio),
            "the split is lopsided: {ratio}"
        );
    }

    #[test]
    fn pick_returns_an_element_of_the_slice() {
        let items = ["a", "b", "c"];
        let mut rng = Rng::new(5);
        for _ in 0..1000 {
            let picked = rng.pick(&items);
            assert!(items.contains(picked), "picked something not in the list");
        }
    }

    #[test]
    fn pick_can_reach_the_last_element() {
        // The original clamps with `Math.min(length - 1, floor(next * length))`,
        // so the final element is reachable rather than excluded by a rounding
        // accident when `next()` returns exactly 1.
        let items = [0, 1, 2];
        let mut rng = Rng::new(1);
        let mut saw_last = false;
        for _ in 0..1000 {
            if rng.pick(&items) == &2 {
                saw_last = true;
                break;
            }
        }
        assert!(saw_last, "the last element was never chosen");
    }

    #[test]
    fn state_can_be_read_and_restored() {
        // What a replay needs.
        let mut rng = Rng::new(2024);
        for _ in 0..37 {
            rng.next();
        }
        let saved = rng.state();
        let expected: Vec<f32> = (0..20).map(|_| rng.next()).collect();

        let mut restored = Rng::new(saved);
        let actual: Vec<f32> = (0..20).map(|_| restored.next()).collect();
        assert_eq!(expected, actual);
    }
}
