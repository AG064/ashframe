//! Every gameplay number, in one place.
//!
//! Ported from the original's `config.ts`, which said of itself: *"Every
//! gameplay number lives here so movement, combat, mission pacing and
//! presentation can be tuned together. Nothing else in the codebase should
//! carry a bare gameplay constant."*
//!
//! That rule is worth keeping literally. A number that matters and lives at
//! its point of use is a number nobody can find when the game feels wrong, and
//! a constant that appears in two files is a constant that will disagree with
//! itself within a month.
//!
//! Values are unchanged, to the digit. Tuning is the original author's, and a
//! port is not the place to second-guess it — especially before the game runs
//! and there is anything to judge it by.

/// Timing. The simulation runs at a fixed rate; rendering never drives it.
pub mod sim {
    /// Steps per second.
    pub const HZ: u32 = 120;
    /// Seconds per step, as a literal so it is exactly the same number
    /// everywhere rather than a division recomputed at each use.
    pub const DT: f32 = 1.0 / 120.0;
    /// Longest frame the loop will try to catch up on, in seconds.
    ///
    /// A frame longer than this — a breakpoint, a backgrounded window — is
    /// clamped rather than replayed. Simulating four seconds of catch-up after
    /// a stall produces a burst of movement the player never asked for and
    /// cannot react to.
    pub const MAX_CATCH_UP: f32 = 0.25;
    /// Hard cap on steps per rendered frame, for the same reason.
    pub const MAX_STEPS: u32 = 24;
}

/// The arena.
pub mod world {
    /// Half-extent of the floor. The playable area is roughly 300 × 300 metres.
    pub const HALF: f32 = 150.0;
    /// Fall below this and the mech is recovered to the nearest safe pad.
    pub const KILL_PLANE: f32 = -30.0;
    pub const GRAVITY: f32 = -26.0;
    /// Fixed arena floor height.
    pub const FLOOR_Y: f32 = 0.0;
    /// How tall a ledge a hostile will walk over rather than into.
    ///
    /// Shared between the collision resolver, which ignores anything below it,
    /// and the ground probe, which is what actually places the unit on top.
    /// They have to agree: the resolver deliberately declines to block a low
    /// ledge, so if the probe cannot see it the unit walks through it instead.
    pub const ENEMY_STEP_HEIGHT: f32 = 1.0;
}

/// The player's frame.
pub mod player {
    pub const HEIGHT: f32 = 6.0;
    pub const RADIUS: f32 = 1.7;
    /// Eye and anchor offset above the feet.
    pub const CENTRE_Y: f32 = 3.0;

    pub const GLIDE_SPEED: f32 = 22.0;
    pub const GLIDE_ACCEL: f32 = 78.0;
    pub const GLIDE_DECEL: f32 = 60.0;
    /// Extra braking when input opposes the current velocity.
    pub const TURN_BRAKE: f32 = 34.0;

    pub const AIR_ACCEL: f32 = 34.0;
    pub const AIR_DRAG: f32 = 0.6;
    pub const AIR_MAX_SPEED: f32 = 26.0;
    pub const FALL_MAX: f32 = 55.0;
    pub const JUMP_SPEED: f32 = 15.0;
    /// Sustained ascent while thrust is held in the air.
    pub const FLIGHT_THRUST: f32 = 46.0;
    pub const FLIGHT_MAX_UP: f32 = 20.0;
    /// Descent is capped while thrusting, so flight feels controlled rather
    /// than like falling with a light on.
    pub const FLIGHT_FALL_CAP: f32 = -12.0;

    pub const QUICK_BOOST_SPEED: f32 = 48.0;
    pub const QUICK_BOOST_DURATION: f32 = 0.18;
    pub const QUICK_BOOST_COOLDOWN: f32 = 0.34;
    /// A dodge pressed just before landing still fires.
    pub const INPUT_BUFFER: f32 = 0.15;
    /// Grace after leaving an edge during which a jump still works.
    pub const COYOTE_TIME: f32 = 0.12;
    /// How tall a ledge the mech walks over rather than into.
    ///
    /// A generous figure, so kerbs and ramp steps are walked over rather than
    /// caught on. It is also the height from which the ground probe looks for a
    /// surface: the probe has to start above the feet by at least a step, or a
    /// ledge the mech is walking onto would be invisible to it, and by no more
    /// than a step, or anything the mech is standing *under* would be mistaken
    /// for the floor.
    pub const STEP_HEIGHT: f32 = 1.25;

