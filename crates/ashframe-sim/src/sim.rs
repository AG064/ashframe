//! The simulation.
//!
//! Ported from the original's `game/world.ts`: *"This is the whole game state
//! without a single renderer object: collision, the player, enemies,
//! projectiles, targeting and mission flow. It is driven by fixed steps and
//! reports everything that happened through `SimHooks`, so the same code runs in
//! the browser and in headless tests."*
//!
//! That property is the reason the port was possible at all, and it is what the
//! Godot layer is written against: it reads this state and forwards these
//! events, and nothing here knows the renderer exists.
//!
//! ## Borrowing, and why the sink is built per step
//!
//! The original's event sink was a field on the simulation holding a closure
//! that captured `this`, so a rifle shot could spawn a bullet, bump a statistic
//! and reach back into the player for recoil, all from inside the player's own
//! step. Rust cannot hold a borrow of four sibling fields in a long-lived field.
//!
//! [`Sink`] is that same object, rebuilt from disjoint field borrows wherever it
//! is needed. Two consequences are worth knowing:
//!
//! - Bullets and missiles fired during the player's step are queued and spawned
//!   immediately afterwards, rather than mid-step. Within one 120 Hz tick that
//!   is the same frame, and the round still exists before the enemies act and
//!   before ordnance moves.
//! - Recoil is applied by the gun rather than by the sink, because the sink
//!   cannot borrow the player it is being called from.
//!
//! ## Damage, and the return trip
//!
//! Projectiles and blast damage talk to a `&mut [Damageable]`, which is a slice
//! of *snapshots*: a player or an enemy is richer than the interface combat
//! needs. Damage therefore lands on the copies and is folded back with
//! [`absorb`] after each phase that can deal it. The list is rebuilt between the
//! enemy phase and the ordnance phase rather than reused, so a unit's own timer
//! tick is never overwritten by a snapshot taken before it ran.

use crate::arena::{self, Arena, SpawnPoints};
use crate::camera::{Assist, CameraTarget};
use crate::collision::CollisionWorld;
use crate::combat::{absorb, apply_damage};
use crate::config::{
    blade as blade_cfg, missiles as missile_cfg, player as player_cfg, rifle, world as world_cfg,
    MechPresetId,
};
use crate::enemies::{EnemyRoster, EnemyWorld};
use crate::math::{clamp, Rng};
use crate::mission::{Mission, MissionFacts, MissionHooks, SpawnOrder};
use crate::player::{MovementIntent, Player, Triggers};
use crate::projectiles::{BulletSpec, MissileSpec, ProjectileSystem};
use crate::targeting::{TargetingSystem, TargetingView};
use crate::types::{BladePhase, Damageable, EnemyKind, Faction, Hooks, SimEvent, Vec3, WeaponId};

/// Seed for the world's generator.
///
/// Fixed, because a replay has to reproduce the same scatter and the same enemy
/// choices. The original seeded the same value.
const SEED: u32 = 0x51ed_270b;

/// Per-step control state, resolved from named actions by the layer above.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ControlState {
    pub move_x: f32,
    pub move_z: f32,
    pub move_mag: f32,
    pub jump_held: bool,
    pub jump_pressed: bool,
    pub quick_boost_pressed: bool,
    pub assault_held: bool,
    pub fire_primary: bool,
    pub missile_pressed: bool,
    pub blade_pressed: bool,
    pub reload_pressed: bool,
    pub repair_pressed: bool,
    pub toggle_lock_pressed: bool,
    /// `-1`, `0` or `+1` from the wheel.
    pub cycle_dir: f32,
}

/// Ordnance the sink queued during a step, spawned once the borrow ends.
#[derive(Debug, Clone, PartialEq)]
enum PendingSpawn {
    Rifle(BulletSpec),
    Missile(MissileSpec),
}

/// Turns gameplay events into world changes and statistics, then forwards what
/// is left to the app.
struct Sink<'a> {
    mission: &'a mut Mission,
    app: &'a mut dyn Hooks,
    pending: &'a mut Vec<PendingSpawn>,
    player_id: u32,
    /// The lock at the moment this sink was built. A missile launched during the
    /// step homes on what was locked when the trigger was pulled.
    lock: Option<u32>,
}

