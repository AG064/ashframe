//! Damage, stagger and death.
//!
//! Ported from the original's `game/combat.ts`, which said: *"All destruction
//! flows through `applyDamage` so stability thresholds, the single-stagger rule
//! and death handling exist in exactly one place."*
//!
//! That is why this is a free function over a [`Damageable`] rather than a
//! method on the player or on an enemy. Every hit in the game — autocannon,
//! missile splash, blade, enemy fire, the boss's slam — arrives here, so the
//! rules about when something staggers and when it dies are stated once.

use crate::config::player as cfg;
use crate::types::{Damageable, Faction, Hooks, SimEvent};

/// How long an enemy stays staggered once its stability breaks.
///
/// The player's duration is in the config table because it is part of how the
/// game feels to play. This one is not, and is kept as the original wrote it:
/// long enough to be a window, short enough that it is not a stun-lock.
const ENEMY_STAGGER_DURATION: f32 = 1.6;

/// How much more damage a staggered enemy takes.
///
/// The original expressed this through a `vulnerable` local whose non-staggered
/// branch was unreachable for enemies, so the number appeared only in the
/// branch that ran. Kept as written, with a name, because an unexplained 1.35
/// in the middle of a damage calculation is the kind of thing that gets
/// "tidied" into a config value and quietly changes the game.
const ENEMY_STAGGER_DAMAGE_SCALE: f32 = 1.35;

/// What one damage instance did.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DamageResult {
    pub destroyed: bool,
    pub staggered: bool,
    /// Damage actually applied, after vulnerability scaling.
    pub dealt: f32,
}

/// The state combat is allowed to change, borrowed off the mech that owns it.
///
/// The original had one mech object that satisfied the damageable interface
/// directly, so a hit changed the mech. Here a mech is a `Player` or an `Enemy`
/// and [`Damageable`] is a snapshot of one, handed to the projectile system as a
/// slice. Damage lands on the snapshots, and this is the return trip.
///
/// It deliberately does *not* carry position or velocity. By the time damage is
/// folded back the mech has already moved this step, and it is the authority on
/// where it is; letting a stale snapshot write its position back would make
/// every unit that took a hit stutter backwards a frame.
pub struct CombatFields<'a> {
    pub health: &'a mut f32,
    pub stability: &'a mut f32,
    pub alive: &'a mut bool,
    pub stagger_timer: &'a mut f32,
    pub invuln_timer: &'a mut f32,
    pub stagger_armed: &'a mut bool,
}

/// Fold combat results from a snapshot back into the mech it was taken from.
///
/// The destructuring is the point: adding a field to [`Damageable`] stops this
/// compiling, so a new piece of combat state cannot be silently lost on the way
/// back. The ignored bindings are the fields the mech, not combat, is the
/// authority for.
pub fn absorb(updated: &Damageable, into: CombatFields<'_>) {
    let Damageable {
        health,
        stability,
        alive,
        stagger_timer,
        invuln_timer,
        stagger_armed,
        // Identity, placement and the maxima belong to the mech.
        id: _,
        faction: _,
        name: _,
        pos: _,
        vel: _,
        radius: _,
        height: _,
        max_health: _,
        max_stability: _,
    } = updated;

    *into.health = *health;
    *into.stability = *stability;
    *into.alive = *alive;
    *into.stagger_timer = *stagger_timer;
    *into.invuln_timer = *invuln_timer;
    *into.stagger_armed = *stagger_armed;
}

/// How much to scale this hit by, before the caller's own multiplier.
///
/// Split out because it is the whole of the vulnerability rule and is worth
/// being able to test on its own.
fn vulnerability(target: &Damageable) -> f32 {
    if !target.staggered() {
        return 1.0;
    }
    if target.faction == Faction::Player {
        cfg::STAGGER_DAMAGE_SCALE
    } else {
        ENEMY_STAGGER_DAMAGE_SCALE
    }
}