    pub const ASSAULT_SPEED: f32 = 40.0;
    pub const ASSAULT_ACCEL: f32 = 90.0;
    /// Yaw and pitch authority multiplier while assault boosting.
    pub const ASSAULT_TURN_SCALE: f32 = 0.45;
    /// Forward lift, so an assault boost can be angled slightly up.
    pub const ASSAULT_PITCH_LIMIT: f32 = 0.35;

    pub const ENERGY_MAX: f32 = 100.0;
    pub const ENERGY_QUICK_BOOST: f32 = 20.0;
    pub const ENERGY_FLIGHT_DRAIN: f32 = 18.0;
    pub const ENERGY_ASSAULT_DRAIN: f32 = 24.0;
    pub const ENERGY_REGEN_GROUND: f32 = 25.0;
    pub const ENERGY_REGEN_AIR: f32 = 11.0;
    pub const ENERGY_REGEN_DELAY: f32 = 0.55;
    /// Below this the mech cannot start a boost or flight.
    pub const ENERGY_MIN_TO_BOOST: f32 = 8.0;

    pub const HEALTH_MAX: f32 = 1000.0;
    pub const STABILITY_MAX: f32 = 100.0;
    pub const STABILITY_DECAY_DELAY: f32 = 1.1;
    pub const STABILITY_DECAY_RATE: f32 = 42.0;
    /// How long the stagger window lasts once triggered.
    pub const STAGGER_DURATION: f32 = 1.35;
    /// Damage multiplier while staggered.
    pub const STAGGER_DAMAGE_SCALE: f32 = 1.5;

    pub const REPAIRS: u32 = 2;
    pub const REPAIR_AMOUNT: f32 = 420.0;
    pub const REPAIR_TIME: f32 = 1.5;

    /// Body lean is presentation only and never gates control.
    pub const LEAN_MAX: f32 = 0.34;
    pub const LEAN_RATE: f32 = 7.5;
    pub const TORSO_LAG: f32 = 6.0;

    pub const RESPAWN_INVULN: f32 = 2.0;
}

/// The chase camera.
pub mod camera {
    pub const FOV: f32 = 68.0;
    /// FOV added at full assault boost.
    pub const FOV_BOOST: f32 = 9.0;
    pub const DISTANCE: f32 = 13.5;
    /// Tighter and over the shoulder while locked on.
    pub const LOCK_DISTANCE: f32 = 11.5;
    pub const HEIGHT: f32 = 4.6;
    pub const SHOULDER: f32 = 2.1;
    /// Follow rates in 1/seconds. Higher is snappier.
    pub const FOLLOW_RATE: f32 = 14.0;
    pub const AIM_FOLLOW_RATE: f32 = 26.0;
    pub const MIN_PITCH: f32 = -0.62;
    pub const MAX_PITCH: f32 = 1.05;
    /// Probe radius for the pull-in that keeps walls from swallowing it.
    pub const PROBE_RADIUS: f32 = 1.2;
    pub const MIN_DISTANCE: f32 = 3.2;
    pub const SHAKE_DECAY: f32 = 5.5;
    pub const MOUSE_SENSITIVITY: f32 = 0.0022;
}

/// The autocannon.
pub mod rifle {
    pub const NAME: &str = "VK-40 AUTOCANNON";
    pub const DAMAGE: f32 = 26.0;
    pub const IMPACT: f32 = 6.0;
    /// Rounds per second.
    pub const RPM: f32 = 7.2;
    pub const MAGAZINE: u32 = 42;
    pub const RELOAD_TIME: f32 = 1.75;
    pub const SPREAD: f32 = 0.011;
    /// Muzzle velocity. Projectiles are swept, so this cannot tunnel.
    pub const SPEED: f32 = 300.0;
    pub const RANGE: f32 = 420.0;
    pub const RECOIL: f32 = 0.5;
    pub const TRACER_EVERY: u32 = 1;
}