impl Hooks for Sink<'_> {
    fn emit(&mut self, event: &SimEvent) {
        match event {
            SimEvent::Fire {
                weapon: WeaponId::Rifle,
                origin,
                direction,
            } => {
                self.pending.push(PendingSpawn::Rifle(BulletSpec {
                    weapon: WeaponId::Rifle,
                    faction: Faction::Player,
                    owner: self.player_id,
                    at: *origin,
                    direction: *direction,
                    speed: rifle::SPEED,
                    damage: rifle::DAMAGE,
                    impact: rifle::IMPACT,
                    life: rifle::RANGE / rifle::SPEED,
                    radius: 0.3,
                }));
                self.mission.stats.shots_fired += 1;
            }
            SimEvent::MissileLaunch { origin, direction } => {
                self.pending.push(PendingSpawn::Missile(MissileSpec {
                    faction: Faction::Player,
                    owner: self.player_id,
                    target: self.lock,
                    at: *origin,
                    direction: *direction,
                    speed: missile_cfg::SPEED,
                    damage: missile_cfg::DAMAGE,
                    splash_radius: missile_cfg::SPLASH_RADIUS,
                    splash_damage: missile_cfg::SPLASH_DAMAGE,
                    turn_rate: missile_cfg::TURN_RATE,
                    life: missile_cfg::LIFE,
                }));
            }
            // The player's own hits are reported in the terms the app speaks —
            // damage and stagger — rather than as raw combat events, and are
            // not forwarded twice.
            SimEvent::Hit {
                target, amount, at, ..
            } if *target == self.player_id => {
                self.mission.stats.damage_taken += amount;
                self.app.emit(&SimEvent::PlayerDamage {
                    amount: *amount,
                    at: *at,
                });
                return;
            }
            SimEvent::Stagger { target, .. } if *target == self.player_id => {
                self.app.emit(&SimEvent::PlayerStagger);
                return;
            }
            SimEvent::Destroy { target, kind, .. } if *target != self.player_id => {
                // An unrecognised label is not scored. Guessing a default would
                // make a renamed archetype look like it was worth points.
                if let Some(archetype) = EnemyKind::from_label(kind) {
                    self.mission.register_kill(archetype);
                }
            }
            SimEvent::RepairStart { .. } => {
                self.mission.stats.repairs_used += 1;
            }
            _ => {}
        }
        self.app.emit(event);
    }
}

/// Puts the mission's orders into the world.
///
/// The original threaded an arena object through the app and into the mission so
/// a spawn closure could reach it. The simulation owns the arena here, so the
/// mission never sees it: it asks for `skirmisher` slot 3 and this decides where
/// that is, nudges the unit clear of scenery, and turns it to face the player.
struct MissionSink<'a> {
    roster: &'a mut EnemyRoster,
    spawns: &'a SpawnPoints,
    world: &'a CollisionWorld,
    player: &'a Player,
    app: &'a mut dyn Hooks,
    time: f32,
}

impl MissionHooks for MissionSink<'_> {
    fn spawn(&mut self, order: SpawnOrder) {
        let slots: &[Vec3] = match order.kind {
            EnemyKind::Skirmisher => &self.spawns.skirmisher,
            EnemyKind::Artillery => &self.spawns.artillery,
            EnemyKind::Boss => std::slice::from_ref(&self.spawns.boss),
        };
        // A slot that does not exist falls back to the player's own spawn
        // rather than dropping the unit: a wave that quietly arrives one short
        // would stall the mission with nothing to shoot.
        let base = match slots.get(order.slot % slots.len().max(1)) {
            Some(point) => *point,
            None => self.spawns.player,
        };

        let enemy = self.roster.obtain(order.kind);
        let placed = arena::resolve_spawn(self.world, base, enemy.radius, enemy.height);
        let yaw = (self.player.pos.x - placed.x).atan2(self.player.pos.z - placed.z);
        enemy.revive(placed);
        enemy.yaw = yaw;
    }

    fn set_objective(&mut self, text: &str) {
        self.app.emit(&SimEvent::Objective {
            text: text.to_string(),
        });
    }

    fn complete(&mut self) {
        self.app
            .emit(&SimEvent::MissionComplete { time: self.time });
    }

    fn fail(&mut self) {
        self.app.emit(&SimEvent::MissionFailed);
    }
}

/// The whole game, without a renderer.
/// The whole game, without a renderer.
pub struct Simulation {
    pub collision: CollisionWorld,
    pub arena: Arena,
    pub projectiles: ProjectileSystem,
    pub roster: EnemyRoster,
    pub targeting: TargetingSystem,
    pub mission: Mission,
    pub player: Player,

    /// Camera pose used for aiming. The camera controller writes this before
    /// the simulation steps, which is what makes aiming start from what the
    /// player can actually see.
    pub view: TargetingView,
    /// Where the reticle lands, after world and enemy intersection.
    pub aim_point: Vec3,

    pub preset: MechPresetId,
    pub time: f32,

    rng: Rng,
    pending: Vec<PendingSpawn>,
    targets: Vec<Damageable>,
    /// Whether the player is mid-recovery from leaving the world, which stops
    /// the out-of-bounds check firing repeatedly while it is being placed.
    recovering: bool,
}

impl Simulation {
    pub fn new(preset: MechPresetId) -> Self {
        let mut collision = CollisionWorld::new(world_cfg::FLOOR_Y);
        let arena = arena::build(&mut collision);
        let spawn = arena.spawns.player;
        Self {
            collision,
            arena,
            projectiles: ProjectileSystem::new(),
            roster: EnemyRoster::new(),
            targeting: TargetingSystem::new(),
            mission: Mission::new(),
            player: Player::new(preset, spawn),
            view: TargetingView::default(),
            aim_point: Vec3::ZERO,
            preset,
            time: 0.0,
            rng: Rng::new(SEED),
            pending: Vec::new(),
            targets: Vec::new(),
            recovering: false,
        }
    }

    /// Full reset for a restart: enemies, ordnance, timers, statistics and view.
    pub fn reset(&mut self, preset: MechPresetId) {
        self.preset = preset;
        self.time = 0.0;
        self.rng = Rng::new(SEED);
        self.roster.clear();
        self.projectiles.clear();
        self.targeting.reset();
        self.mission.reset();
        self.recovering = false;
        self.pending.clear();
        self.targets.clear();

        let spawn = self.arena.spawns.player;
        self.player.reset(spawn, preset);
        self.view.yaw = 0.0;
        self.view.pitch = -0.08;
        self.view.at = spawn + Vec3::new(0.0, player_cfg::HEIGHT * 0.85, -6.0);
    }