/// Apply one damage instance.
///
/// A staggered target takes extra damage. Crossing the stability threshold
/// triggers **exactly one** stagger: the flag is not re-armed until the stagger
/// expires, so a burst of hits cannot retrigger it on every step — which would
/// hold a target permanently stunned and make the burst weapons win every
/// fight.
///
/// Impact is ignored while staggered, because stability is already full and
/// adding to it would only matter at the moment it re-arms.
pub fn apply_damage(
    target: &mut Damageable,
    amount: f32,
    impact: f32,
    hooks: &mut dyn Hooks,
    scale: f32,
) -> DamageResult {
    if !target.alive || target.invuln_timer > 0.0 {
        return DamageResult::default();
    }

    let dealt = amount * vulnerability(target) * scale;
    target.health -= dealt;

    hooks.emit(&SimEvent::Hit {
        at: target.pos + crate::types::Vec3::new(0.0, target.height * 0.5, 0.0),
        target: target.id,
        amount: dealt,
        staggered: target.staggered(),
    });

    let mut staggered = false;
    if !target.staggered() && impact > 0.0 {
        target.stability += impact;
        if target.stability >= target.max_stability {
            // Clamped rather than left above the maximum, so a stability bar
            // reads full rather than overfull, and so the next hit starts from
            // a known place.
            target.stability = target.max_stability;
            target.stagger_timer = if target.faction == Faction::Player {
                cfg::STAGGER_DURATION
            } else {
                ENEMY_STAGGER_DURATION
            };
            staggered = true;
            hooks.emit(&SimEvent::Stagger {
                target: target.id,
                at: target.pos + crate::types::Vec3::new(0.0, target.height * 0.5, 0.0),
            });
        }
    }

    if target.health <= 0.0 {
        // Clamped to zero so a health bar cannot read negative, and so a
        // results screen summing damage taken does not report more than the
        // frame ever had.
        target.health = 0.0;
        target.alive = false;
        hooks.emit(&SimEvent::Destroy {
            at: target.pos + crate::types::Vec3::new(0.0, target.height * 0.5, 0.0),
            target: target.id,
            kind: target.name,
        });
        return DamageResult {
            destroyed: true,
            staggered,
            dealt,
        };
    }

    DamageResult {
        destroyed: false,
        staggered,
        dealt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EventLog, Vec3};

    fn subject(faction: Faction, health: f32, stability: f32) -> Damageable {
        Damageable {
            id: 1,
            faction,
            name: "target",
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            radius: 1.0,
            height: 4.0,
            health,
            max_health: health,
            stability: 0.0,
            max_stability: stability,
            alive: true,
            stagger_timer: 0.0,
            invuln_timer: 0.0,
            stagger_armed: true,
        }
    }

    #[test]
    fn damage_comes_off_the_health() {
        let mut target = subject(Faction::Enemy, 100.0, 100.0);
        let mut log = EventLog::new();
        let result = apply_damage(&mut target, 25.0, 0.0, &mut log, 1.0);
        assert_eq!(target.health, 75.0);
        assert_eq!(result.dealt, 25.0);
        assert!(!result.destroyed);
        assert_eq!(log.count("hit"), 1);
    }

    #[test]
    fn a_dead_target_takes_nothing_more() {
        // Otherwise a corpse can be shot for score, or destroyed twice.
        let mut target = subject(Faction::Enemy, 100.0, 100.0);
        target.alive = false;
        let mut log = EventLog::new();
        let result = apply_damage(&mut target, 25.0, 10.0, &mut log, 1.0);
        assert_eq!(result, DamageResult::default());
        assert_eq!(target.health, 100.0);
        assert!(log.events.is_empty());
    }

    #[test]
    fn an_invulnerable_target_takes_nothing() {
        // The respawn window. Without it a player is shot on the frame they
        // reappear, which reads as the game being broken rather than hard.
        let mut target = subject(Faction::Player, 100.0, 100.0);
        target.invuln_timer = 1.0;
        let mut log = EventLog::new();
        assert_eq!(
            apply_damage(&mut target, 25.0, 10.0, &mut log, 1.0),
            DamageResult::default()
        );
        assert_eq!(target.health, 100.0);
    }

    // -- stagger ----------------------------------------------------------

    #[test]
    fn crossing_the_stability_threshold_staggers_once() {
        let mut target = subject(Faction::Enemy, 1000.0, 100.0);
        let mut log = EventLog::new();

        let first = apply_damage(&mut target, 1.0, 60.0, &mut log, 1.0);
        assert!(!first.staggered, "60 of 100 is not yet a break");

        let second = apply_damage(&mut target, 1.0, 60.0, &mut log, 1.0);
        assert!(second.staggered, "the threshold was crossed");
        assert_eq!(target.stagger_timer, ENEMY_STAGGER_DURATION);
        assert_eq!(log.count("stagger"), 1);
    }

    #[test]
    fn a_burst_of_hits_does_not_retrigger_a_stagger() {
        // The rule the original called out by name. Without it, sustained fire
        // holds a target staggered forever and the burst weapons win every
        // fight without the player doing anything.
        let mut target = subject(Faction::Enemy, 10_000.0, 100.0);
        let mut log = EventLog::new();

        for _ in 0..50 {
            apply_damage(&mut target, 1.0, 50.0, &mut log, 1.0);
        }
        assert_eq!(
            log.count("stagger"),
            1,
            "a stagger may trigger once per threshold crossing, not once per hit"
        );
    }

    #[test]
    fn stability_is_clamped_at_the_maximum() {
        let mut target = subject(Faction::Enemy, 10_000.0, 100.0);
        let mut log = EventLog::new();
        apply_damage(&mut target, 1.0, 500.0, &mut log, 1.0);
        assert_eq!(target.stability, target.max_stability);
    }

    #[test]
    fn a_staggered_target_takes_extra_damage() {
        let mut staggered = subject(Faction::Enemy, 1000.0, 100.0);
        staggered.stagger_timer = 1.0;
        let mut fresh = subject(Faction::Enemy, 1000.0, 100.0);

        let mut log = EventLog::new();
        let hard = apply_damage(&mut staggered, 100.0, 0.0, &mut log, 1.0);
        let soft = apply_damage(&mut fresh, 100.0, 0.0, &mut log, 1.0);

        assert!(
            hard.dealt > soft.dealt,
            "a staggered enemy should be more vulnerable"
        );
        assert_eq!(soft.dealt, 100.0, "an unstaggered one takes the base");
    }

    #[test]
    fn the_player_and_enemies_have_different_vulnerability() {
        let mut enemy = subject(Faction::Enemy, 1000.0, 100.0);
        enemy.stagger_timer = 1.0;
        let mut player = subject(Faction::Player, 1000.0, 100.0);
        player.stagger_timer = 1.0;

        assert_eq!(vulnerability(&enemy), ENEMY_STAGGER_DAMAGE_SCALE);
        assert_eq!(vulnerability(&player), cfg::STAGGER_DAMAGE_SCALE);
        assert_ne!(
            vulnerability(&enemy),
            vulnerability(&player),
            "the original scales these differently and the port keeps that"
        );
    }

    #[test]
    fn the_player_staggers_for_the_configured_time() {
        let mut player = subject(Faction::Player, 1000.0, 100.0);
        let mut log = EventLog::new();
        apply_damage(&mut player, 1.0, 200.0, &mut log, 1.0);
        assert_eq!(player.stagger_timer, cfg::STAGGER_DURATION);
    }

    #[test]
    fn the_caller_scale_multiplies_through() {
        let mut target = subject(Faction::Enemy, 1000.0, 100.0);
        let mut log = EventLog::new();
        let result = apply_damage(&mut target, 100.0, 0.0, &mut log, 0.5);
        assert_eq!(result.dealt, 50.0);
    }

    // -- death ------------------------------------------------------------

    #[test]
    fn reaching_zero_health_destroys_exactly_once() {
        let mut target = subject(Faction::Enemy, 50.0, 100.0);
        let mut log = EventLog::new();

        let killing = apply_damage(&mut target, 50.0, 0.0, &mut log, 1.0);
        assert!(killing.destroyed);
        assert!(!target.alive);
        assert_eq!(target.health, 0.0);
        assert_eq!(log.count("destroy"), 1);

        // A second hit on the corpse does nothing at all.
        let after = apply_damage(&mut target, 50.0, 0.0, &mut log, 1.0);
        assert!(!after.destroyed);
        assert_eq!(log.count("destroy"), 1);
    }

    #[test]
    fn health_never_reads_negative() {
        // A results screen summing damage taken would otherwise report more
        // than the frame ever had, and a health bar would draw past its end.
        let mut target = subject(Faction::Enemy, 10.0, 100.0);
        let mut log = EventLog::new();
        apply_damage(&mut target, 500.0, 0.0, &mut log, 1.0);
        assert_eq!(target.health, 0.0);
    }

    #[test]
    fn a_killing_blow_can_also_be_the_staggering_blow() {
        // Both are reported, because the renderer plays an effect for each and
        // a destroyed target that never visibly staggered looks like a bug.
        let mut target = subject(Faction::Enemy, 10.0, 50.0);
        let mut log = EventLog::new();
        let result = apply_damage(&mut target, 500.0, 200.0, &mut log, 1.0);
        assert!(result.destroyed);
        assert!(result.staggered);
        assert_eq!(log.count("destroy"), 1);
        assert_eq!(log.count("stagger"), 1);
    }

    #[test]
    fn events_carry_the_midpoint_of_the_target() {
        // Effects are placed from these, so a hit reported at the feet would
        // put every spark on the floor.
        let mut target = subject(Faction::Enemy, 100.0, 100.0);
        target.pos = Vec3::new(3.0, 1.0, -2.0);
        target.height = 4.0;
        let mut log = EventLog::new();
        apply_damage(&mut target, 1.0, 0.0, &mut log, 1.0);

        match &log.events[0] {
            SimEvent::Hit { at, .. } => assert_eq!(*at, Vec3::new(3.0, 3.0, -2.0)),
            other => panic!("expected a hit, got {other:?}"),
        }
    }
}