/// The missile pod.
pub mod missiles {
    pub const NAME: &str = "HR-6 MISSILE POD";
    pub const COUNT: u32 = 6;
    /// Delay between launches within one volley.
    pub const LAUNCH_INTERVAL: f32 = 0.09;
    pub const DAMAGE: f32 = 46.0;
    pub const IMPACT: f32 = 16.0;
    pub const SPLASH_RADIUS: f32 = 5.5;
    pub const SPLASH_DAMAGE: f32 = 22.0;
    pub const SPEED: f32 = 52.0;
    /// Radians per second the missile can turn.
    pub const TURN_RATE: f32 = 2.5;
    pub const LIFE: f32 = 6.0;
    pub const COOLDOWN: f32 = 7.5;
    /// A lock must be held before the pod will fire.
    pub const LOCK_REQUIRED: bool = true;
    /// Distance at which the warhead self-detonates harmlessly.
    pub const PROXIMITY_FUSE: f32 = 2.2;
}

/// The energy blade.
pub mod blade {
    pub const NAME: &str = "KS-9 ARC BLADE";
    pub const DAMAGE: f32 = 210.0;
    pub const IMPACT: f32 = 46.0;
    /// Reach of the sweep from the mech centre.
    pub const RANGE: f32 = 9.5;
    /// Half-angle of the sweep, in radians.
    pub const ARC: f32 = 1.15;
    /// Vertical tolerance for a hit.
    pub const VERTICAL: f32 = 5.0;
    pub const WINDUP: f32 = 0.19;
    /// Length of the damage window.
    pub const ACTIVE: f32 = 0.16;
    pub const RECOVERY: f32 = 0.27;
    pub const COOLDOWN: f32 = 1.0;
    pub const LUNGE: f32 = 14.0;
}

/// The S-2 Skirmisher: fast, strafing, fires in bursts.
pub mod skirmisher {
    pub const NAME: &str = "S-2 SKIRMISHER";
    pub const HEALTH: f32 = 240.0;
    pub const STABILITY: f32 = 60.0;
    pub const HEIGHT: f32 = 4.6;
    pub const RADIUS: f32 = 1.35;
    pub const SPEED: f32 = 17.0;
    pub const STRAFE_SPEED: f32 = 13.0;
    pub const ACCEL: f32 = 42.0;
    pub const PREFERRED_RANGE: f32 = 34.0;
    pub const BURST_COUNT: u32 = 3;
    pub const BURST_INTERVAL: f32 = 0.13;
    pub const BURST_COOLDOWN: f32 = 2.3;
    pub const DAMAGE: f32 = 15.0;
    pub const PROJECTILE_SPEED: f32 = 105.0;
    pub const PROJECTILE_LIFE: f32 = 2.4;
    /// Cone of fire inaccuracy, in radians.
    pub const SPREAD: f32 = 0.045;
    pub const SIGHT: f32 = 130.0;
    pub const REPAIR_DROP: u32 = 0;
}

/// The M-8 Siege Unit: slow, and telegraphs before it lands a salvo.
pub mod artillery {
    pub const NAME: &str = "M-8 SIEGE UNIT";
    pub const HEALTH: f32 = 420.0;
    pub const STABILITY: f32 = 95.0;
    pub const HEIGHT: f32 = 5.4;
    pub const RADIUS: f32 = 1.9;
    pub const SPEED: f32 = 5.5;
    pub const ACCEL: f32 = 18.0;
    pub const PREFERRED_RANGE: f32 = 70.0;
    /// Telegraph length before the barrage lands.
    pub const TELEGRAPH: f32 = 1.15;
    pub const SALVO_COUNT: u32 = 5;
    pub const SALVO_INTERVAL: f32 = 0.22;
    pub const COOLDOWN: f32 = 4.2;
    pub const DAMAGE: f32 = 30.0;
    pub const SPLASH_RADIUS: f32 = 7.0;
    pub const PROJECTILE_SPEED: f32 = 62.0;
    pub const SIGHT: f32 = 170.0;
    pub const REPAIR_DROP: u32 = 0;
}

