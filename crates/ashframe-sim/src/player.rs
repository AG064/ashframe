//! The player mech.
//!
//! Ported from the original's `game/player.ts`: *"Movement is the core of the
//! game, so this module owns the whole control loop: camera-relative ground
//! skating, directional quick boosts, jump + sustained aerial thrust, forward
//! assault boosting, a shared energy pool, weapons and damage."*
//!
//! The design rule it stated is kept: *"the body may lean, lag and settle, but
//! the **control** response is immediate. No movement input is ever delayed by
//! animation state."* That is why [`Player::lean_x`] and the visual body yaw
//! are computed from what already happened and never feed back into a decision.
//!
//! ## The one deliberate behavioural change
//!
//! The original scattered autocannon fire with `Math.random()`. Everything else
//! about the simulation is deterministic — fixed 120 Hz step, a seeded
//! generator for enemy scatter, no wall-clock anywhere — and that one call made
//! the whole thing unreproducible, because a replay could not follow the same
//! shots.
//!
//! This uses the seeded generator instead. The pattern of a burst is therefore
//! identical run to run, which is what a replay needs, and there is no
//! gameplay difference: spread is still uniform across the same cone.

use crate::collision::CollisionWorld;
use crate::config::{
    blade as blade_cfg, missiles as missile_cfg, player as cfg, rifle, MechPreset, MechPresetId,
    MECH_PRESETS,
};
use crate::math::{clamp, damp, damp_angle, Rng};
use crate::types::{BladePhase, Damageable, Faction, Hooks, SimEvent, Vec3, WeaponId};

/// What the controls are asking for this step.
///
/// Already resolved and camera-relative: this is what the input layer decided,
/// not the raw keys. Keeping that split is what lets a replay feed the
/// simulation the same intents a person produced.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MovementIntent {
    /// Desired move direction in world space, in XZ.
    pub move_x: f32,
    pub move_z: f32,
    /// Magnitude of the move input, 0 to 1.
    pub move_mag: f32,
    pub jump_held: bool,
    pub jump_pressed: bool,
    pub quick_boost_pressed: bool,
    pub assault_held: bool,
    /// Where the mech is aiming, used by the assault boost.
    pub aim_x: f32,
    pub aim_z: f32,
}

/// The triggers, pulled out of [`MovementIntent`] because they are about
/// weapons rather than about moving.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Triggers {
    pub fire_primary: bool,
    pub missile_pressed: bool,
    pub blade_pressed: bool,
    pub reload_pressed: bool,
    pub repair_pressed: bool,
}

/// What the HUD and the renderer read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerSnapshot {
    pub pos: Vec3,
    pub vel: Vec3,
    pub yaw: f32,
    pub body_yaw: f32,
    pub lean_x: f32,
    pub lean_z: f32,
    pub grounded: bool,
    pub assaulting: bool,
    pub flying: bool,
    pub boosting: bool,
    pub energy: f32,
    pub health: f32,
    pub stability: f32,
    pub staggered: bool,
    pub speed: f32,
    pub repairing: bool,
    pub blade_phase: BladePhase,
    pub reloading: bool,
}

/// The mech.
#[derive(Debug, Clone)]
pub struct Player {
    pub id: u32,
    pub pos: Vec3,
    pub vel: Vec3,
    pub radius: f32,
    pub height: f32,

    pub health: f32,
    pub max_health: f32,
    pub stability: f32,
    pub max_stability: f32,
    pub alive: bool,
    pub stagger_timer: f32,
    pub invuln_timer: f32,
    pub stagger_armed: bool,

    /// Facing used for aiming. Follows the camera directly.
    pub yaw: f32,
    /// Visual body yaw, lagging behind [`Player::yaw`].
    pub body_yaw: f32,
    pub lean_x: f32,
    pub lean_z: f32,
    /// Pitch of the torso and weapons, driven by the camera.
    pub pitch: f32,

    pub grounded: bool,
    coyote: f32,
    jump_buffer: f32,

    pub energy: f32,
    energy_hold: f32,
    was_empty: bool,

    boost_timer: f32,
    boost_cooldown: f32,
    boost_dir: Vec3,
    pub boosting: bool,
    pub assaulting: bool,
    pub flying: bool,

    // Weapons.
    pub ammo: u32,
    fire_cooldown: f32,
    pub reload_timer: f32,
    pub missile_cooldown: f32,
    volley_left: u32,
    volley_timer: f32,
    volley_target_id: Option<u32>,
    pub blade_phase: BladePhase,
    blade_timer: f32,
    pub blade_cooldown: f32,
    blade_hit_ids: Vec<u32>,
    blade_lunge_applied: bool,

    pub repairs: u32,
    pub repair_timer: f32,

    pub preset: MechPresetId,
    pub glide_speed: f32,
    pub quick_boost_speed: f32,
    pub energy_max: f32,
    pub energy_regen_ground: f32,

    /// Landing impulse from the last landing, for effects.
    pub last_land_speed: f32,
    /// Where the weapons are pointed, set by the world each step.
    pub aim_point: Vec3,

    pub recoil: f32,
    stability_decay: f32,
    assault_held_time: f32,

    /// For weapon spread. Seeded, so a burst scatters identically every run.
    rng: Rng,
}

impl Player {
    /// Build a mech on a frame, at a spawn point.
    ///
    /// The initial invulnerability window is the configured respawn grace, so a
    /// mission that starts with enemies already firing does not begin with the
    /// player losing health before they can move.
    pub fn new(preset: MechPresetId, spawn: Vec3) -> Self {
        let frame = *preset.preset();
        Self {
            id: 1,
            pos: spawn,
            vel: Vec3::ZERO,
            radius: cfg::RADIUS,
            height: cfg::HEIGHT,

            health: frame.health,
            max_health: frame.health,
            stability: 0.0,
            max_stability: cfg::STABILITY_MAX,
            alive: true,
            stagger_timer: 0.0,
            invuln_timer: cfg::RESPAWN_INVULN,
            stagger_armed: false,

            yaw: 0.0,
            body_yaw: 0.0,
            lean_x: 0.0,
            lean_z: 0.0,
            pitch: 0.0,

            grounded: true,
            coyote: 0.0,
            jump_buffer: 0.0,

            energy: frame.energy_max,
            energy_hold: 0.0,
            was_empty: false,

            boost_timer: 0.0,
            boost_cooldown: 0.0,
            boost_dir: Vec3::ZERO,
            boosting: false,
            assaulting: false,
            flying: false,

            ammo: rifle::MAGAZINE,
            fire_cooldown: 0.0,
            reload_timer: 0.0,
            missile_cooldown: 0.0,
            volley_left: 0,
            volley_timer: 0.0,
            volley_target_id: None,
            blade_phase: BladePhase::Idle,
            blade_timer: 0.0,
            blade_cooldown: 0.0,
            blade_hit_ids: Vec::new(),
            blade_lunge_applied: false,

            repairs: cfg::REPAIRS,
            repair_timer: 0.0,

            preset,
            glide_speed: frame.glide_speed,
            quick_boost_speed: frame.quick_boost_speed,
            energy_max: frame.energy_max,
            energy_regen_ground: frame.energy_regen_ground,

            last_land_speed: 0.0,
            aim_point: Vec3::ZERO,

            recoil: 0.0,
            stability_decay: 0.0,
            assault_held_time: 0.0,

            rng: Rng::default(),
        }
    }