    /// Advance one fixed step.
    pub fn step(&mut self, dt: f32, control: &ControlState, app: &mut dyn Hooks) {
        self.time += dt;
        self.update_aim_point();

        self.step_player(dt, control, app);
        self.rebuild_targets();
        self.step_targeting_input(control, app);
        self.step_enemies(dt, app);
        // Rebuilt from post-step state, so a unit's own timer tick is not
        // overwritten by a snapshot taken before it ran.
        self.rebuild_targets();
        self.step_ordnance(dt, app);
        self.rebuild_targets();
        // Targeting validates against the world as it now is, and runs after
        // ordnance so a lock is not held onto a wreck that died this step.
        self.validate_targeting(dt, app);

        self.step_mission(dt, app);
        self.check_out_of_bounds(app);
    }

    fn step_player(&mut self, dt: f32, control: &ControlState, app: &mut dyn Hooks) {
        if !self.player.alive {
            return;
        }
        let intent = MovementIntent {
            move_x: control.move_x,
            move_z: control.move_z,
            move_mag: control.move_mag,
            jump_held: control.jump_held,
            jump_pressed: control.jump_pressed,
            quick_boost_pressed: control.quick_boost_pressed,
            assault_held: control.assault_held,
            // The mech faces the camera, not the movement stick. Turning is
            // therefore about where the player is looking, which is the only
            // thing that makes strafing read as strafing.
            aim_x: self.view.yaw.sin(),
            aim_z: self.view.yaw.cos(),
        };
        let triggers = Triggers {
            fire_primary: control.fire_primary,
            missile_pressed: control.missile_pressed,
            blade_pressed: control.blade_pressed,
            reload_pressed: control.reload_pressed,
            repair_pressed: control.repair_pressed,
        };

        let lock = self.targeting.locked_id();
        let player_id = self.player.id;
        self.pending.clear();
        {
            let mut sink = Sink {
                mission: &mut self.mission,
                app,
                pending: &mut self.pending,
                player_id,
                lock,
            };
            self.player
                .step(dt, &intent, &triggers, lock, &self.collision, &mut sink);
        }
        self.flush_pending();
        self.resolve_blade(app);
    }

    /// Spawn everything queued by the sink during the step just taken.
    fn flush_pending(&mut self) {
        for spawn in self.pending.drain(..) {
            match spawn {
                PendingSpawn::Rifle(spec) => self.projectiles.spawn_bullet(spec),
                PendingSpawn::Missile(spec) => self.projectiles.spawn_missile(spec),
            }
        }
    }

    /// Resolve the energy blade against every enemy inside the sweep volume.
    fn resolve_blade(&mut self, app: &mut dyn Hooks) {
        if self.player.blade_phase != BladePhase::Active {
            return;
        }
        let px = self.player.pos.x;
        let py = self.player.pos.y + self.player.height * 0.5;
        let pz = self.player.pos.z;
        let fx = self.player.body_yaw.sin();
        let fz = self.player.body_yaw.cos();
        let cos_arc = blade_cfg::ARC.cos();

        let player_id = self.player.id;
        let mut hits: Vec<(u32, Vec3, bool)> = Vec::new();
        for enemy in self.roster.list.iter_mut() {
            if !enemy.alive {
                continue;
            }
            let dx = enemy.pos.x - px;
            let dz = enemy.pos.z - pz;
            let dist = (dx * dx + dz * dz).sqrt();
            if dist > blade_cfg::RANGE + enemy.radius {
                continue;
            }
            let ey = enemy.pos.y + enemy.height * 0.5;
            if (ey - py).abs() > blade_cfg::VERTICAL {
                continue;
            }
            // Directly on top of the mech counts as in front of it, rather than
            // dividing by zero to decide.
            let dot = if dist < 1e-4 {
                1.0
            } else {
                (dx * fx + dz * fz) / dist
            };
            if dot < cos_arc {
                continue;
            }
            if !self.player.register_blade_hit(enemy.id) {
                continue;
            }
            let mut copy = enemy.as_damageable();
            let result = {
                let mut sink = Sink {
                    mission: &mut self.mission,
                    app,
                    pending: &mut self.pending,
                    player_id,
                    lock: None,
                };
                apply_damage(
                    &mut copy,
                    blade_cfg::DAMAGE,
                    blade_cfg::IMPACT,
                    &mut sink,
                    1.0,
                )
            };
            absorb(&copy, enemy.combat_fields());
            hits.push((
                enemy.id,
                Vec3::new(enemy.pos.x, ey, enemy.pos.z),
                result.destroyed,
            ));
        }

        for (target, at, destroyed) in hits {
            app.emit(&SimEvent::BladeHit {
                at,
                target,
                destroyed,
            });
        }
    }