/// The ASHFRAME PATRIARCH.
pub mod boss {
    pub const NAME: &str = "ASHFRAME PATRIARCH";
    pub const HEALTH: f32 = 4200.0;
    pub const STABILITY: f32 = 260.0;
    pub const HEIGHT: f32 = 11.5;
    pub const RADIUS: f32 = 3.4;
    pub const SPEED: f32 = 13.0;
    pub const ACCEL: f32 = 26.0;
    /// Health fractions at which it changes behaviour.
    pub const PHASE_2_AT: f32 = 0.6;
    pub const PHASE_3_AT: f32 = 0.28;
    pub const BARRAGE_COUNT: u32 = 10;
    pub const BARRAGE_INTERVAL: f32 = 0.11;
    pub const BARRAGE_COOLDOWN: f32 = 5.4;
    pub const BARRAGE_DAMAGE: f32 = 26.0;
    pub const BARRAGE_SPEED: f32 = 120.0;
    pub const BARRAGE_SPREAD: f32 = 0.06;
    pub const DASH_SPEED: f32 = 44.0;
    pub const DASH_DURATION: f32 = 0.65;
    pub const DASH_COOLDOWN: f32 = 6.0;
    pub const SLAM_RANGE: f32 = 16.0;
    pub const SLAM_WINDUP: f32 = 0.75;
    pub const SLAM_DAMAGE: f32 = 160.0;
    pub const SLAM_RADIUS: f32 = 20.0;
    pub const SLAM_COOLDOWN: f32 = 5.0;
    pub const BLADE_DAMAGE: f32 = 120.0;
    pub const BLADE_RANGE: f32 = 22.0;
    pub const BLADE_ARC: f32 = 1.0;
    pub const SIGHT: f32 = 200.0;
}

/// Target lock.
pub mod targeting {
    pub const MAX_RANGE: f32 = 190.0;
    /// Angular tolerance from the reticle for a candidate, in radians.
    pub const CONE: f32 = 0.52;
    /// Lock is dropped beyond this angle — wider than the acquisition cone, so
    /// a lock is not lost the instant the target drifts to the edge of it.
    pub const BREAK_CONE: f32 = 0.95;
    /// Seconds of occlusion or angle violation tolerated before the lock breaks.
    pub const GRACE: f32 = 0.45;
    /// How fast the lock point tracks the target, in 1/seconds.
    pub const TRACK_RATE: f32 = 7.5;
    /// Camera authority while locked. Lower keeps free aim responsive.
    pub const ASSIST_YAW: f32 = 2.1;
    pub const ASSIST_PITCH: f32 = 1.6;
}

/// Mission pacing.
pub mod mission {
    /// Time between a wave clearing and the next deployment.
    pub const WAVE_GAP: f32 = 2.5;
    pub const INTRO_TIME: f32 = 3.2;

    /// Points for a kill, by archetype.
    ///
    /// Matched by archetype rather than looked up by name, which is what the
    /// original did: a string table would score an archetype that was renamed
    /// as zero, and a silent zero on the results screen reads as a bug rather
    /// than as a missing entry.
    pub fn score_for(kind: crate::types::EnemyKind) -> u32 {
        use crate::types::EnemyKind;
        match kind {
            EnemyKind::Skirmisher => 100,
            EnemyKind::Artillery => 175,
            EnemyKind::Boss => 2500,
        }
    }
}

/// Presentation limits.
pub mod fx {
    pub const MAX_PARTICLES: usize = 900;
    pub const MAX_DEBRIS: usize = 90;
    pub const MAX_DECALS: usize = 48;
    /// Hard cap on simultaneously scheduled audio voices. Sounds past it are
    /// dropped rather than mixed, because distorting is worse than quiet.
    pub const MAX_VOICES: usize = 28;
    pub const SHAKE_FIRING: f32 = 0.05;
    pub const SHAKE_IMPACT: f32 = 0.35;
    pub const SHAKE_LANDING: f32 = 0.4;
    pub const SHAKE_EXPLOSION: f32 = 0.7;
}

/// Score awarded per kill.
pub fn score_for(enemy: crate::types::EnemyKind) -> u32 {
    use crate::types::EnemyKind;
    match enemy {
        EnemyKind::Skirmisher => 100,
        EnemyKind::Artillery => 175,
        EnemyKind::Boss => 2500,
    }
}

/// One of the three selectable frames.
///
/// The original stored these in an array and indexed it. This is an enum, so a
/// frame that does not exist cannot be selected — and the parameters live
/// beside the name rather than in a parallel table that could drift from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MechPresetId {
    Light,
    /// The frame a project starts on, and the one an unfamiliar stored choice
    /// falls back to.
    #[default]
    Balanced,
    Heavy,
}