    /// Full reset for a mission restart.
    ///
    /// Written out field by field rather than reconstructed, so a field added
    /// later cannot be forgotten: the compiler will refuse to build this until
    /// it is mentioned, which is the whole reason the original did it this way
    /// too.
    pub fn reset(&mut self, spawn: Vec3, preset: MechPresetId) {
        let frame = *preset.preset();
        self.preset = preset;
        self.max_health = frame.health;
        self.health = frame.health;
        self.glide_speed = frame.glide_speed;
        self.quick_boost_speed = frame.quick_boost_speed;
        self.energy_max = frame.energy_max;
        self.energy_regen_ground = frame.energy_regen_ground;

        self.pos = spawn;
        self.vel = Vec3::ZERO;
        self.yaw = 0.0;
        self.body_yaw = 0.0;
        self.pitch = 0.0;
        self.lean_x = 0.0;
        self.lean_z = 0.0;
        self.stability = 0.0;
        self.alive = true;
        self.stagger_timer = 0.0;
        self.stagger_armed = false;
        self.invuln_timer = cfg::RESPAWN_INVULN;

        self.grounded = true;
        self.coyote = 0.0;
        self.jump_buffer = 0.0;
        self.energy = self.energy_max;
        self.energy_hold = 0.0;
        self.was_empty = false;
        self.boost_timer = 0.0;
        self.boost_cooldown = 0.0;
        self.boost_dir = Vec3::ZERO;
        self.boosting = false;
        self.assaulting = false;
        self.flying = false;
        self.assault_held_time = 0.0;

        self.ammo = rifle::MAGAZINE;
        self.fire_cooldown = 0.0;
        self.reload_timer = 0.0;
        self.missile_cooldown = 0.0;
        self.volley_left = 0;
        self.volley_timer = 0.0;
        self.volley_target_id = None;
        self.blade_phase = BladePhase::Idle;
        self.blade_timer = 0.0;
        self.blade_cooldown = 0.0;
        self.blade_hit_ids.clear();
        self.blade_lunge_applied = false;

        self.repairs = cfg::REPAIRS;
        self.repair_timer = 0.0;
        self.last_land_speed = 0.0;
        self.recoil = 0.0;
        self.stability_decay = 0.0;
    }

    /// Ground speed, ignoring vertical motion.
    pub fn speed(&self) -> f32 {
        self.vel.horizontal_length()
    }

    pub fn staggered(&self) -> bool {
        self.stagger_timer > 0.0
    }

    /// Remaining time in the current blade phase.
    pub fn blade_timer_left(&self) -> f32 {
        self.blade_timer
    }

    /// The centre of the mech, which is what most effects are placed from.
    pub fn centre_y(&self) -> f32 {
        self.pos.y + self.height * 0.5
    }

    /// The autocannon's muzzle, on the right shoulder mount.
    pub fn rifle_muzzle(&self) -> Vec3 {
        let (c, s) = (self.body_yaw.cos(), self.body_yaw.sin());
        let (rx, rz) = (1.9, 1.5);
        Vec3::new(
            self.pos.x + c * rz - s * rx,
            self.pos.y + self.height * 0.74,
            self.pos.z - s * rz - c * rx,
        )
    }

    /// The missile pod's muzzle, on the left shoulder mount.
    pub fn missile_muzzle(&self) -> Vec3 {
        let (c, s) = (self.body_yaw.cos(), self.body_yaw.sin());
        let (rx, rz) = (-1.9, 0.4);
        Vec3::new(
            self.pos.x + c * rz - s * rx,
            self.pos.y + self.height * 0.92,
            self.pos.z - s * rz - c * rx,
        )
    }