    fn step_targeting_input(&mut self, control: &ControlState, app: &mut dyn Hooks) {
        if control.toggle_lock_pressed {
            if self.targeting.locked_id().is_some() {
                self.targeting.clear_lock(app);
            } else {
                self.targeting
                    .acquire(&self.view, &self.targets, &self.collision, app);
            }
        }
        if control.cycle_dir != 0.0 {
            self.targeting.cycle(
                control.cycle_dir,
                &self.view,
                &self.targets,
                &self.collision,
                app,
            );
        }
    }

    fn validate_targeting(&mut self, dt: f32, app: &mut dyn Hooks) {
        self.targeting
            .update(dt, &self.view, &self.targets, &self.collision, app);
    }

    fn step_enemies(&mut self, dt: f32, app: &mut dyn Hooks) {
        let target = if self.player.alive {
            Some(self.player.pos)
        } else {
            None
        };
        let target_alive = self.player.alive;
        let time = self.time;
        let player_id = self.player.id;
        {
            let mut sink = Sink {
                mission: &mut self.mission,
                app,
                pending: &mut self.pending,
                player_id,
                lock: None,
            };
            let mut ctx = EnemyWorld {
                world: &self.collision,
                projectiles: &mut self.projectiles,
                hooks: &mut sink,
                target,
                target_alive,
                damageables: &mut self.targets,
                rng: &mut self.rng,
                time,
            };
            self.roster.step(dt, &mut ctx);
        }
        // The boss's slam damages the snapshot of the player, not the player.
        self.absorb_targets();
    }

    fn step_ordnance(&mut self, dt: f32, app: &mut dyn Hooks) {
        let player_id = self.player.id;
        {
            let mut sink = Sink {
                mission: &mut self.mission,
                app,
                pending: &mut self.pending,
                player_id,
                lock: None,
            };
            self.projectiles
                .step(dt, &self.collision, &mut self.targets, &mut sink);
        }
        self.absorb_targets();
    }

    /// Rebuild the shared damageable list: the player, then every living enemy
    /// in roster order.
    ///
    /// Reused rather than reallocated, because this runs three times a step.
    fn rebuild_targets(&mut self) {
        self.targets.clear();
        self.targets.push(self.player.as_damageable());
        for enemy in &self.roster.list {
            if enemy.alive {
                self.targets.push(enemy.as_damageable());
            }
        }
    }

    /// Fold damage taken on the snapshots back into the mechs that own it.
    ///
    /// Index zero is always the player. The rest were pushed in roster order and
    /// only for living units, so this walks the roster the same way — and a unit
    /// killed by the damage being folded in is skipped from that point on, which
    /// is why the index and the walk advance together rather than apart.
    fn absorb_targets(&mut self) {
        if let Some(first) = self.targets.first() {
            absorb(first, self.player.combat_fields());
        }
        let mut index = 1;
        for enemy in self.roster.list.iter_mut() {
            if !enemy.alive {
                continue;
            }
            if let Some(updated) = self.targets.get(index) {
                absorb(updated, enemy.combat_fields());
            }
            index += 1;
        }
    }

    fn step_mission(&mut self, dt: f32, app: &mut dyn Hooks) {
        let facts = MissionFacts {
            alive_enemies: self.roster.active_count(),
            boss_alive: self
                .roster
                .list
                .iter()
                .any(|e| e.alive && e.kind == EnemyKind::Boss),
            player_alive: self.player.alive,
        };
        let time = self.time;
        let mut hooks = MissionSink {
            roster: &mut self.roster,
            spawns: &self.arena.spawns,
            world: &self.collision,
            player: &self.player,
            app,
            time,
        };
        self.mission.update(dt, facts, &mut hooks);
    }

    /// Aim point: the first thing the camera ray meets, or far along the ray.
    fn update_aim_point(&mut self) {
        let direction = self.view.forward();
        let max_dist = rifle::RANGE;
        let mut best = match self.collision.raycast(self.view.at, direction, max_dist) {
            Some(hit) => hit.t,
            None => max_dist,
        };
        // Enemies block the ray too, so aiming at a mech aims at the mech
        // rather than at the wall behind it.
        for enemy in &self.roster.list {
            if !enemy.alive {
                continue;
            }
            let centre = Vec3::new(enemy.pos.x, enemy.pos.y + enemy.height * 0.5, enemy.pos.z);
            let half = Vec3::new(enemy.radius * 1.05, enemy.height * 0.5, enemy.radius * 1.05);
            if let Some(t) = ray_box(self.view.at, direction, best, centre, half) {
                if t < best {
                    best = t;
                }
            }
        }
        self.aim_point = self.view.at + direction * best;
        self.player.aim_point = self.aim_point;
    }

    /// Recover the mech if it somehow leaves the world.
    fn check_out_of_bounds(&mut self, app: &mut dyn Hooks) {
        if self.player.pos.y > world_cfg::KILL_PLANE && !self.recovering {
            return;
        }
        self.recovering = true;
        let (x, z) = self
            .collision
            .nearest_ground(self.player.pos.x, self.player.pos.z, 6.0, 8);
        let y = self.collision.ground_at(x, z, 200.0, 0.0) + 0.5;
        self.player.pos = Vec3::new(x, y, z);
        self.player.vel = Vec3::ZERO;
        app.emit(&SimEvent::OutOfBounds {
            at: Vec3::new(x, y, z),
        });
        self.recovering = false;
    }