/// The frame a player chose, and what it actually changes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MechPreset {
    pub id: MechPresetId,
    /// Shown on the briefing screen.
    pub label: &'static str,
    pub blurb: &'static str,
    pub health: f32,
    pub glide_speed: f32,
    pub quick_boost_speed: f32,
    pub energy_max: f32,
    pub energy_regen_ground: f32,
    /// Armour tint, as `0xRRGGBB`, for the renderer.
    pub armor_tint: u32,
    /// Emissive accent, as `0xRRGGBB`, for the renderer.
    pub accent: u32,
    /// Scales acceleration and knockback.
    pub mass: f32,
}

/// The three frames, in the order they are offered.
///
/// The README's table: *"They change real parameters, not labels."* Every field
/// below is read by the simulation; none is decoration.
pub const MECH_PRESETS: [MechPreset; 3] = [
    MechPreset {
        id: MechPresetId::Light,
        label: "VYPER",
        blurb: "Fast, fragile, high energy reserve.",
        health: 760.0,
        glide_speed: 25.5,
        quick_boost_speed: 54.0,
        energy_max: 130.0,
        energy_regen_ground: 30.0,
        armor_tint: 0x00ad_bccb,
        accent: 0x00ff_6a2a,
        mass: 0.82,
    },
    MechPreset {
        id: MechPresetId::Balanced,
        label: "RANGER",
        blurb: "Standard all-round frame.",
        health: 1000.0,
        glide_speed: 22.0,
        quick_boost_speed: 48.0,
        energy_max: 100.0,
        energy_regen_ground: 25.0,
        armor_tint: 0x00c9_d0d6,
        accent: 0x0039_d7ff,
        mass: 1.0,
    },
    MechPreset {
        id: MechPresetId::Heavy,
        label: "BASTION",
        blurb: "Armoured, slower, hits harder on the blade.",
        health: 1380.0,
        glide_speed: 18.5,
        quick_boost_speed: 43.0,
        energy_max: 86.0,
        energy_regen_ground: 21.0,
        armor_tint: 0x00a8_a396,
        accent: 0x00ff_c44d,
        mass: 1.22,
    },
];

impl MechPresetId {
    pub fn preset(self) -> &'static MechPreset {
        MECH_PRESETS
            .iter()
            .find(|preset| preset.id == self)
            .expect("every id has a preset")
    }

    pub fn label(self) -> &'static str {
        self.preset().label
    }

    /// The id as the briefing screen and `localStorage` spell it.
    pub fn key(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Balanced => "balanced",
            Self::Heavy => "heavy",
        }
    }

    /// Parse a stored choice, falling back to the balanced frame.
    ///
    /// Falling back rather than failing: this comes from `localStorage`, which
    /// a person can edit and a previous version may have written differently.
    /// A game that refuses to start because a preference string is unfamiliar
    /// is worse than one that starts on the default.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "light" | "vyper" => Self::Light,
            "heavy" | "bastion" => Self::Heavy,
            _ => Self::Balanced,
        }
    }
}

#[cfg(test)]
mod tests {
    // This file *is* constants, and asserting on them is the whole purpose:
    // these tests are a regression guard on numbers a person might change
    // while tuning. A test the compiler can fold to `true` today is precisely
    // what is wanted, because the day it stops folding is the day somebody
    // changed a value the game depends on.
    #![allow(clippy::assertions_on_constants)]

    use super::*;
    use crate::types::EnemyKind;

    #[test]
    fn the_simulation_rate_and_step_agree() {
        // A step that does not match the rate makes every timer in the game
        // slightly wrong, in a way that only shows up over minutes of play.
        assert_eq!(sim::HZ, 120, "the README documents 120 Hz");
        assert!((sim::DT - 1.0 / sim::HZ as f32).abs() < 1e-9);
    }

    #[test]
    fn the_catch_up_budget_is_sane() {
        // A quarter second is 30 steps at 120 Hz, comfortably more than the
        // 24-step cap -- so the cap is what actually binds, and a long frame
        // cannot buy unlimited catch-up.
        assert!(sim::MAX_CATCH_UP * sim::HZ as f32 > sim::MAX_STEPS as f32);
        assert!(sim::MAX_CATCH_UP < 1.0, "catch-up must not exceed a second");
    }