    /// The frame this mech is built on.
    pub fn frame(&self) -> &'static MechPreset {
        self.preset.preset()
    }

    /// Advance one fixed step.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        dt: f32,
        intent: &MovementIntent,
        triggers: &Triggers,
        lock_id: Option<u32>,
        world: &CollisionWorld,
        hooks: &mut dyn Hooks,
    ) {
        if !self.alive {
            return;
        }
        if self.invuln_timer > 0.0 {
            self.invuln_timer -= dt;
        }
        self.tick_stagger(dt, hooks);
        self.tick_timers(dt, hooks);
        self.apply_movement(dt, intent, world, hooks);
        self.tick_repair(triggers.repair_pressed, hooks);
        self.tick_weapons(dt, triggers, lock_id, hooks);
        self.tick_recoil(dt);
    }

    fn tick_stagger(&mut self, dt: f32, hooks: &mut dyn Hooks) {
        if self.stagger_timer > 0.0 {
            self.stagger_timer -= dt;
            if self.stagger_timer <= 0.0 {
                self.stagger_timer = 0.0;
                self.stagger_armed = false;
                hooks.emit(&SimEvent::Unstagger { target: self.id });
            }
        } else if self.stability > 0.0 {
            // Stability only starts decaying after a delay, so a burst that
            // lands over a second still accumulates rather than each hit
            // recovering before the next arrives.
            self.stability_decay -= dt;
            if self.stability_decay <= 0.0 {
                self.stability = (self.stability - cfg::STABILITY_DECAY_RATE * dt).max(0.0);
            }
        }
    }

    fn tick_timers(&mut self, dt: f32, hooks: &mut dyn Hooks) {
        self.fire_cooldown = (self.fire_cooldown - dt).max(0.0);
        self.boost_cooldown = (self.boost_cooldown - dt).max(0.0);
        self.blade_cooldown = (self.blade_cooldown - dt).max(0.0);
        self.missile_cooldown = (self.missile_cooldown - dt).max(0.0);

        if self.repair_timer > 0.0 {
            self.repair_timer -= dt;
            // Heals smoothly across the repair window rather than in one lump
            // at the end, so being interrupted costs the part that did not
            // happen.
            self.health =
                (self.health + (cfg::REPAIR_AMOUNT / cfg::REPAIR_TIME) * dt).min(self.max_health);
            if self.repair_timer <= 0.0 {
                self.repair_timer = 0.0;
                hooks.emit(&SimEvent::RepairEnd);
            }
        }

        // Energy regeneration is delayed, faster on the ground, and never while
        // something is being spent on. The delay is what stops a player from
        // tapping a boost and regenerating through it.
        let spending = self.boosting || self.flying || self.assaulting;
        if self.energy_hold > 0.0 {
            self.energy_hold -= dt;
        }
        if !spending && self.energy_hold <= 0.0 && self.energy < self.energy_max {
            let rate = if self.grounded {
                self.energy_regen_ground
            } else {
                cfg::ENERGY_REGEN_AIR
            };
            self.energy = (self.energy + rate * dt).min(self.energy_max);
        }

        // Edge triggered, so the HUD announces an empty tank once rather than
        // every step it stays empty.
        let empty = self.energy <= 0.5;
        if empty && !self.was_empty {
            self.was_empty = true;
            hooks.emit(&SimEvent::EnergyEmpty);
        } else if !empty && self.was_empty && self.energy > self.energy_max * 0.25 {
            // Not the instant a drop of energy returns: the tank has to reach a
            // quarter, or the announcement would fire on the first frame of
            // regeneration and mean nothing.
            self.was_empty = false;
            hooks.emit(&SimEvent::EnergyRestored);
        }
    }

    fn spend_energy(&mut self, amount: f32) {
        self.energy = (self.energy - amount).max(0.0);
        self.energy_hold = cfg::ENERGY_REGEN_DELAY;
    }

    fn apply_movement(
        &mut self,
        dt: f32,
        intent: &MovementIntent,
        world: &CollisionWorld,
        hooks: &mut dyn Hooks,
    ) {
        let turn_scale = if self.assaulting {
            cfg::ASSAULT_TURN_SCALE
        } else {
            1.0
        };
        // Facing follows aim exactly. Only the visual body lags, which is the
        // design rule: no input is ever delayed by how the mech looks.
        self.yaw = intent.aim_x.atan2(intent.aim_z);
        self.body_yaw = damp_angle(self.body_yaw, self.yaw, cfg::TORSO_LAG, dt);

        let input_mag = clamp(intent.move_mag, 0.0, 1.0);
        let (dir_x, dir_z) = if input_mag > 0.001 {
            (intent.move_x / input_mag, intent.move_z / input_mag)
        } else {
            (0.0, 0.0)
        };

        // Jump buffering and coyote time, together: a jump pressed just before
        // landing still fires, and a jump pressed just after walking off an
        // edge still fires.
        if intent.jump_pressed {
            self.jump_buffer = cfg::INPUT_BUFFER;
        }
        if self.jump_buffer > 0.0 {
            self.jump_buffer -= dt;
        }
        if self.grounded {
            self.coyote = cfg::COYOTE_TIME;
        } else if self.coyote > 0.0 {
            self.coyote -= dt;
        }

        let can_boost = self.energy >= cfg::ENERGY_MIN_TO_BOOST
            && self.boost_cooldown <= 0.0
            && self.stagger_timer <= 0.0
            && self.repair_timer <= 0.0;

        // -- quick boost --------------------------------------------------
        if intent.quick_boost_pressed && can_boost {
            let (bx, bz) = if input_mag < 0.2 {
                // With no directional input, dodge backwards relative to
                // facing: a dodge that went nowhere would be useless.
                (-self.body_yaw.sin(), -self.body_yaw.cos())
            } else {
                (dir_x, dir_z)
            };
            self.boost_dir = Vec3::new(bx, 0.0, bz).normalized();
            self.boost_timer = cfg::QUICK_BOOST_DURATION;
            self.boost_cooldown = cfg::QUICK_BOOST_COOLDOWN;
            self.boosting = true;
            self.spend_energy(cfg::ENERGY_QUICK_BOOST);
            hooks.emit(&SimEvent::QuickBoost {
                at: self.pos,
                direction: self.boost_dir,
            });
        }
        if self.boost_timer > 0.0 {
            self.boost_timer -= dt;
            if self.boost_timer <= 0.0 {
                self.boosting = false;
            }
        }

        // -- assault boost ------------------------------------------------
        let want_assault = intent.assault_held
            && self.energy > 0.0
            && self.stagger_timer <= 0.0
            && self.repair_timer <= 0.0
            && self.boost_timer <= 0.0;
        if want_assault && !self.assaulting {
            self.assaulting = true;
            hooks.emit(&SimEvent::AssaultStart);
        } else if !want_assault && self.assaulting {
            self.assaulting = false;
            hooks.emit(&SimEvent::AssaultEnd);
        }
        if self.assaulting {
            self.assault_held_time += dt;
            self.spend_energy(cfg::ENERGY_ASSAULT_DRAIN * dt);
        } else {
            self.assault_held_time = 0.0;
        }

        // -- vertical -----------------------------------------------------
        if self.jump_buffer > 0.0 && (self.grounded || self.coyote > 0.0) {
            self.jump_buffer = 0.0;
            self.coyote = 0.0;
            self.vel.y = cfg::JUMP_SPEED;
            self.grounded = false;
            hooks.emit(&SimEvent::Jump {
                at: self.pos + Vec3::new(0.0, self.height, 0.0),
            });
        }

        let wants_flight = intent.jump_held
            && !self.grounded
            && self.energy > 0.0
            && self.stagger_timer <= 0.0
            && !self.assaulting;
        self.flying = wants_flight;
        if wants_flight {
            self.vel.y += cfg::FLIGHT_THRUST * dt;
            // Capped both ways: an ascent cap so flight is not a rocket, and a
            // descent cap so it feels controlled rather than like falling with
            // a light on.
            self.vel.y = self.vel.y.clamp(cfg::FLIGHT_FALL_CAP, cfg::FLIGHT_MAX_UP);
            self.spend_energy(cfg::ENERGY_FLIGHT_DRAIN * dt);
        } else if !self.grounded {
            self.vel.y += crate::config::world::GRAVITY * dt;
            if self.vel.y < -cfg::FALL_MAX {
                self.vel.y = -cfg::FALL_MAX;
            }
        }

        // -- horizontal ---------------------------------------------------
        // Being staggered or repairing slows the mech rather than freezing it,
        // so there is always something the player can do.
        let speed_scale = if self.stagger_timer > 0.0 {
            0.35
        } else if self.repair_timer > 0.0 {
            0.5
        } else {
            1.0
        };

        let mut target_x = dir_x * self.glide_speed * input_mag * speed_scale * turn_scale;
        let mut target_z = dir_z * self.glide_speed * input_mag * speed_scale * turn_scale;
        let mut accel = if self.grounded {
            cfg::GLIDE_ACCEL
        } else {
            cfg::AIR_ACCEL
        };

        if self.assaulting {
            // Assault boost drives forward on the body's facing, not the
            // stick, so the reduced turn authority is what steers it.
            target_x = self.body_yaw.sin() * cfg::ASSAULT_SPEED;
            target_z = self.body_yaw.cos() * cfg::ASSAULT_SPEED;
            accel = cfg::ASSAULT_ACCEL;
        }

        if self.boost_timer > 0.0 {
            // The dodge overrides ordinary acceleration entirely for its short
            // window. Blending it would make a dodge feel mushy and, worse,
            // make its distance depend on what the player was already doing.
            self.vel.x = self.boost_dir.x * self.quick_boost_speed;
            self.vel.z = self.boost_dir.z * self.quick_boost_speed;
        } else {
            let current = self.speed();
            let mut rate = accel;
            if input_mag < 0.05 && !self.assaulting {
                rate = if self.grounded {
                    cfg::GLIDE_DECEL
                } else {
                    // Less braking in the air, so letting go mid-jump coasts.
                    cfg::GLIDE_DECEL * 0.35
                };
            } else if current > 0.5 {
                // Opposing input bites harder, which is what makes a direction
                // change feel immediate rather than like a wide turn.
                let dot = (self.vel.x * dir_x + self.vel.z * dir_z) / current.max(0.0001);
                if dot < -0.2 {
                    rate += cfg::TURN_BRAKE * -dot;
                }
            }

            let delta = Vec3::new(target_x - self.vel.x, 0.0, target_z - self.vel.z);
            let delta_len = delta.horizontal_length();
            let max_delta = rate * dt;
            if delta_len <= max_delta || delta_len < 1e-5 {
                // Snapped rather than approached, so a target speed is reached
                // exactly and a stopped mech is actually stopped.
                self.vel.x = target_x;
                self.vel.z = target_z;
            } else {
                self.vel.x += (delta.x / delta_len) * max_delta;
                self.vel.z += (delta.z / delta_len) * max_delta;
            }

            // Air drift is capped so air control stays a nudge rather than a
            // second way to skate.
            if !self.grounded && !self.assaulting {
                let cap = cfg::AIR_MAX_SPEED.max(self.speed());
                let sp = self.speed();
                if sp > cap {
                    self.vel.x *= cap / sp;
                    self.vel.z *= cap / sp;
                }
            }
        }

        self.integrate(dt, world, hooks);
        self.update_lean(dt, dir_x, dir_z);
    }

    /// Move the body and resolve it against the world.
    ///
    /// Horizontal motion is **substepped**: a 48 m/s dodge covers 0.4 metres in
    /// a step, and against thin cover a single positional resolve can push the
    /// body out of one box and straight into another. Splitting the move into
    /// steps of at most 0.7 metres and resolving each keeps the no-clipping
    /// guarantee a property of the resolver rather than a tuning accident.
    fn integrate(&mut self, dt: f32, world: &CollisionWorld, hooks: &mut dyn Hooks) {
        let delta = Vec3::new(self.vel.x * dt, 0.0, self.vel.z * dt);
        let distance = delta.horizontal_length();
        let max_step = 0.7;
        let steps = ((distance / max_step).ceil() as u32).max(1);
        let share = 1.0 / steps as f32;

        for _ in 0..steps {
            self.pos.x += delta.x * share;
            self.pos.z += delta.z * share;

            let resolved = world.resolve_cylinder(
                &mut self.pos,
                self.radius,
                self.height,
                // A generous step height, so kerbs and ramp steps are walked
                // over rather than caught on.
                1.25,
                self.vel.y,
            );

            if resolved.hit {
                // Remove only the part of the velocity pushing into the
                // surface, so sliding along a wall keeps its speed instead of
                // stopping dead.
                let normal = resolved.normal_xz.horizontal_normalized();
                let length = normal.horizontal_length();
                if length > 1e-5 {
                    let into = self.vel.x * normal.x + self.vel.z * normal.z;
                    if into < 0.0 {
                        self.vel.x -= into * normal.x;
                        self.vel.z -= into * normal.z;
                    }
                }
            }
        }

        // Vertical, resolved once: there is no equivalent tunnelling risk
        // straight up or down.
        self.pos.y += self.vel.y * dt;
        let resolved =
            world.resolve_cylinder(&mut self.pos, self.radius, self.height, 1.25, self.vel.y);
        if resolved.ceiling && self.vel.y > 0.0 {
            self.vel.y = 0.0;
        }

        let ground = world.ground_under(
            self.pos.x,
            self.pos.z,
            self.pos.y + self.height + 2.0,
            self.radius,
        );

        if self.pos.y <= ground + 0.02 {
            let impact = -self.vel.y;
            // Announced only for a real landing, so stepping off a kerb does
            // not shake the camera.
            if !self.grounded && impact > 3.0 {
                self.last_land_speed = impact;
                hooks.emit(&SimEvent::Land {
                    at: Vec3::new(self.pos.x, ground, self.pos.z),
                    speed: impact,
                });
            }
            self.pos.y = ground;
            if self.vel.y < 0.0 {
                self.vel.y = 0.0;
            }
            self.grounded = true;
            self.flying = false;
        } else if self.pos.y > ground + 0.06 {
            // The gap before leaving the ground matches the one for landing, so
            // a body resting on a surface does not flicker between the two.
            self.grounded = false;
        }

        // Hard arena bounds. The world recovers anything that still escapes,
        // but a clamp here means it never has to.
        let limit = crate::config::world::HALF - self.radius;
        self.pos.x = clamp(self.pos.x, -limit, limit);
        self.pos.z = clamp(self.pos.z, -limit, limit);
    }

    /// Body lean, derived from the movement that actually happened.
    ///
    /// Decorative in the strict sense: nothing reads these back, so they can
    /// never gate control.
    fn update_lean(&mut self, dt: f32, dir_x: f32, dir_z: f32) {
        let local = self.to_local(dir_x, dir_z);
        let target_lean_z = -local.x * cfg::LEAN_MAX * if self.grounded { 1.0 } else { 0.6 };
        let target_lean_x = local.z * cfg::LEAN_MAX * 0.6 + clamp(-self.vel.y * 0.004, -0.12, 0.12);
        self.lean_x = damp(self.lean_x, target_lean_x, cfg::LEAN_RATE, dt);
        self.lean_z = damp(self.lean_z, target_lean_z, cfg::LEAN_RATE, dt);
    }

    /// Project a world XZ vector into the mech's own frame.
    fn to_local(&self, x: f32, z: f32) -> Vec3 {
        let c = (-self.body_yaw).cos();
        let s = (-self.body_yaw).sin();
        Vec3::new(x * c - z * s, 0.0, x * s + z * c)
    }

    fn tick_repair(&mut self, pressed: bool, hooks: &mut dyn Hooks) {
        // Only on the ground and only when actually hurt, so a repair is a
        // decision rather than something to press on cooldown.
        if pressed
            && self.repairs > 0
            && self.repair_timer <= 0.0
            && self.grounded
            && self.health < self.max_health
        {
            self.repairs -= 1;
            self.repair_timer = cfg::REPAIR_TIME;
            hooks.emit(&SimEvent::RepairStart {
                remaining: self.repairs,
            });
        }
    }

    fn tick_weapons(
        &mut self,
        dt: f32,
        triggers: &Triggers,
        lock_id: Option<u32>,
        hooks: &mut dyn Hooks,
    ) {
        // -- reload -------------------------------------------------------
        if self.reload_timer > 0.0 {
            self.reload_timer -= dt;
            if self.reload_timer <= 0.0 {
                self.reload_timer = 0.0;
                self.ammo = rifle::MAGAZINE;
                hooks.emit(&SimEvent::ReloadEnd);
            }
        } else if triggers.reload_pressed && self.ammo < rifle::MAGAZINE {
            self.start_reload(hooks);
        }

        // -- autocannon ---------------------------------------------------
        if triggers.fire_primary
            && self.fire_cooldown <= 0.0
            && self.reload_timer <= 0.0
            && self.stagger_timer <= 0.0
        {
            if self.ammo > 0 {
                self.fire_rifle(hooks);
            } else {
                hooks.emit(&SimEvent::DryFire {
                    weapon: WeaponId::Rifle,
                });
                // An empty magazine reloads itself rather than making the
                // player notice and press a key.
                self.start_reload(hooks);
            }
        }

        // -- missile volley -----------------------------------------------
        if triggers.missile_pressed
            && self.missile_cooldown <= 0.0
            && self.volley_left == 0
            && self.stagger_timer <= 0.0
        {
            if missile_cfg::LOCK_REQUIRED && lock_id.is_none() {
                hooks.emit(&SimEvent::DryFire {
                    weapon: WeaponId::Missiles,
                });
            } else {
                self.volley_left = missile_cfg::COUNT;
                self.volley_timer = 0.0;
                self.volley_target_id = lock_id;
                self.missile_cooldown = missile_cfg::COOLDOWN;
            }
        }
        if self.volley_left > 0 {
            self.volley_timer -= dt;
            if self.volley_timer <= 0.0 {
                // The interval is reset from the constant rather than
                // decremented, so a long frame cannot make the volley stutter.
                self.volley_timer = missile_cfg::LAUNCH_INTERVAL;
                self.volley_left -= 1;
                self.launch_missile(hooks);
            }
        }

        // -- blade --------------------------------------------------------
        if triggers.blade_pressed
            && self.blade_phase == BladePhase::Idle
            && self.blade_cooldown <= 0.0
            && self.stagger_timer <= 0.0
        {
            self.blade_phase = BladePhase::Windup;
            self.blade_timer = blade_cfg::WINDUP;
            // Cleared per swing, which is what enforces one hit per target per
            // swing rather than one per frame.
            self.blade_hit_ids.clear();
            self.blade_lunge_applied = false;
            hooks.emit(&SimEvent::Blade {
                phase: BladePhase::Windup,
            });
        }

        if self.blade_phase != BladePhase::Idle {
            self.blade_timer -= dt;
            match self.blade_phase {
                BladePhase::Windup if self.blade_timer <= 0.0 => {
                    self.blade_phase = BladePhase::Active;
                    self.blade_timer = blade_cfg::ACTIVE;
                    hooks.emit(&SimEvent::Blade {
                        phase: BladePhase::Active,
                    });
                }
                BladePhase::Active => {
                    // The lunge is applied once, at the start of the damage
                    // window, so it reads as a step into the swing.
                    if !self.blade_lunge_applied {
                        self.blade_lunge_applied = true;
                        self.vel.x += self.body_yaw.sin() * blade_cfg::LUNGE;
                        self.vel.z += self.body_yaw.cos() * blade_cfg::LUNGE;
                    }
                    if self.blade_timer <= 0.0 {
                        self.blade_phase = BladePhase::Recovery;
                        self.blade_timer = blade_cfg::RECOVERY;
                        hooks.emit(&SimEvent::Blade {
                            phase: BladePhase::Recovery,
                        });
                    }
                }
                BladePhase::Recovery if self.blade_timer <= 0.0 => {
                    self.blade_phase = BladePhase::Idle;
                    self.blade_cooldown = blade_cfg::COOLDOWN;
                }
                _ => {}
            }
        }
    }

    fn start_reload(&mut self, hooks: &mut dyn Hooks) {
        self.reload_timer = rifle::RELOAD_TIME;
        hooks.emit(&SimEvent::ReloadStart);
    }

    fn fire_rifle(&mut self, hooks: &mut dyn Hooks) {
        self.fire_cooldown = 1.0 / rifle::RPM;
        self.ammo -= 1;

        let muzzle = self.rifle_muzzle();
        let aim = (self.aim_point - muzzle).normalized();
        // Firing mid-swing throws the shot wider, which is what discourages
        // holding both triggers at once.
        let jitter = if self.blade_phase == BladePhase::Idle {
            1.0
        } else {
            1.6
        };
        let spread = rifle::SPREAD * jitter;

        // From the seeded generator, not from the system clock. This is the one
        // place the original was not deterministic.
        let scatter = Vec3::new(
            (self.rng.next() - 0.5) * spread,
            (self.rng.next() - 0.5) * spread,
            (self.rng.next() - 0.5) * spread,
        );

        hooks.emit(&SimEvent::Fire {
            weapon: WeaponId::Rifle,
            origin: muzzle,
            direction: (aim + scatter).normalized(),
        });
    }

    fn launch_missile(&mut self, hooks: &mut dyn Hooks) {
        let muzzle = self.missile_muzzle();
        // Alternating sides, so a volley reads as a ripple rather than a single
        // stream from one shoulder.
        let side = if self.volley_left.is_multiple_of(2) {
            1.0
        } else {
            -1.0
        };
        let c = self.body_yaw.cos();
        let s = self.body_yaw.sin();
        let origin = Vec3::new(
            muzzle.x + c * 0.6 * side,
            muzzle.y,
            muzzle.z - s * 0.6 * side,
        );

        let aim = Vec3::new(
            self.aim_point.x - origin.x,
            // Aimed a little above the lock point, so the missile arcs in
            // rather than flying flat into whatever is in front of the target.
            self.aim_point.y + 3.5 - muzzle.y,
            self.aim_point.z - origin.z,
        );

        hooks.emit(&SimEvent::MissileLaunch {
            origin,
            direction: aim.normalized(),
        });
    }

    /// Which enemy the current volley was fired at, if any.
    pub fn volley_target(&self) -> Option<u32> {
        self.volley_target_id
    }

    /// Called by the world when a blade swing overlaps a target.
    ///
    /// Returns whether this swing had not already hit that target. The
    /// bookkeeping lives here so the swing cannot hit one enemy twice, which
    /// would make the blade's damage depend on frame timing.
    pub fn register_blade_hit(&mut self, id: u32) -> bool {
        if self.blade_hit_ids.contains(&id) {
            return false;
        }
        self.blade_hit_ids.push(id);
        true
    }

    /// Add recoil to the visual weapon group.
    pub fn recoil_impulse(&mut self, strength: f32) {
        self.recoil = (self.recoil + strength).min(0.55);
    }

    fn tick_recoil(&mut self, dt: f32) {
        self.recoil = (self.recoil - dt * 5.2).max(0.0);
    }

    /// Take a hit from an enemy.
    ///
    /// Separate from [`crate::combat::apply_damage`] because the player's
    /// damage differs in ways that matter: it emits player-specific events, and
    /// the stability decay timer is reset here rather than in the combat path.
    pub fn take_damage(&mut self, amount: f32, impact: f32, hooks: &mut dyn Hooks) -> (bool, bool) {
        if !self.alive || self.invuln_timer > 0.0 {
            return (false, false);
        }
        let scale = if self.staggered() {
            cfg::STAGGER_DAMAGE_SCALE
        } else {
            1.0
        };
        let dealt = amount * scale;
        self.health -= dealt;
        hooks.emit(&SimEvent::PlayerDamage {
            amount: dealt,
            at: Vec3::new(self.pos.x, self.centre_y(), self.pos.z),
        });

        let mut staggered = false;
        if !self.staggered() {
            self.stability += impact;
            self.stability_decay = cfg::STABILITY_DECAY_DELAY;
            if self.stability >= self.max_stability {
                self.stability = self.max_stability;
                self.stagger_timer = cfg::STAGGER_DURATION;
                staggered = true;
                hooks.emit(&SimEvent::PlayerStagger);
            }
        }

        if self.health <= 0.0 {
            self.health = 0.0;
            self.alive = false;
            return (true, staggered);
        }
        (false, staggered)
    }

    pub fn snapshot(&self) -> PlayerSnapshot {
        PlayerSnapshot {
            pos: self.pos,
            vel: self.vel,
            yaw: self.yaw,
            body_yaw: self.body_yaw,
            lean_x: self.lean_x,
            lean_z: self.lean_z,
            grounded: self.grounded,
            assaulting: self.assaulting,
            flying: self.flying,
            boosting: self.boosting,
            energy: self.energy,
            health: self.health,
            stability: self.stability,
            staggered: self.staggered(),
            speed: self.speed(),
            repairing: self.repair_timer > 0.0,
            blade_phase: self.blade_phase,
            reloading: self.reload_timer > 0.0,
        }
    }

    /// Present the mech as something the combat and projectile code can hit.
    ///
    /// Borrowed rather than copied, so a hit applies to the real mech and not
    /// to a stand-in that then has to be written back.
    pub fn as_damageable(&self) -> crate::types::Damageable {
        crate::types::Damageable {
            id: self.id,
            faction: Faction::Player,
            name: "ASHFRAME".to_string(),
            pos: self.pos,
            vel: self.vel,
            radius: self.radius,
            height: self.height,
            health: self.health,
            max_health: self.max_health,
            stability: self.stability,
            max_stability: self.max_stability,
            alive: self.alive,
            stagger_timer: self.stagger_timer,
            invuln_timer: self.invuln_timer,
            stagger_armed: self.stagger_armed,
        }
    }

    /// The frames a player can choose between.
    pub fn presets() -> &'static [MechPreset] {
        &MECH_PRESETS
    }
}