    /// The live boss, if there is one.
    pub fn boss(&self) -> Option<&crate::enemies::Enemy> {
        self.roster
            .list
            .iter()
            .find(|e| e.alive && e.kind == EnemyKind::Boss)
    }

    /// Where the camera is following. Callers build this from the player.
    pub fn camera_target(&self) -> CameraTarget {
        CameraTarget {
            at: self.player.pos,
            height: self.player.height,
            yaw: self.player.body_yaw,
            assaulting: self.player.assaulting,
            speed: self.player.speed(),
        }
    }

    /// The assist the targeting system offers the camera this frame.
    pub fn camera_assist(&self, dt: f32) -> Assist {
        let enemies = self
            .roster
            .list
            .iter()
            .map(|e| e.as_damageable())
            .collect::<Vec<_>>();
        let (yaw, pitch) = self.targeting.assist(dt, &self.view, &enemies);
        Assist { yaw, pitch }
    }

    /// Clamp a world position into the arena bounds.
    pub fn clamp_to_arena(v: &mut Vec3) {
        let lim = world_cfg::HALF - 8.0;
        v.x = clamp(v.x, -lim, lim);
        v.z = clamp(v.z, -lim, lim);
    }
}

/// Ray against an axis-aligned box centred at `centre`; the entry distance, if
/// the ray enters within `max_dist`.
fn ray_box(origin: Vec3, direction: Vec3, max_dist: f32, centre: Vec3, half: Vec3) -> Option<f32> {
    let mut t0 = 0.0f32;
    let mut t1 = max_dist;
    let axes = [
        (origin.x, direction.x, centre.x - half.x, centre.x + half.x),
        (origin.y, direction.y, centre.y - half.y, centre.y + half.y),
        (origin.z, direction.z, centre.z - half.z, centre.z + half.z),
    ];
    for (o, d, lo, hi) in axes {
        if d.abs() < 1e-9 {
            if o < lo || o > hi {
                return None;
            }
            continue;
        }
        let mut a = (lo - o) / d;
        let mut b = (hi - o) / d;
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        if a > t0 {
            t0 = a;
        }
        if b < t1 {
            t1 = b;
        }
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{MechPresetId, MECH_PRESETS};
    use crate::types::EventLog;

    /// A simulation with an event log, driven the way a game loop drives it.
    struct Harness {
        sim: Simulation,
        log: EventLog,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                sim: Simulation::new(MechPresetId::Balanced),
                log: EventLog::new(),
            }
        }

        fn run(&mut self, seconds: f32, control: &ControlState) {
            let steps = (seconds / crate::config::sim::DT).round() as u32;
            for _ in 0..steps {
                self.sim
                    .step(crate::config::sim::DT, control, &mut self.log);
            }
        }

        fn step(&mut self, control: &ControlState) {
            self.sim
                .step(crate::config::sim::DT, control, &mut self.log);
        }

        /// Put one enemy in the world at a chosen spot, bypassing the mission.
        fn place(&mut self, kind: EnemyKind, at: Vec3) -> u32 {
            let enemy = self.sim.roster.obtain(kind);
            enemy.revive(at);
            enemy.invuln_timer = 0.0;
            enemy.id
        }

        /// Kill everything currently standing, the way a player would.
        fn clear_the_yard(&mut self) {
            for enemy in self.sim.roster.list.iter_mut() {
                if enemy.alive {
                    enemy.health = 0.0;
                    enemy.alive = false;
                }
            }
        }
    }

    fn idle() -> ControlState {
        ControlState::default()
    }

    fn holding() -> ControlState {
        ControlState {
            fire_primary: true,
            ..ControlState::default()
        }
    }

    #[test]
    fn the_player_spawns_in_the_arena_facing_down_it() {
        let h = Harness::new();
        assert!(h.sim.player.alive);
        assert_eq!(h.sim.player.pos, h.sim.arena.spawns.player);
        assert!(
            h.sim
                .collision
                .ground_at(h.sim.player.pos.x, h.sim.player.pos.z, 200.0, 400.0)
                <= h.sim.player.pos.y + 0.01
        );
    }

    #[test]
    fn a_round_fired_at_a_mech_in_front_of_it_damages_that_mech() {
        // The whole point of the damage snapshot: damage lands on a copy, and
        // this is the test that fails if the return trip is missing.
        let mut h = Harness::new();
        let at = h.sim.player.pos + Vec3::new(0.0, 0.0, -24.0);
        let id = h.place(EnemyKind::Skirmisher, at);

        // Facing -Z, straight down the yard.
        h.sim.view.yaw = std::f32::consts::PI;
        h.sim.view.pitch = 0.0;
        h.sim.view.at = h.sim.player.pos + Vec3::new(0.0, 4.0, 0.0);

        let before = h.sim.roster.by_id(id).unwrap().health;
        h.run(2.0, &holding());

        assert!(h.sim.mission.stats.shots_fired > 0, "nothing was fired");
        let after = h.sim.roster.by_id(id).unwrap().health;
        assert!(
            after < before,
            "the target took no damage at all: {before} -> {after}"
        );
    }

    #[test]
    fn a_kill_is_counted_and_scored_through_the_real_damage_path() {
        let mut h = Harness::new();
        let at = h.sim.player.pos + Vec3::new(0.0, 0.0, -24.0);
        let id = h.place(EnemyKind::Skirmisher, at);

        // A lethal round down the aim axis, resolved by the same swept path the
        // autocannon uses.
        h.sim.projectiles.spawn_bullet(BulletSpec {
            weapon: WeaponId::Rifle,
            faction: Faction::Player,
            owner: h.sim.player.id,
            at: h.sim.player.pos + Vec3::new(0.0, 2.0, -10.0),
            direction: Vec3::new(0.0, 0.0, -1.0),
            speed: 300.0,
            damage: 10_000.0,
            impact: 0.0,
            life: 1.0,
            radius: 0.3,
        });

        h.run(0.4, &idle());

        assert!(!h.sim.roster.by_id(id).unwrap().alive);
        assert_eq!(h.sim.mission.stats.kills, 1);
        assert_eq!(h.sim.mission.stats.score, 100);
        assert!(h.sim.mission.stats.elapsed > 0.3);
    }

    #[test]
    fn the_player_takes_damage_from_the_world_and_the_stats_see_it() {
        let mut h = Harness::new();
        // Spawn protection would swallow the hit; a mech that has been in the
        // yard for a while is the case being tested.
        h.sim.player.invuln_timer = 0.0;
        let before = h.sim.player.health;

        // An enemy round arriving at the player, spawned the way an enemy
        // would spawn it.
        let mut shot = crate::projectiles::EnemyShotSpec::new(
            WeaponId::EnemyGun,
            999,
            h.sim.player.pos + Vec3::new(0.0, 3.0, -12.0),
            Vec3::new(0.0, 0.0, 1.0),
            200.0,
            1.0,
        );
        shot.damage = 40.0;
        h.sim.projectiles.spawn_enemy(shot);
        h.run(0.3, &idle());

        assert!(h.sim.player.health < before, "the player never got hit");
        assert!(h.sim.mission.stats.damage_taken > 0.0);
        assert!(h.log.any("player-damage"));
    }

    #[test]
    fn the_mission_walks_from_the_intro_to_the_boss_and_can_be_won() {
        let mut h = Harness::new();
        assert_eq!(h.sim.mission.phase, crate::types::MissionPhase::Intro);

        h.run(3.4, &idle());
        assert_eq!(h.sim.mission.phase, crate::types::MissionPhase::Wave1);
        assert_eq!(h.sim.roster.active_count(), 3, "the screen is three units");

        h.clear_the_yard();
        h.run(0.05, &idle());
        assert_eq!(h.sim.mission.phase, crate::types::MissionPhase::Gap1);

        h.run(2.6, &idle());
        assert_eq!(h.sim.mission.phase, crate::types::MissionPhase::Wave2);
        assert_eq!(
            h.sim
                .roster
                .list
                .iter()
                .filter(|e| e.alive && e.kind == EnemyKind::Artillery)
                .count(),
            2
        );

        h.clear_the_yard();
        h.run(0.05, &idle());
        assert_eq!(h.sim.mission.phase, crate::types::MissionPhase::Gap2);

        h.run(2.6, &idle());
        assert_eq!(h.sim.mission.phase, crate::types::MissionPhase::Boss);
        assert!(h.sim.boss().is_some(), "the Patriarch never arrived");

        h.clear_the_yard();
        h.run(0.05, &idle());
        assert_eq!(h.sim.mission.phase, crate::types::MissionPhase::Victory);
        assert!(h.log.any("mission-complete"));
        assert!(h.log.any("objective"));
    }

    #[test]
    fn losing_the_frame_fails_the_mission() {
        let mut h = Harness::new();
        h.run(3.4, &idle());
        h.sim.player.health = 1.0;
        h.sim.player.invuln_timer = 0.0;
        h.sim.player.take_damage(500.0, 0.0, &mut h.log);

        h.run(0.05, &idle());
        assert!(!h.sim.player.alive);
        assert_eq!(h.sim.mission.phase, crate::types::MissionPhase::Defeat);
        assert!(h.log.any("mission-failed"));
    }

    #[test]
    fn a_missile_volley_needs_a_lock() {
        let mut h = Harness::new();
        let at = h.sim.player.pos + Vec3::new(0.0, 0.0, -30.0);
        let id = h.place(EnemyKind::Skirmisher, at);
        h.sim.view.yaw = std::f32::consts::PI;
        h.sim.view.pitch = 0.0;
        h.sim.view.at = h.sim.player.pos + Vec3::new(0.0, 4.0, 0.0);

        h.log.clear();
        h.step(&ControlState {
            missile_pressed: true,
            ..ControlState::default()
        });
        assert!(h.log.any("dry-fire"), "the pod fired with nothing locked");

        // Acquired through the real path: a hand-set lock would not prove the
        // targeting system would have offered one.
        let list = vec![h.sim.roster.by_id(id).unwrap().as_damageable()];
        let acquired = h
            .sim
            .targeting
            .acquire(&h.sim.view, &list, &h.sim.collision, &mut h.log);
        assert!(acquired, "the mech ahead could not be locked");
        h.log.clear();
        h.step(&ControlState {
            missile_pressed: true,
            ..ControlState::default()
        });
        h.run(0.6, &idle());
        assert_eq!(h.log.count("missile-launch"), missile_cfg::COUNT as usize);
    }

    #[test]
    fn a_swing_damages_each_mech_in_the_arc_once() {
        let mut h = Harness::new();
        let near = h.sim.player.pos + Vec3::new(-2.5, 0.0, -6.0);
        let far = h.sim.player.pos + Vec3::new(2.5, 0.0, -6.0);
        let a = h.place(EnemyKind::Skirmisher, near);
        let b = h.place(EnemyKind::Skirmisher, far);

        // Facing -Z: the blade arc is centred on the body.
        h.sim.player.yaw = std::f32::consts::PI;
        h.sim.player.body_yaw = std::f32::consts::PI;
        h.sim.view.yaw = std::f32::consts::PI;

        h.step(&ControlState {
            blade_pressed: true,
            ..ControlState::default()
        });
        h.run(blade_cfg::WINDUP + blade_cfg::ACTIVE + 0.02, &idle());

        let hit_a = h.sim.roster.by_id(a).unwrap().health;
        let hit_b = h.sim.roster.by_id(b).unwrap().health;
        assert!(hit_a < h.sim.roster.by_id(a).unwrap().max_health);
        assert!(hit_b < h.sim.roster.by_id(b).unwrap().max_health);

        // Through the rest of the active window: no second helping.
        h.run(blade_cfg::ACTIVE, &idle());
        assert_eq!(h.sim.roster.by_id(a).unwrap().health, hit_a);
        assert_eq!(h.sim.roster.by_id(b).unwrap().health, hit_b);
    }

    #[test]
    fn a_swing_misses_what_is_behind_or_out_of_reach() {
        let mut h = Harness::new();
        let far = h.sim.player.pos + Vec3::new(0.0, 0.0, -40.0);
        let behind = h.sim.player.pos + Vec3::new(0.0, 0.0, 6.0);
        let a = h.place(EnemyKind::Skirmisher, far);
        let b = h.place(EnemyKind::Skirmisher, behind);

        h.sim.player.yaw = std::f32::consts::PI;
        h.sim.player.body_yaw = std::f32::consts::PI;
        h.sim.view.yaw = std::f32::consts::PI;

        h.step(&ControlState {
            blade_pressed: true,
            ..ControlState::default()
        });
        h.run(blade_cfg::WINDUP + blade_cfg::ACTIVE + 0.02, &idle());

        let full_a = h.sim.roster.by_id(a).unwrap().max_health;
        let full_b = h.sim.roster.by_id(b).unwrap().max_health;
        assert_eq!(
            h.sim.roster.by_id(a).unwrap().health,
            full_a,
            "out of reach"
        );
        assert_eq!(h.sim.roster.by_id(b).unwrap().health, full_b, "behind");
    }

    #[test]
    fn the_aim_point_stops_on_the_mech_it_is_pointing_at() {
        let mut h = Harness::new();
        let at = h.sim.player.pos + Vec3::new(0.0, 0.0, -30.0);
        let id = h.place(EnemyKind::Skirmisher, at);

        h.sim.view.yaw = std::f32::consts::PI;
        h.sim.view.pitch = 0.0;
        h.sim.view.at = h.sim.player.pos + Vec3::new(0.0, 4.0, 0.0);
        h.step(&idle());

        let enemy = h.sim.roster.by_id(id).unwrap();
        let distance = (h.sim.aim_point - h.sim.view.at).length();
        let to_enemy = (enemy.pos - h.sim.view.at).length();
        assert!(
            distance < to_enemy,
            "the reticle went past the mech: {distance} vs {to_enemy}"
        );
        assert_eq!(h.sim.player.aim_point, h.sim.aim_point);
    }

    #[test]
    fn a_mech_below_the_world_is_recovered_within_a_step() {
        let mut h = Harness::new();
        h.sim.player.pos = Vec3::new(0.0, world_cfg::KILL_PLANE - 10.0, 0.0);
        h.step(&idle());

        assert!(
            h.sim.player.pos.y > world_cfg::KILL_PLANE,
            "still below the kill plane: {:?}",
            h.sim.player.pos
        );
        assert!(h.sim.player.pos.is_finite());
    }

    #[test]
    fn the_out_of_bounds_net_catches_a_mech_the_ground_probe_cannot() {
        // A living mech is put back by its own ground probe, which falls back
        // to the world floor; this check only ever fires for one that is not
        // being stepped, which in practice means a wreck. It is kept because
        // that fallback is a property of the collision model rather than a
        // promise, and a model that stopped offering a floor would otherwise
        // drop a mech into nothing with no net under it.
        let mut h = Harness::new();
        h.sim.player.alive = false;
        h.sim.player.pos = Vec3::new(0.0, world_cfg::KILL_PLANE - 10.0, 0.0);
        h.step(&idle());

        assert!(h.log.any("out-of-bounds"));
        assert!(h.sim.player.pos.y > world_cfg::KILL_PLANE);
        assert_eq!(h.sim.player.vel, Vec3::ZERO);
    }

    #[test]
    fn the_same_inputs_produce_the_same_world() {
        // Determinism is what makes a replay worth recording, and the original
        // had one source of it removed on purpose: the fire spread came from
        // `Math.random`. It comes from a seeded generator here.
        let play = |h: &mut Harness| {
            h.run(3.4, &idle());
            h.sim.view.yaw = std::f32::consts::PI;
            h.run(2.0, &holding());
            h.run(1.0, &idle());
        };
        let mut a = Harness::new();
        let mut b = Harness::new();
        play(&mut a);
        play(&mut b);

        assert_eq!(a.sim.player.pos, b.sim.player.pos);
        assert_eq!(a.sim.roster.list.len(), b.sim.roster.list.len());
        for (x, y) in a.sim.roster.list.iter().zip(&b.sim.roster.list) {
            assert_eq!(x.pos, y.pos);
            assert_eq!(x.health, y.health);
        }
        assert_eq!(a.sim.mission.stats.score, b.sim.mission.stats.score);
        assert_eq!(
            a.sim.mission.stats.shots_fired,
            b.sim.mission.stats.shots_fired
        );
    }

    #[test]
    fn a_reset_returns_every_subsystem_to_its_starting_state() {
        let mut h = Harness::new();
        h.run(3.4, &idle());
        h.run(1.0, &holding());
        assert!(h.sim.roster.active_count() > 0);

        h.sim.reset(MechPresetId::Light);

        assert_eq!(h.sim.roster.active_count(), 0);
        assert_eq!(h.sim.projectiles.active_count(), 0);
        assert_eq!(h.sim.targeting.locked_id(), None);
        assert_eq!(h.sim.mission.phase, crate::types::MissionPhase::Intro);
        assert_eq!(h.sim.mission.stats.kills, 0);
        assert_eq!(h.sim.mission.stats.damage_taken, 0.0);
        assert_eq!(h.sim.mission.stats.elapsed, 0.0);
        assert_eq!(h.sim.time, 0.0);
        assert!(h.sim.player.alive);
        assert_eq!(h.sim.player.health, MECH_PRESETS[0].health);
        assert_eq!(h.sim.player.ammo, rifle::MAGAZINE);
        assert_eq!(h.sim.player.repairs, player_cfg::REPAIRS);
        assert_eq!(h.sim.player.pos, h.sim.arena.spawns.player);
    }

    #[test]
    fn repeated_restarts_do_not_accumulate_enemies() {
        let mut h = Harness::new();
        for _ in 0..6 {
            h.run(3.4, &idle());
            h.sim.reset(MechPresetId::Balanced);
        }
        h.run(0.2, &idle());
        assert!(
            h.sim.roster.active_count() <= 3,
            "the yard filled up across restarts: {}",
            h.sim.roster.active_count()
        );
        assert!(h.sim.roster.list.len() <= 4);
    }

    #[test]
    fn the_mech_neither_falls_through_the_floor_nor_leaves_the_arena() {
        let mut h = Harness::new();
        h.run(
            2.0,
            &ControlState {
                move_x: 1.0,
                move_mag: 1.0,
                ..ControlState::default()
            },
        );
        assert!(h.sim.player.pos.y >= -1.0);
        assert!(h.sim.player.pos.x.abs() < world_cfg::HALF + 10.0);

        h.run(
            6.0,
            &ControlState {
                move_x: -1.0,
                move_mag: 1.0,
                assault_held: true,
                ..ControlState::default()
            },
        );
        assert!(h.sim.player.pos.y >= -1.0);
        assert!(h.sim.player.pos.x.abs() < world_cfg::HALF + 10.0);
    }

    #[test]
    fn a_quick_boost_is_bounded_by_the_speed_it_was_given() {
        let mut h = Harness::new();
        h.step(&ControlState {
            quick_boost_pressed: true,
            move_z: 1.0,
            move_mag: 1.0,
            ..ControlState::default()
        });
        let peak = h.sim.player.speed();
        assert!(peak > player_cfg::GLIDE_SPEED, "the boost did nothing");
        assert!(peak <= player_cfg::QUICK_BOOST_SPEED + 1.0);
    }

    #[test]
    fn a_step_is_a_step_however_the_caller_groups_them() {
        // The fixed timestep has to mean a fixed timestep: energy spent over a
        // simulated second must not depend on how many calls it arrived in.
        let mut one = Harness::new();
        let mut many = Harness::new();
        let c = ControlState {
            assault_held: true,
            ..ControlState::default()
        };
        one.run(1.0, &c);
        for _ in 0..120 {
            many.step(&c);
        }
        assert!((one.sim.player.energy - many.sim.player.energy).abs() < 1e-4);
        assert_eq!(one.sim.player.pos, many.sim.player.pos);
    }

    #[test]
    fn an_enemy_shot_that_misses_leaves_the_player_alone() {
        // The negative half of the damage path: an ordnance phase that damaged
        // everything in the list regardless of where it was would pass the
        // positive test above.
        let mut h = Harness::new();
        let before = h.sim.player.health;
        let mut shot = crate::projectiles::EnemyShotSpec::new(
            WeaponId::EnemyGun,
            999,
            h.sim.player.pos + Vec3::new(0.0, 3.0, -12.0),
            Vec3::new(0.0, 0.0, -1.0),
            200.0,
            1.0,
        );
        shot.damage = 40.0;
        h.sim.projectiles.spawn_enemy(shot);
        h.run(0.3, &idle());
        assert_eq!(h.sim.player.health, before);
        assert_eq!(h.sim.mission.stats.damage_taken, 0.0);
    }
}