    #[test]
    fn the_arena_is_the_size_the_readme_claims() {
        // "Playable area is roughly 300 x 300."
        assert_eq!(world::HALF * 2.0, 300.0);
    }

    #[test]
    fn the_kill_plane_is_below_the_floor() {
        assert!(world::KILL_PLANE < world::FLOOR_Y);
    }

    #[test]
    fn gravity_pulls_downwards() {
        assert!(world::GRAVITY < 0.0, "gravity must be negative");
    }

    #[test]
    fn every_frame_preset_matches_the_documented_table() {
        // The README prints these numbers. A preset that drifts from its
        // documentation is a preset players cannot reason about.
        let light = MechPresetId::Light.preset();
        assert_eq!(light.label, "VYPER");
        assert_eq!(light.health, 760.0);
        assert_eq!(light.glide_speed, 25.5);
        assert_eq!(light.quick_boost_speed, 54.0);
        assert_eq!(light.energy_max, 130.0);

        let balanced = MechPresetId::Balanced.preset();
        assert_eq!(balanced.label, "RANGER");
        assert_eq!(balanced.health, 1000.0);
        assert_eq!(balanced.glide_speed, 22.0);
        assert_eq!(balanced.quick_boost_speed, 48.0);
        assert_eq!(balanced.energy_max, 100.0);

        let heavy = MechPresetId::Heavy.preset();
        assert_eq!(heavy.label, "BASTION");
        assert_eq!(heavy.health, 1380.0);
        assert_eq!(heavy.glide_speed, 18.5);
        assert_eq!(heavy.quick_boost_speed, 43.0);
        assert_eq!(heavy.energy_max, 86.0);
    }

    #[test]
    fn the_presets_are_ordered_light_to_heavy() {
        // The trade the player is choosing between has to be a real one: a
        // light frame that is not faster, or a heavy one that is not tougher,
        // is a false choice.
        let [light, balanced, heavy] = MECH_PRESETS;
        assert!(light.health < balanced.health);
        assert!(balanced.health < heavy.health);
        assert!(light.glide_speed > balanced.glide_speed);
        assert!(balanced.glide_speed > heavy.glide_speed);
        assert!(light.energy_max > balanced.energy_max);
        assert!(balanced.energy_max > heavy.energy_max);
        assert!(light.mass < balanced.mass);
        assert!(balanced.mass < heavy.mass);
    }

    #[test]
    fn every_preset_has_a_label_and_a_blurb() {
        for preset in MECH_PRESETS {
            assert!(!preset.label.is_empty());
            assert!(!preset.blurb.is_empty());
            assert!(preset.health > 0.0);
            assert!(preset.mass > 0.0);
        }
    }

    #[test]
    fn preset_ids_round_trip_through_their_keys() {
        for preset in MECH_PRESETS {
            assert_eq!(MechPresetId::parse(preset.id.key()), preset.id);
            assert_eq!(MechPresetId::parse(preset.label), preset.id);
        }
    }

    #[test]
    fn an_unfamiliar_stored_choice_falls_back_rather_than_failing() {
        // This comes from localStorage, which a person can edit and an older
        // version may have written differently. Refusing to start over a
        // preference string would be a poor trade.
        assert_eq!(MechPresetId::parse("nonsense"), MechPresetId::Balanced);
        assert_eq!(MechPresetId::parse(""), MechPresetId::Balanced);
        assert_eq!(MechPresetId::default(), MechPresetId::Balanced);
    }

    #[test]
    fn the_score_table_covers_every_enemy() {
        // A missing archetype would score nothing, and nobody would notice
        // until a mission ended with the wrong total.
        assert_eq!(score_for(EnemyKind::Skirmisher), 100);
        assert_eq!(score_for(EnemyKind::Artillery), 175);
        assert_eq!(score_for(EnemyKind::Boss), 2500);
    }

    #[test]
    fn a_boss_is_worth_more_than_a_whole_wave_of_skirmishers() {
        assert!(score_for(EnemyKind::Boss) > score_for(EnemyKind::Artillery) * 10);
    }

    #[test]
    fn the_weapons_are_ordered_by_range_the_way_the_game_plays() {
        // The blade closes, the rifle reaches, the missiles reach further.
        assert!(blade::RANGE < rifle::RANGE);
        assert!(rifle::SPEED > missiles::SPEED, "the rifle is the fast one");
    }