/// `Damageable` is what the world hands to the combat path; the player is one.
///
/// The conversion is by value, and a caller that damages the result must write
/// it back. The world uses [`Player::take_damage`] for the player instead,
/// because the player's damage has its own events.
impl From<&Player> for Damageable {
    fn from(player: &Player) -> Self {
        player.as_damageable()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::EventLog;

    fn spawn() -> Vec3 {
        Vec3::new(0.0, 0.0, 0.0)
    }

    fn player() -> Player {
        Player::new(MechPresetId::Balanced, spawn())
    }

    fn world() -> CollisionWorld {
        CollisionWorld::new(0.0)
    }

    /// An intent that is doing nothing.
    fn idle() -> MovementIntent {
        MovementIntent::default()
    }

    /// An intent moving forward at full stick.
    fn forward() -> MovementIntent {
        MovementIntent {
            move_x: 0.0,
            move_z: 1.0,
            move_mag: 1.0,
            ..MovementIntent::default()
        }
    }

    #[test]
    fn a_new_mech_starts_whole_and_armed() {
        let p = player();
        assert_eq!(p.health, 1000.0);
        assert_eq!(p.energy, 100.0);
        assert_eq!(p.ammo, rifle::MAGAZINE);
        assert_eq!(p.repairs, cfg::REPAIRS);
        assert!(p.alive);
    }

    #[test]
    fn a_preset_actually_changes_the_frame() {
        let light = Player::new(MechPresetId::Light, spawn());
        let heavy = Player::new(MechPresetId::Heavy, spawn());
        assert!(light.max_health < heavy.max_health);
        assert!(light.glide_speed > heavy.glide_speed);
        assert!(light.energy_max > heavy.energy_max);
    }

    #[test]
    fn holding_forward_moves_the_mech() {
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();
        for _ in 0..120 {
            p.step(
                crate::config::sim::DT,
                &forward(),
                &Triggers::default(),
                None,
                &world,
                &mut log,
            );
        }
        assert!(
            p.pos.z > 5.0,
            "the mech should have moved, ended at {}",
            p.pos.z
        );
        assert!(
            (p.speed() - p.glide_speed).abs() < 0.5,
            "it should have reached glide speed, got {}",
            p.speed()
        );
    }

    #[test]
    fn a_mech_at_rest_stays_at_rest() {
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();
        for _ in 0..120 {
            p.step(
                crate::config::sim::DT,
                &idle(),
                &Triggers::default(),
                None,
                &world,
                &mut log,
            );
        }
        assert!(
            p.speed() < 0.01,
            "nothing should be moving, got {}",
            p.speed()
        );
        assert!(p.grounded);
    }

    #[test]
    fn speed_is_reached_the_same_way_at_any_frame_rate() {
        // The reason the whole simulation runs at a fixed step.
        let world = world();
        let run = |dt: f32, steps: usize| {
            let mut p = player();
            let mut log = EventLog::new();
            for _ in 0..steps {
                p.step(dt, &forward(), &Triggers::default(), None, &world, &mut log);
            }
            p.pos.z
        };

        let coarse = run(1.0 / 30.0, 30);
        let fine = run(1.0 / 120.0, 120);
        assert!(
            (coarse - fine).abs() < 1.0,
            "one second of movement diverged: {coarse} vs {fine}"
        );
    }

    // -- jumping ----------------------------------------------------------

    #[test]
    fn a_jump_leaves_the_ground_and_lands_again() {
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();
        let dt = crate::config::sim::DT;

        let jump = MovementIntent {
            jump_pressed: true,
            ..idle()
        };
        p.step(dt, &jump, &Triggers::default(), None, &world, &mut log);
        assert!(log.any("jump"), "the jump should have been announced");
        assert!(p.vel.y > 0.0, "and imparted upward speed");

        // Fly the arc out and land.
        for _ in 0..400 {
            p.step(dt, &idle(), &Triggers::default(), None, &world, &mut log);
            if p.grounded {
                break;
            }
        }
        assert!(p.grounded, "it should have come back down");
        assert!(log.any("land"));
    }

    #[test]
    fn a_jump_pressed_just_before_landing_still_fires() {
        // The input buffer. Without it a jump pressed a frame early is simply
        // lost, which reads as the controls dropping inputs.
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();
        let dt = crate::config::sim::DT;

        // Get airborne.
        p.step(
            dt,
            &MovementIntent {
                jump_pressed: true,
                ..idle()
            },
            &Triggers::default(),
            None,
            &world,
            &mut log,
        );
        // Hold nothing until just before touchdown.
        let mut airborne = 0;
        while !p.grounded && airborne < 400 {
            p.step(dt, &idle(), &Triggers::default(), None, &world, &mut log);
            airborne += 1;
        }

        // Now airborne again, fall, and press jump one step before landing.
        p.step(
            dt,
            &MovementIntent {
                jump_pressed: true,
                ..idle()
            },
            &Triggers::default(),
            None,
            &world,
            &mut log,
        );
        log.clear();
        for _ in 0..400 {
            p.step(dt, &idle(), &Triggers::default(), None, &world, &mut log);
            if p.grounded {
                break;
            }
        }
        p.step(
            dt,
            &MovementIntent {
                jump_pressed: true,
                ..idle()
            },
            &Triggers::default(),
            None,
            &world,
            &mut log,
        );
        assert!(log.any("jump"), "a buffered jump should still fire");
    }

    #[test]
    fn sustained_thrust_drains_energy_and_lifts_the_mech() {
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();
        let dt = crate::config::sim::DT;

        p.step(
            dt,
            &MovementIntent {
                jump_pressed: true,
                ..idle()
            },
            &Triggers::default(),
            None,
            &world,
            &mut log,
        );

        let held = MovementIntent {
            jump_held: true,
            ..idle()
        };
        let energy_before = p.energy;
        for _ in 0..60 {
            p.step(dt, &held, &Triggers::default(), None, &world, &mut log);
        }
        assert!(p.energy < energy_before, "flight should cost energy");
        assert!(p.flying, "the mech should be flying");
    }

    #[test]
    fn flight_cannot_lift_the_mech_with_no_energy() {
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();
        let dt = crate::config::sim::DT;
        p.energy = 0.0;
        p.grounded = false;

        let held = MovementIntent {
            jump_held: true,
            ..idle()
        };
        p.step(dt, &held, &Triggers::default(), None, &world, &mut log);
        assert!(!p.flying, "an empty tank should not fly");
    }

    // -- energy -----------------------------------------------------------

    #[test]
    fn a_quick_boost_costs_energy_and_moves_the_mech() {
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();
        let dt = crate::config::sim::DT;

        let before = p.energy;
        p.step(
            dt,
            &MovementIntent {
                quick_boost_pressed: true,
                move_x: 0.0,
                move_z: 1.0,
                move_mag: 1.0,
                ..idle()
            },
            &Triggers::default(),
            None,
            &world,
            &mut log,
        );

        assert!(p.energy < before, "a boost should cost energy");
        assert!(log.any("quick-boost"));
        assert!(
            p.speed() > p.glide_speed,
            "a boost should exceed glide speed, got {}",
            p.speed()
        );
    }

    #[test]
    fn a_boost_with_no_direction_goes_backwards() {
        // Otherwise a dodge with no stick input would do nothing, which is
        // exactly when a player most wants it.
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();

        p.step(
            crate::config::sim::DT,
            &MovementIntent {
                quick_boost_pressed: true,
                ..idle()
            },
            &Triggers::default(),
            None,
            &world,
            &mut log,
        );
        assert!(p.speed() > 0.0, "the dodge should have moved the mech");
    }

    #[test]
    fn energy_regenerates_after_a_delay() {
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();
        let dt = crate::config::sim::DT;

        p.step(
            dt,
            &MovementIntent {
                quick_boost_pressed: true,
                ..idle()
            },
            &Triggers::default(),
            None,
            &world,
            &mut log,
        );
        let spent = p.energy;

        // Immediately after, nothing comes back: the delay is the point.
        p.step(dt, &idle(), &Triggers::default(), None, &world, &mut log);
        assert_eq!(p.energy, spent, "regeneration should be delayed");

        for _ in 0..240 {
            p.step(dt, &idle(), &Triggers::default(), None, &world, &mut log);
        }
        assert!(p.energy > spent, "it should have regenerated by now");
    }

    #[test]
    fn running_the_tank_empty_is_announced_once() {
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();
        let dt = crate::config::sim::DT;
        p.energy = 1.0;

        let held = MovementIntent {
            assault_held: true,
            ..idle()
        };
        for _ in 0..120 {
            p.step(dt, &held, &Triggers::default(), None, &world, &mut log);
        }
        assert_eq!(
            log.count("energy-empty"),
            1,
            "the announcement is edge triggered, not per step"
        );
    }

    // -- weapons ----------------------------------------------------------

    #[test]
    fn firing_consumes_ammunition_and_respects_the_rate() {
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();
        let dt = crate::config::sim::DT;

        let fire = Triggers {
            fire_primary: true,
            ..Triggers::default()
        };
        for _ in 0..120 {
            p.step(dt, &idle(), &fire, None, &world, &mut log);
        }

        // One second at 7.2 rounds per second is seven shots, not 120.
        let shots = log.count("fire");
        assert!(
            (6..=9).contains(&shots),
            "a second of fire should be about seven rounds, got {shots}"
        );
        assert_eq!(p.ammo, rifle::MAGAZINE - shots as u32);
    }

    #[test]
    fn an_empty_magazine_reloads_itself() {
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();
        let dt = crate::config::sim::DT;
        p.ammo = 0;

        let fire = Triggers {
            fire_primary: true,
            ..Triggers::default()
        };
        p.step(dt, &idle(), &fire, None, &world, &mut log);
        assert!(log.any("dry-fire"));
        assert!(log.any("reload-start"), "an empty gun should reload itself");

        for _ in 0..(rifle::RELOAD_TIME / dt) as u32 + 10 {
            p.step(dt, &idle(), &Triggers::default(), None, &world, &mut log);
        }
        assert_eq!(p.ammo, rifle::MAGAZINE, "the magazine should be full");
        assert!(log.any("reload-end"));
    }

    #[test]
    fn the_rifle_spread_is_deterministic() {
        // The one place the original was not. Two runs from the same state
        // must produce the same shots, or a replay cannot follow its own
        // recording.
        let world = world();
        let fire = Triggers {
            fire_primary: true,
            ..Triggers::default()
        };
        let dt = crate::config::sim::DT;

        let shots = || {
            let mut p = player();
            let mut log = EventLog::new();
            for _ in 0..120 {
                p.step(dt, &idle(), &fire, None, &world, &mut log);
            }
            log.events
                .iter()
                .filter_map(|e| match e {
                    SimEvent::Fire { direction, .. } => Some(*direction),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        let first = shots();
        let second = shots();
        assert!(!first.is_empty());
        assert_eq!(first, second, "the same state must produce the same shots");
    }

    #[test]
    fn firing_mid_swing_scatters_wider() {
        // Two runs differing only in blade phase, using the same generator, so
        // the comparison is fair.
        let world = world();
        let dt = crate::config::sim::DT;

        let deviation = |blade_phase: BladePhase| {
            let mut p = player();
            p.blade_phase = blade_phase;
            let mut angles = Vec::new();
            for _ in 0..40 {
                p.ammo = rifle::MAGAZINE;
                p.fire_cooldown = 0.0;
                let mut log = EventLog::new();
                p.step(
                    dt,
                    &idle(),
                    &Triggers {
                        fire_primary: true,
                        ..Triggers::default()
                    },
                    None,
                    &world,
                    &mut log,
                );
                if let Some(SimEvent::Fire { direction, .. }) = log.events.first() {
                    let flat = Vec3::new(direction.x, 0.0, direction.z).normalized();
                    angles.push(flat.x.atan2(flat.z).abs());
                }
            }
            angles.iter().sum::<f32>() / angles.len().max(1) as f32
        };

        let steady = deviation(BladePhase::Idle);
        let swinging = deviation(BladePhase::Active);
        assert!(
            swinging > steady,
            "a shot mid-swing should scatter wider: {swinging} vs {steady}"
        );
    }

    #[test]
    fn a_missile_volley_needs_a_lock() {
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();

        p.step(
            crate::config::sim::DT,
            &idle(),
            &Triggers {
                missile_pressed: true,
                ..Triggers::default()
            },
            // No lock.
            None,
            &world,
            &mut log,
        );
        assert!(log.any("dry-fire"), "firing without a lock should refuse");
        assert!(!log.any("missile-launch"));
    }

    #[test]
    fn a_missile_volley_fires_every_round_in_the_pod() {
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();
        let dt = crate::config::sim::DT;

        p.step(
            dt,
            &idle(),
            &Triggers {
                missile_pressed: true,
                ..Triggers::default()
            },
            Some(7),
            &world,
            &mut log,
        );

        for _ in 0..240 {
            p.step(dt, &idle(), &Triggers::default(), Some(7), &world, &mut log);
        }
        assert_eq!(
            log.count("missile-launch"),
            missile_cfg::COUNT as usize,
            "the whole pod should have emptied"
        );
        assert_eq!(p.volley_target(), Some(7));
    }

    #[test]
    fn the_blade_runs_through_its_three_phases() {
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();
        let dt = crate::config::sim::DT;

        p.step(
            dt,
            &idle(),
            &Triggers {
                blade_pressed: true,
                ..Triggers::default()
            },
            None,
            &world,
            &mut log,
        );

        let mut seen = Vec::new();
        for _ in 0..240 {
            p.step(dt, &idle(), &Triggers::default(), None, &world, &mut log);
            seen.push(p.blade_phase);
            if p.blade_phase == BladePhase::Idle && seen.len() > 5 {
                break;
            }
        }

        assert!(seen.contains(&BladePhase::Windup), "windup");
        assert!(seen.contains(&BladePhase::Active), "active");
        assert!(seen.contains(&BladePhase::Recovery), "recovery");
        assert_eq!(p.blade_phase, BladePhase::Idle, "and back to idle");
        assert!(log.count("blade") >= 3, "each phase is announced");
    }

    #[test]
    fn a_swing_hits_each_target_once() {
        // The rule that stops the blade's damage depending on frame timing.
        let mut p = player();
        assert!(p.register_blade_hit(4), "the first hit counts");
        assert!(!p.register_blade_hit(4), "the second does not");
        assert!(p.register_blade_hit(5), "a different target still counts");
    }

    #[test]
    fn a_new_swing_can_hit_the_same_target_again() {
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();
        let dt = crate::config::sim::DT;

        p.register_blade_hit(4);
        p.step(
            dt,
            &idle(),
            &Triggers {
                blade_pressed: true,
                ..Triggers::default()
            },
            None,
            &world,
            &mut log,
        );
        for _ in 0..240 {
            p.step(dt, &idle(), &Triggers::default(), None, &world, &mut log);
            if p.blade_phase == BladePhase::Idle {
                break;
            }
        }
        assert!(
            p.register_blade_hit(4),
            "a fresh swing should be able to hit the same enemy"
        );
    }

    #[test]
    fn the_blade_cannot_be_spammed() {
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();
        let dt = crate::config::sim::DT;

        let swing = Triggers {
            blade_pressed: true,
            ..Triggers::default()
        };
        for _ in 0..600 {
            p.step(dt, &idle(), &swing, None, &world, &mut log);
        }
        // Five seconds at one swing per (phases + cooldown) is a handful, not
        // six hundred.
        let swings = log.count("blade") / 3;
        assert!(
            (2..=6).contains(&swings),
            "the blade should be rate limited, got {swings} swings in five seconds"
        );
    }

    // -- damage -----------------------------------------------------------

    #[test]
    fn a_staggered_player_takes_more_damage() {
        let mut fresh = player();
        let mut staggered = player();
        // A new mech starts inside its spawn grace, during which it takes
        // nothing at all -- so both have to be past it for this to compare
        // anything.
        fresh.invuln_timer = 0.0;
        staggered.invuln_timer = 0.0;
        staggered.stagger_timer = 1.0;
        let mut log = EventLog::new();

        fresh.take_damage(100.0, 0.0, &mut log);
        staggered.take_damage(100.0, 0.0, &mut log);

        let fresh_loss = fresh.max_health - fresh.health;
        let staggered_loss = staggered.max_health - staggered.health;
        assert!(staggered_loss > fresh_loss);
    }

    #[test]
    fn the_spawn_grace_prevents_damage() {
        // A mission that starts with enemies already firing should not begin
        // with the player losing health before they can move.
        let mut p = player();
        let mut log = EventLog::new();
        assert!(p.invuln_timer > 0.0, "a new mech starts invulnerable");
        p.take_damage(100.0, 50.0, &mut log);
        assert_eq!(p.health, p.max_health);
    }

    #[test]
    fn enough_impact_staggers_and_announces_it_once() {
        let mut p = player();
        let mut log = EventLog::new();
        p.invuln_timer = 0.0;

        p.take_damage(1.0, 50.0, &mut log);
        assert!(!p.staggered(), "50 of 100 is not yet a break");
        p.take_damage(1.0, 60.0, &mut log);
        assert!(p.staggered());
        assert_eq!(log.count("player-stagger"), 1);

        // And it cannot retrigger while it lasts.
        for _ in 0..10 {
            p.take_damage(1.0, 50.0, &mut log);
        }
        assert_eq!(log.count("player-stagger"), 1);
    }

    #[test]
    fn running_out_of_health_destroys_the_mech() {
        let mut p = player();
        let mut log = EventLog::new();
        p.invuln_timer = 0.0;

        let (destroyed, _) = p.take_damage(10_000.0, 0.0, &mut log);
        assert!(destroyed);
        assert!(!p.alive);
        assert_eq!(p.health, 0.0);

        // And a corpse takes nothing more.
        let (again, _) = p.take_damage(100.0, 0.0, &mut log);
        assert!(!again);
    }

    #[test]
    fn stability_decays_after_a_delay() {
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();
        let dt = crate::config::sim::DT;
        p.invuln_timer = 0.0;

        p.take_damage(1.0, 40.0, &mut log);
        assert!(p.stability > 0.0);

        // During the delay it holds.
        for _ in 0..30 {
            p.step(dt, &idle(), &Triggers::default(), None, &world, &mut log);
        }
        assert!(p.stability > 0.0, "stability should hold during the delay");

        // Then it recovers.
        for _ in 0..300 {
            p.step(dt, &idle(), &Triggers::default(), None, &world, &mut log);
        }
        assert_eq!(p.stability, 0.0);
    }

    // -- repair -----------------------------------------------------------

    #[test]
    fn a_repair_heals_over_time_and_is_limited() {
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();
        let dt = crate::config::sim::DT;
        p.health = 100.0;

        p.step(
            dt,
            &idle(),
            &Triggers {
                repair_pressed: true,
                ..Triggers::default()
            },
            None,
            &world,
            &mut log,
        );
        assert_eq!(p.repairs, cfg::REPAIRS - 1);
        assert!(p.repair_timer > 0.0);

        for _ in 0..(cfg::REPAIR_TIME / dt) as u32 + 10 {
            p.step(dt, &idle(), &Triggers::default(), None, &world, &mut log);
        }
        assert!(p.health > 100.0, "the repair should have healed");
        assert!(log.any("repair-end"));
    }

    #[test]
    fn a_repair_refuses_at_full_health() {
        // Otherwise a player wastes a charge, which feels like the game
        // cheating them.
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();
        p.step(
            crate::config::sim::DT,
            &idle(),
            &Triggers {
                repair_pressed: true,
                ..Triggers::default()
            },
            None,
            &world,
            &mut log,
        );
        assert_eq!(p.repairs, cfg::REPAIRS);
        assert_eq!(p.repair_timer, 0.0);
    }

    #[test]
    fn repairs_run_out() {
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();
        let dt = crate::config::sim::DT;

        for _ in 0..cfg::REPAIRS + 3 {
            p.health = 100.0;
            p.repair_timer = 0.0;
            p.step(
                dt,
                &idle(),
                &Triggers {
                    repair_pressed: true,
                    ..Triggers::default()
                },
                None,
                &world,
                &mut log,
            );
            // Let the repair finish so the next one is allowed.
            for _ in 0..(cfg::REPAIR_TIME / dt) as u32 + 2 {
                p.step(dt, &idle(), &Triggers::default(), None, &world, &mut log);
            }
        }
        assert_eq!(p.repairs, 0, "charges should not go below zero");
    }

    // -- reset ------------------------------------------------------------

    #[test]
    fn a_reset_restores_everything() {
        // Restart has to be clean, or a second mission starts with the first
        // one's damage.
        // No world and no stepping: this is about what reset puts back, not
        // about what a step does.
        let mut p = player();
        let mut log = EventLog::new();

        p.take_damage(500.0, 50.0, &mut log);
        p.energy = 3.0;
        p.ammo = 2;
        p.repairs = 0;
        p.pos = Vec3::new(50.0, 10.0, 50.0);
        p.blade_phase = BladePhase::Active;
        p.register_blade_hit(9);

        p.reset(spawn(), MechPresetId::Heavy);

        assert_eq!(p.health, p.max_health);
        assert_eq!(p.energy, p.energy_max);
        assert_eq!(p.ammo, rifle::MAGAZINE);
        assert_eq!(p.repairs, cfg::REPAIRS);
        assert_eq!(p.pos, spawn());
        assert_eq!(p.blade_phase, BladePhase::Idle);
        assert_eq!(p.stability, 0.0);
        assert!(p.alive);
    }

    #[test]
    fn repeated_resets_do_not_accumulate_state() {
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();
        let dt = crate::config::sim::DT;

        for _ in 0..5 {
            for _ in 0..60 {
                p.step(dt, &forward(), &Triggers::default(), None, &world, &mut log);
            }
            p.reset(spawn(), MechPresetId::Balanced);
            assert_eq!(p.pos, spawn());
            assert_eq!(p.vel, Vec3::ZERO);
            assert_eq!(p.speed(), 0.0);
        }
    }

    // -- the arena bounds -------------------------------------------------

    #[test]
    fn the_mech_cannot_leave_the_arena() {
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();
        let dt = crate::config::sim::DT;

        // Aimed straight out, held for a long time.
        let outward = MovementIntent {
            move_x: 1.0,
            move_z: 0.0,
            move_mag: 1.0,
            aim_x: 1.0,
            aim_z: 0.0,
            ..MovementIntent::default()
        };
        for _ in 0..2000 {
            p.step(dt, &outward, &Triggers::default(), None, &world, &mut log);
        }

        let limit = crate::config::world::HALF - p.radius;
        assert!(
            p.pos.x.abs() <= limit + 0.01,
            "the mech escaped to x = {}, limit {limit}",
            p.pos.x
        );
    }

    #[test]
    fn the_mech_stands_on_the_floor() {
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();
        for _ in 0..120 {
            p.step(
                crate::config::sim::DT,
                &idle(),
                &Triggers::default(),
                None,
                &world,
                &mut log,
            );
        }
        assert!((p.pos.y - crate::config::world::FLOOR_Y).abs() < 0.05);
        assert!(p.grounded);
    }

    #[test]
    fn the_muzzles_sit_where_the_shoulders_are() {
        let p = player();
        let rifle = p.rifle_muzzle();
        let pod = p.missile_muzzle();
        assert!(rifle.y > p.pos.y, "the muzzle is above the feet");
        assert!(pod.y > rifle.y, "the pod sits above the cannon");
        assert!(
            (rifle.x - pod.x).abs() > 1.0,
            "the two mounts are on opposite shoulders"
        );
    }

    #[test]
    fn the_body_lags_behind_the_aim_without_delaying_it() {
        // The design rule: control is immediate, appearance is not. Facing is
        // exact on the step the aim changes; the body catches up over time.
        let world = world();
        let mut p = player();
        let mut log = EventLog::new();

        let turned = MovementIntent {
            aim_x: 1.0,
            aim_z: 0.0,
            ..idle()
        };
        p.step(
            crate::config::sim::DT,
            &turned,
            &Triggers::default(),
            None,
            &world,
            &mut log,
        );

        assert!(
            (p.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-4,
            "facing should be immediate, got {}",
            p.yaw
        );
        assert!(
            p.body_yaw < p.yaw,
            "the body should still be catching up, got {}",
            p.body_yaw
        );
    }

    #[test]
    fn a_snapshot_reports_what_is_actually_set() {
        let p = player();
        let s = p.snapshot();
        assert_eq!(s.health, p.health);
        assert_eq!(s.energy, p.energy);
        assert_eq!(s.grounded, p.grounded);
        assert_eq!(s.blade_phase, p.blade_phase);
        assert_eq!(s.staggered, p.staggered());
    }
}