    #[test]
    fn a_rifle_round_cannot_tunnel_a_thin_wall() {
        // The reason projectiles are swept rather than stepped: at 300 m/s and
        // a 120 Hz step, a round moves 2.5 metres between steps -- further than
        // many surfaces are thick. This asserts the number that makes sweeping
        // mandatory, so nobody later "optimises" it into a point test.
        let per_step = rifle::SPEED * sim::DT;
        assert!(
            per_step > 1.0,
            "a round advances {per_step} m per step, so collisions must be swept"
        );
    }

    #[test]
    fn the_missile_lock_is_required_as_documented() {
        assert!(missiles::LOCK_REQUIRED);
    }

    #[test]
    fn enemy_tiers_are_ordered_by_toughness() {
        assert!(skirmisher::HEALTH < artillery::HEALTH);
        assert!(artillery::HEALTH < boss::HEALTH);
        assert!(skirmisher::STABILITY < artillery::STABILITY);
        assert!(artillery::STABILITY < boss::STABILITY);
    }

    #[test]
    fn the_boss_phases_are_ordered_and_inside_the_bar() {
        assert!(boss::PHASE_3_AT < boss::PHASE_2_AT);
        assert!(boss::PHASE_2_AT < 1.0);
        assert!(boss::PHASE_3_AT > 0.0);
    }

    #[test]
    fn the_lock_break_cone_is_wider_than_the_acquisition_cone() {
        // Otherwise a lock would be lost the instant a target drifted to the
        // edge of the cone that acquired it, which reads as the game dropping
        // locks at random.
        assert!(targeting::BREAK_CONE > targeting::CONE);
    }

    #[test]
    fn the_camera_pitch_limits_are_ordered() {
        assert!(camera::MIN_PITCH < camera::MAX_PITCH);
    }

    #[test]
    fn the_camera_can_pull_in_further_than_its_minimum() {
        assert!(camera::MIN_DISTANCE > camera::PROBE_RADIUS);
        assert!(camera::DISTANCE > camera::MIN_DISTANCE);
    }

    #[test]
    fn presenting_more_than_the_limits_is_not_possible() {
        // The caps exist so a long fight cannot grow unbounded; they must be
        // small enough to mean something.
        assert!(fx::MAX_PARTICLES > 0 && fx::MAX_PARTICLES <= 4096);
        assert!(fx::MAX_VOICES > 0 && fx::MAX_VOICES <= 128);
    }

    #[test]
    fn energy_economy_is_coherent() {
        // A boost must not be free, and a full bar must afford one.
        assert!(player::ENERGY_QUICK_BOOST > 0.0);
        assert!(player::ENERGY_MAX > player::ENERGY_QUICK_BOOST);
        assert!(
            player::ENERGY_MAX > player::ENERGY_MIN_TO_BOOST,
            "a full bar must be able to start a boost"
        );
        assert!(player::ENERGY_REGEN_GROUND > player::ENERGY_REGEN_AIR);
    }

    #[test]
    fn flight_caps_descent_but_still_permits_it() {
        // Thrusting holds the fall to a controlled rate rather than stopping
        // it: a cap at or above zero would let the mech hover indefinitely.
        assert!(player::FLIGHT_FALL_CAP < 0.0, "the cap must be a descent");
        assert!(
            player::FLIGHT_FALL_CAP > -player::FALL_MAX,
            "thrusting must fall more slowly than falling"
        );
    }

    #[test]
    fn a_stagger_makes_a_target_more_vulnerable_not_less() {
        assert!(player::STAGGER_DAMAGE_SCALE > 1.0);
        assert!(player::STAGGER_DURATION > 0.0);
    }

    #[test]
    fn a_repair_does_not_heal_more_than_a_frame_has() {
        assert!(player::REPAIR_AMOUNT < player::HEALTH_MAX);
        assert!(player::REPAIRS > 0);
    }

    #[test]
    fn the_mission_gap_is_a_pause_and_not_a_wait() {
        assert!(mission::WAVE_GAP > 0.0 && mission::WAVE_GAP < 10.0);
        assert!(mission::INTRO_TIME > 0.0 && mission::INTRO_TIME < 10.0);
    }
}
