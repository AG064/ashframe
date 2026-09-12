//! Enemies.
//!
//! Ported from the original's `game/enemies.ts`: *"Three archetypes share one
//! base: a mobile skirmisher, a slow artillery unit that pressures you with
//! telegraphed fire, and the boss. All of them respect line of sight, all of
//! them can be staggered, and none of them shoot through cover. Movement uses
//! steering with a stuck detector rather than a navmesh, so a unit that walks
//! into a wall re-routes instead of grinding against it."*
//!
//! ## The steering, and what it costs
//!
//! The original named this as a limitation, and it is worth repeating where
//! somebody changing it will see it: a unit steers toward a goal, and when the
//! straight line to that goal is blocked it slides the goal sideways and tries
//! again. A stuck detector catches the case where that still fails — no
//! progress while trying to move — and re-picks a fresh orbit.
//!
//! That is not pathfinding. A unit can take a long way round a large structure,
//! and against a concave arrangement it can oscillate. It is thirty lines
//! instead of a navigation mesh, it never fails badly enough to break a fight,
//! and the honest upgrade is Godot's navmesh rather than more of this.
//!
//! ## On the context type
//!
//! The original passed an `EnemyContext` object holding the world, the
//! projectile system, the event sink, the RNG and the damageable list, all by
//! reference. Rust cannot hold four mutable borrows in one long-lived struct
//! and also hand out `&mut` to the enemies being stepped.
//!
//! [`EnemyWorld`] exists anyway, because it is the same idea expressed in a way
//! the borrow checker can prove: it bundles references to things that are
//! genuinely *different* objects, built once per step and passed down. What it
//! cannot hold is a reference to the enemies themselves, which is why the
//! roster owns those and steps them one at a time.

use crate::collision::CollisionWorld;
use crate::combat::CombatFields;
use crate::config::{
    artillery as artillery_cfg, boss as boss_cfg, skirmisher as skirmisher_cfg, world as world_cfg,
};
use crate::math::{clamp, Rng};
use crate::projectiles::ProjectileSystem;
use crate::types::{Damageable, EnemyKind, Faction, Hooks, SimEvent, Vec3, WeaponId};

/// What a unit is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiState {
    Idle,
    Engage,
    Reposition,
    Telegraph,
    Attack,
    Staggered,
    Dead,
}

impl AiState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Engage => "engage",
            Self::Reposition => "reposition",
            Self::Telegraph => "telegraph",
            Self::Attack => "attack",
            Self::Staggered => "staggered",
            Self::Dead => "dead",
        }
    }
}

/// Everything a unit needs that is not itself.
///
/// Built once per step by the world. Every field is a different object, which
/// is what lets Rust accept four mutable borrows side by side.
pub struct EnemyWorld<'a> {
    pub world: &'a CollisionWorld,
    pub projectiles: &'a mut ProjectileSystem,
    pub hooks: &'a mut dyn Hooks,
    /// Where the player is, or `None` once they are destroyed.
    pub target: Option<Vec3>,
    pub target_alive: bool,
    /// Everything an area attack can hit. The boss slam uses this, and in
    /// practice it is the player.
    pub damageables: &'a mut [Damageable],
    /// Shared across the roster, so a replay reproduces the same scatter.
    pub rng: &'a mut Rng,
    /// Simulation time, for staggering volleys.
    pub time: f32,
}

/// Shared behaviour for every hostile mech.
#[derive(Debug, Clone)]
pub struct Enemy {
    pub id: u32,
    pub kind: EnemyKind,
    pub name: &'static str,
    pub pos: Vec3,
    pub vel: Vec3,
    pub yaw: f32,

    pub health: f32,
    pub max_health: f32,
    pub stability: f32,
    pub max_stability: f32,
    pub alive: bool,
    pub stagger_timer: f32,
    pub invuln_timer: f32,
    pub stagger_armed: bool,
    /// Brief immunity after a stagger, so nothing can be locked down forever.
    pub stagger_immune: f32,

    pub radius: f32,
    pub height: f32,

    pub state: AiState,
    pub timer: f32,
    pub cooldown: f32,
    /// Rounds left in the current burst or salvo.
    pub burst_left: u32,
    pub burst_timer: f32,

    pub goal: Vec3,
    pub orbit_dir: f32,
    pub orbit_timer: f32,
    pub stuck_timer: f32,
    pub last_pos: Vec3,

    /// Turret aim offset from the body, for the renderer.
    pub torso_yaw: f32,
    /// Locomotion phase, driving leg animation.
    pub gait: f32,
    pub dashing: bool,
    pub dash_dir: Vec3,
    /// Whether the unit currently sees the player.
    pub has_los: bool,
    /// Damage flash timer, for the renderer.
    pub hit_flash: f32,
}

impl Enemy {
    pub fn new(kind: EnemyKind) -> Self {
        let (name, health, stability, radius, height) = match kind {
            EnemyKind::Skirmisher => (
                skirmisher_cfg::NAME,
                skirmisher_cfg::HEALTH,
                skirmisher_cfg::STABILITY,
                skirmisher_cfg::RADIUS,
                skirmisher_cfg::HEIGHT,
            ),
            EnemyKind::Artillery => (
                artillery_cfg::NAME,
                artillery_cfg::HEALTH,
                artillery_cfg::STABILITY,
                artillery_cfg::RADIUS,
                artillery_cfg::HEIGHT,
            ),
            EnemyKind::Boss => (
                boss_cfg::NAME,
                boss_cfg::HEALTH,
                boss_cfg::STABILITY,
                boss_cfg::RADIUS,
                boss_cfg::HEIGHT,
            ),
        };

        Self {
            id: 0,
            kind,
            name,
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            // Facing the player's side of the arena, who starts to the south.
            yaw: std::f32::consts::PI,
            health,
            max_health: health,
            stability: 0.0,
            max_stability: stability,
            alive: true,
            stagger_timer: 0.0,
            invuln_timer: 0.0,
            stagger_armed: false,
            stagger_immune: 0.0,
            radius,
            height,
            state: AiState::Idle,
            timer: 0.0,
            cooldown: 0.0,
            burst_left: 0,
            burst_timer: 0.0,
            goal: Vec3::ZERO,
            orbit_dir: 1.0,
            orbit_timer: 0.0,
            stuck_timer: 0.0,
            last_pos: Vec3::ZERO,
            torso_yaw: 0.0,
            gait: 0.0,
            dashing: false,
            dash_dir: Vec3::ZERO,
            has_los: false,
            hit_flash: 0.0,
        }
    }

    /// Place or replace the unit at a spawn point.
    pub fn spawn(&mut self, at: Vec3, yaw: f32) {
        self.pos = at;
        self.vel = Vec3::ZERO;
        self.last_pos = at;
        self.yaw = yaw;
        self.torso_yaw = 0.0;
        self.state = AiState::Engage;
        self.timer = 0.0;
        // A short initial delay, so a wave does not fire the instant it
        // appears.
        self.cooldown = 0.6;
        self.burst_left = 0;
        self.burst_timer = 0.0;
        self.stuck_timer = 0.0;
        self.stagger_timer = 0.0;
        self.stagger_immune = 0.0;
        self.dashing = false;
        self.has_los = false;
    }

    /// Full restore for a mission restart.
    pub fn revive(&mut self, at: Vec3) {
        self.health = self.max_health;
        self.stability = 0.0;
        self.alive = true;
        self.spawn(at, std::f32::consts::PI);
    }

    pub fn staggered(&self) -> bool {
        self.stagger_timer > 0.0
    }

    pub fn centre_y(&self) -> f32 {
        self.pos.y + self.height * 0.5
    }

    pub fn health_fraction(&self) -> f32 {
        if self.max_health <= 0.0 {
            return 0.0;
        }
        self.health / self.max_health
    }

    pub fn as_damageable(&self) -> Damageable {
        Damageable {
            id: self.id,
            faction: Faction::Enemy,
            name: self.kind.label(),
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

    /// The slice of this unit that combat owns, for folding hits back in.
    pub fn combat_fields(&mut self) -> CombatFields<'_> {
        CombatFields {
            health: &mut self.health,
            stability: &mut self.stability,
            alive: &mut self.alive,
            stagger_timer: &mut self.stagger_timer,
            invuln_timer: &mut self.invuln_timer,
            stagger_armed: &mut self.stagger_armed,
        }
    }

    /// Advance the unit one fixed step.
    pub fn step(&mut self, dt: f32, ctx: &mut EnemyWorld<'_>) {
        if !self.alive {
            return;
        }
        if self.hit_flash > 0.0 {
            self.hit_flash -= dt;
        }
        if self.stagger_immune > 0.0 {
            self.stagger_immune -= dt;
        }

        let staggered = self.tick_common(dt, ctx);
        if !self.alive {
            return;
        }

        if staggered {
            // A staggered unit keeps its momentum but bleeds it away, so it
            // slides to a halt rather than stopping dead. That reads as being
            // knocked off balance rather than as a pause.
            self.vel.x *= 0.86;
            self.vel.z *= 0.86;
        } else {
            match self.kind {
                EnemyKind::Skirmisher => self.step_skirmisher(dt, ctx),
                EnemyKind::Artillery => self.step_artillery(dt, ctx),
                EnemyKind::Boss => self.step_boss(dt, ctx),
            }
        }

        self.integrate(dt, ctx);
        self.animate_gait(dt);
    }

    /// Returns whether the unit is staggered and cannot act.
    fn tick_common(&mut self, dt: f32, ctx: &mut EnemyWorld<'_>) -> bool {
        if self.stagger_timer > 0.0 {
            self.stagger_timer -= dt;
            if self.stagger_timer <= 0.0 {
                self.stagger_timer = 0.0;
                self.stagger_armed = false;
                // The immunity is what stops a fast weapon from chain-stunning
                // a unit forever. The boss gets longer, because a boss that can
                // be held down is not a boss.
                self.stagger_immune = if self.kind == EnemyKind::Boss {
                    5.5
                } else {
                    2.2
                };
                ctx.hooks.emit(&SimEvent::Unstagger { target: self.id });
            }
            self.state = AiState::Staggered;
            return true;
        }

        // Stability recovers faster while immune, so the immunity window is
        // also the unit shaking off the hit rather than merely being unable to
        // be hit again.
        if self.stability > 0.0 {
            let rate = if self.stagger_immune <= 0.0 {
                0.32
            } else {
                0.6
            };
            self.stability = (self.stability - self.max_stability * rate * dt).max(0.0);
        }
        if self.cooldown > 0.0 {
            self.cooldown -= dt;
        }

        // Line of sight from the sensor head to the player's centre.
        self.has_los = match (ctx.target, ctx.target_alive) {
            (Some(target), true) => {
                let eye = Vec3::new(self.pos.x, self.pos.y + self.height * 0.8, self.pos.z);
                let aim = Vec3::new(target.x, target.y + 3.0, target.z);
                ctx.world.line_of_sight(eye, aim)
            }
            _ => false,
        };

        false
    }

    /// Distance to the player on the horizontal plane.
    fn range_to(&self, ctx: &EnemyWorld<'_>) -> f32 {
        match ctx.target {
            None => f32::INFINITY,
            Some(target) => {
                let offset = target - self.pos;
                offset.horizontal_length()
            }
        }
    }

    /// Face the player, with the torso leading the body.
    fn aim_at_target(&mut self, dt: f32, ctx: &EnemyWorld<'_>, rate: f32) {
        let Some(target) = ctx.target else {
            return;
        };
        let want = (target.x - self.pos.x).atan2(target.z - self.pos.z);
        let mut delta = want - self.yaw;
        while delta > std::f32::consts::PI {
            delta -= std::f32::consts::PI * 2.0;
        }
        while delta < -std::f32::consts::PI {
            delta += std::f32::consts::PI * 2.0;
        }
        let applied = clamp(delta, -rate * dt, rate * dt);
        self.yaw += applied;
        // The torso leads: it takes up the part of the turn the body could not,
        // capped so it never twists impossibly far.
        self.torso_yaw = clamp(delta - applied, -1.1, 1.1);
    }

    /// Steer toward the current goal, routing around cover instead of into it.
    fn steer(&mut self, dt: f32, ctx: &EnemyWorld<'_>, speed: f32, accel: f32) {
        let mut delta = Vec3::new(self.goal.x - self.pos.x, 0.0, self.goal.z - self.pos.z);
        let mut distance = delta.horizontal_length();

        // A blocked straight line means the unit would grind along the wall.
        // Sliding the goal sideways keeps mobile enemies moving around cover.
        // This is the whole of the "navigation", and it is why the original
        // called the steering a limitation rather than a solution.
        if distance > 3.5 {
            let eye = self.pos.y + self.height * 0.5;
            let clear = ctx.world.line_of_sight(
                Vec3::new(self.pos.x, eye, self.pos.z),
                Vec3::new(self.goal.x, eye, self.goal.z),
            );
            if !clear {
                let a = delta.x.atan2(delta.z) + self.orbit_dir * 0.95;
                self.goal.x = self.pos.x + a.sin() * distance;
                self.goal.z = self.pos.z + a.cos() * distance;
                self.clamp_goal();
                delta = Vec3::new(self.goal.x - self.pos.x, 0.0, self.goal.z - self.pos.z);
                distance = delta.horizontal_length();
            }
        }

        let want = if distance > 1.5 {
            Vec3::new(
                (delta.x / distance) * speed,
                0.0,
                (delta.z / distance) * speed,
            )
        } else {
            Vec3::ZERO
        };

        let change = Vec3::new(want.x - self.vel.x, 0.0, want.z - self.vel.z);
        let change_len = change.horizontal_length();
        let max_delta = accel * dt;
        if change_len <= max_delta || change_len < 1e-5 {
            self.vel.x = want.x;
            self.vel.z = want.z;
        } else {
            self.vel.x += (change.x / change_len) * max_delta;
            self.vel.z += (change.z / change_len) * max_delta;
        }
    }

    /// Pick a new orbit direction and distance band around the player.
    fn repick_orbit(&mut self, ctx: &mut EnemyWorld<'_>, min_r: f32, max_r: f32) {
        self.orbit_dir = if ctx.rng.next() > 0.5 { 1.0 } else { -1.0 };
        self.orbit_timer = ctx.rng.range(1.6, 3.4);

        let Some(target) = ctx.target else {
            return;
        };
        let r = ctx.rng.range(min_r, max_r);
        let a = (self.pos.x - target.x).atan2(self.pos.z - target.z);
        let na = a + self.orbit_dir * ctx.rng.range(0.5, 1.5);
        self.goal = Vec3::new(target.x + na.sin() * r, self.pos.y, target.z + na.cos() * r);
        self.clamp_goal();
    }

    /// Keep the goal inside the arena, with a margin so a unit never walks into
    /// the blast wall while chasing one.
    fn clamp_goal(&mut self) {
        let limit = world_cfg::HALF - 12.0;
        self.goal.x = clamp(self.goal.x, -limit, limit);
        self.goal.z = clamp(self.goal.z, -limit, limit);
    }

    fn integrate(&mut self, dt: f32, ctx: &mut EnemyWorld<'_>) {
        let delta = Vec3::new(self.vel.x * dt, 0.0, self.vel.z * dt);
        let distance = delta.horizontal_length();
        // Substepped for the same reason the player is: a fast unit against
        // thin cover would otherwise be resolved out of one box and into
        // another.
        let steps = ((distance / 0.6).ceil() as u32).max(1);
        let share = 1.0 / steps as f32;

        for _ in 0..steps {
            self.pos.x += delta.x * share;
            self.pos.z += delta.z * share;
            let resolved = ctx.world.resolve_cylinder(
                &mut self.pos,
                self.radius,
                self.height,
                world_cfg::ENEMY_STEP_HEIGHT,
                self.vel.y,
            );
            let normal = resolved.normal_xz.horizontal_normalized();
            if resolved.hit && normal.horizontal_length() > 1e-5 {
                let into = self.vel.x * normal.x + self.vel.z * normal.z;
                if into < 0.0 {
                    self.vel.x -= into * normal.x;
                    self.vel.z -= into * normal.z;
                }
            }
        }

        // Gravity and a ground snap: mobile units never float.
        //
        // The probe starts one step above the feet rather than above the head,
        // for the same reason the player's does: a cast that begins above the
        // unit sees whatever the unit is standing under, and assigning that to
        // the feet lifts it onto the roof of a structure it was walking
        // beneath. One step is still more than a unit falls in a frame, so a
        // surface it has just sunk past is still found.
        self.vel.y += world_cfg::GRAVITY * dt;
        self.pos.y += self.vel.y * dt;
        let ground = ctx.world.ground_at(
            self.pos.x,
            self.pos.z,
            self.pos.y + world_cfg::ENEMY_STEP_HEIGHT,
            220.0,
        );
        if self.pos.y <= ground {
            self.pos.y = ground;
            self.vel.y = 0.0;
        }

        let limit = world_cfg::HALF - self.radius - 2.0;
        self.pos.x = clamp(self.pos.x, -limit, limit);
        self.pos.z = clamp(self.pos.z, -limit, limit);

        // Stuck detection: wanting to move while making no progress means the
        // steering has failed, so the orbit is re-picked from scratch.
        let moved = (self.pos - self.last_pos).horizontal_length();
        let wants = self.vel.horizontal_length() > 1.5;
        if wants && moved < 0.02 {
            self.stuck_timer += dt;
            if self.stuck_timer > 0.7 {
                self.stuck_timer = 0.0;
                self.repick_orbit(ctx, 26.0, 60.0);
                self.orbit_dir = -self.orbit_dir;
                self.vel = Vec3::ZERO;
            }
        } else {
            self.stuck_timer = 0.0;
        }
        self.last_pos = self.pos;
    }

    fn animate_gait(&mut self, dt: f32) {
        // Faster legs with faster movement, so the walk and the slide agree.
        self.gait += dt * (1.6 + self.vel.horizontal_length() * 0.42);
    }

    /// Fire one aimed shot.
    ///
    /// The aiming error is deliberate rather than predictive: the original
    /// computed a lead term and multiplied it by zero, so units aim where the
    /// player *is*. Kept as-is, because leading a moving target makes enemies
    /// feel like they are reading the player's inputs.
    #[allow(clippy::too_many_arguments)]
    fn shoot(
        &mut self,
        ctx: &mut EnemyWorld<'_>,
        spread: f32,
        speed: f32,
        damage: f32,
        impact: f32,
        life: f32,
        gravity: f32,
        splash_radius: f32,
        splash_damage: f32,
    ) {
        let Some(target) = ctx.target else {
            return;
        };

        let muzzle = Vec3::new(
            self.pos.x + self.yaw.sin() * self.radius * 0.6,
            self.pos.y + self.height * 0.72,
            self.pos.z + self.yaw.cos() * self.radius * 0.6,
        );
        let aim = Vec3::new(target.x, target.y + 3.0, target.z);
        let direction = (aim - muzzle).normalized();

        let weapon = match self.kind {
            EnemyKind::Boss => WeaponId::BossBarrage,
            EnemyKind::Artillery => WeaponId::EnemyMortar,
            EnemyKind::Skirmisher => WeaponId::EnemyGun,
        };

        let scatter = Vec3::new(
            (ctx.rng.next() - 0.5) * spread,
            (ctx.rng.next() - 0.5) * spread,
            (ctx.rng.next() - 0.5) * spread,
        );

        let mut spec = crate::projectiles::EnemyShotSpec::new(
            weapon,
            self.id,
            muzzle,
            direction + scatter,
            speed,
            life,
        );
        spec.damage = damage;
        spec.impact = impact;
        spec.gravity = gravity;
        // A fixed radius for every enemy round, which is the original's choice.
        spec.radius = 0.5;
        spec.splash_radius = splash_radius;
        spec.splash_damage = splash_damage;

        ctx.projectiles.spawn_enemy(spec);
        ctx.hooks.emit(&SimEvent::EnemyFire { at: muzzle });
    }

    // -- archetypes -------------------------------------------------------

    fn step_skirmisher(&mut self, dt: f32, ctx: &mut EnemyWorld<'_>) {
        self.aim_at_target(dt, ctx, 4.2);
        self.orbit_timer -= dt;
        if self.orbit_timer <= 0.0 {
            self.repick_orbit(
                ctx,
                skirmisher_cfg::PREFERRED_RANGE - 10.0,
                skirmisher_cfg::PREFERRED_RANGE + 14.0,
            );
        }

        let range = self.range_to(ctx);
        // Close the distance if the player is running away, otherwise orbit.
        // Without this a skirmisher would circle at its preferred range while
        // the player walked to the other side of the map.
        if range > skirmisher_cfg::PREFERRED_RANGE + 16.0 {
            if let Some(target) = ctx.target {
                self.goal = Vec3::new(target.x, self.pos.y, target.z);
                self.clamp_goal();
            }
        }
        self.steer(dt, ctx, skirmisher_cfg::SPEED, skirmisher_cfg::ACCEL);

        // A burst starts only with a clear shot and an expired cooldown, but
        // once started it finishes: a burst interrupted halfway by a moment of
        // cover would read as the enemy losing its nerve rather than as the
        // player breaking line of sight.
        if self.burst_left == 0
            && self.cooldown <= 0.0
            && self.has_los
            && range < skirmisher_cfg::SIGHT
        {
            self.burst_left = skirmisher_cfg::BURST_COUNT;
            self.burst_timer = 0.0;
        }
        if self.burst_left > 0 {
            self.burst_timer -= dt;
            if self.burst_timer <= 0.0 {
                self.burst_timer = skirmisher_cfg::BURST_INTERVAL;
                self.burst_left -= 1;
                self.shoot(
                    ctx,
                    skirmisher_cfg::SPREAD,
                    skirmisher_cfg::PROJECTILE_SPEED,
                    skirmisher_cfg::DAMAGE,
                    7.0,
                    skirmisher_cfg::PROJECTILE_LIFE,
                    0.0,
                    0.0,
                    0.0,
                );
                if self.burst_left == 0 {
                    // A little jitter on the cooldown, so a group does not fire
                    // in lockstep and become a single burst rather than three.
                    let jitter = ctx.rng.range(0.0, 0.8);
                    self.cooldown = skirmisher_cfg::BURST_COOLDOWN + jitter;
                }
            }
        }
    }

    fn step_artillery(&mut self, dt: f32, ctx: &mut EnemyWorld<'_>) {
        self.aim_at_target(dt, ctx, 1.8);

        if self.state == AiState::Telegraph {
            self.timer -= dt;
            // Immobile while winding up, which is what gives the player a
            // window to break line of sight.
            self.steer(dt, ctx, 0.0, artillery_cfg::ACCEL);
            if self.timer <= 0.0 {
                self.state = AiState::Attack;
                self.burst_left = artillery_cfg::SALVO_COUNT;
                self.burst_timer = 0.0;
            }
            return;
        }

        if self.state == AiState::Attack {
            self.burst_timer -= dt;
            self.steer(dt, ctx, 0.0, artillery_cfg::ACCEL);
            if self.burst_timer <= 0.0 && self.burst_left > 0 {
                self.burst_timer = artillery_cfg::SALVO_INTERVAL;
                self.burst_left -= 1;
                // Mortars arc onto the player's position; the telegraph marker
                // is what makes them avoidable rather than unfair.
                self.shoot(
                    ctx,
                    0.05,
                    artillery_cfg::PROJECTILE_SPEED,
                    artillery_cfg::DAMAGE,
                    12.0,
                    6.0,
                    -18.0,
                    artillery_cfg::SPLASH_RADIUS,
                    artillery_cfg::DAMAGE * 0.7,
                );
                if self.burst_left == 0 {
                    self.state = AiState::Engage;
                    let jitter = ctx.rng.range(0.0, 1.2);
                    self.cooldown = artillery_cfg::COOLDOWN + jitter;
                }
            }
            return;
        }

        let range = self.range_to(ctx);
        if range < artillery_cfg::PREFERRED_RANGE * 0.62 {
            // The player has closed in. Relocate rather than trading shots at a
            // range this unit is not built for.
            self.repick_orbit(
                ctx,
                artillery_cfg::PREFERRED_RANGE,
                artillery_cfg::PREFERRED_RANGE + 24.0,
            );
            self.steer(dt, ctx, artillery_cfg::SPEED, artillery_cfg::ACCEL);
        } else {
            self.orbit_timer -= dt;
            if self.orbit_timer <= 0.0 {
                self.orbit_timer = ctx.rng.range(3.5, 6.5);
                let dx = ctx.rng.signed() * 12.0;
                let dz = ctx.rng.signed() * 12.0;
                self.goal = Vec3::new(self.pos.x + dx, self.pos.y, self.pos.z + dz);
                self.clamp_goal();
            }
            self.steer(dt, ctx, artillery_cfg::SPEED * 0.6, artillery_cfg::ACCEL);
        }

        if self.cooldown <= 0.0 && self.has_los && range < artillery_cfg::SIGHT {
            self.state = AiState::Telegraph;
            self.timer = artillery_cfg::TELEGRAPH;
            let target = ctx.target.unwrap_or(self.pos);
            ctx.hooks.emit(&SimEvent::Telegraph {
                at: Vec3::new(target.x, 0.0, target.z),
                radius: artillery_cfg::SPLASH_RADIUS,
                duration: self.timer,
            });
        }
    }

    fn step_boss(&mut self, dt: f32, ctx: &mut EnemyWorld<'_>) {
        let fraction = self.health_fraction();
        // Aggression rises at two thresholds, and it is a multiplier on timing
        // rather than a separate behaviour: the same attacks arrive sooner.
        let aggressive = if fraction < boss_cfg::PHASE_3_AT {
            1.55
        } else if fraction < boss_cfg::PHASE_2_AT {
            1.22
        } else {
            1.0
        };

        self.aim_at_target(dt, ctx, if self.dashing { 1.2 } else { 3.2 });

        if self.dashing {
            self.timer -= dt;
            self.vel.x = self.dash_dir.x * boss_cfg::DASH_SPEED;
            self.vel.z = self.dash_dir.z * boss_cfg::DASH_SPEED;
            if self.timer <= 0.0 {
                self.dashing = false;
                self.state = AiState::Engage;
                self.cooldown = 0.35;
            }
            return;
        }

        if self.state == AiState::Telegraph {
            self.timer -= dt;
            self.steer(dt, ctx, 0.0, boss_cfg::ACCEL * 0.4);
            if self.timer <= 0.0 {
                let range = self.range_to(ctx);
                if range <= boss_cfg::SLAM_RANGE {
                    // A ground slam centred on the boss. Only landed if the
                    // player was still close when the windup finished, which
                    // is what makes backing off the correct answer.
                    ctx.hooks.emit(&SimEvent::Telegraph {
                        at: self.pos,
                        radius: boss_cfg::SLAM_RADIUS,
                        duration: 0.12,
                    });
                    let at = Vec3::new(self.pos.x, self.pos.y + 1.0, self.pos.z);
                    crate::projectiles::explode(
                        at,
                        boss_cfg::SLAM_RADIUS,
                        boss_cfg::SLAM_DAMAGE,
                        Faction::Enemy,
                        self.id,
                        ctx.damageables,
                        ctx.hooks,
                    );
                }
                self.state = AiState::Attack;
                // Below the second threshold the barrage is half again as long.
                let rounds = boss_cfg::BARRAGE_COUNT as f32
                    * if fraction < boss_cfg::PHASE_2_AT {
                        1.5
                    } else {
                        1.0
                    };
                self.burst_left = rounds.round() as u32;
                self.burst_timer = 0.0;
            }
            return;
        }

        if self.state == AiState::Attack {
            self.burst_timer -= dt;
            self.steer(dt, ctx, 0.0, boss_cfg::ACCEL * 0.3);
            if self.burst_timer <= 0.0 && self.burst_left > 0 {
                self.burst_timer = boss_cfg::BARRAGE_INTERVAL / aggressive;
                self.burst_left -= 1;
                self.shoot(
                    ctx,
                    boss_cfg::BARRAGE_SPREAD,
                    boss_cfg::BARRAGE_SPEED,
                    boss_cfg::BARRAGE_DAMAGE,
                    10.0,
                    3.2,
                    0.0,
                    0.0,
                    0.0,
                );
                if self.burst_left == 0 {
                    let jitter = ctx.rng.range(0.0, 1.0);
                    self.state = AiState::Engage;
                    self.cooldown = boss_cfg::BARRAGE_COOLDOWN / aggressive + jitter;
                }
            }
            return;
        }

        let range = self.range_to(ctx);
        self.orbit_timer -= dt;
        if self.orbit_timer <= 0.0 {
            self.repick_orbit(ctx, 24.0, 46.0);
        }
        self.steer(
            dt,
            ctx,
            boss_cfg::SPEED * (0.85 + 0.3 * aggressive),
            boss_cfg::ACCEL,
        );

        if self.cooldown <= 0.0 {
            if range <= boss_cfg::SLAM_RANGE {
                self.state = AiState::Telegraph;
                self.timer = boss_cfg::SLAM_WINDUP / aggressive;
                ctx.hooks.emit(&SimEvent::Telegraph {
                    at: self.pos,
                    radius: boss_cfg::SLAM_RADIUS,
                    duration: self.timer,
                });
            } else if ctx.rng.next() < 0.4 {
                // A repositioning dash, across the player rather than at them,
                // so it changes the angle instead of closing the distance.
                self.dashing = true;
                self.timer = boss_cfg::DASH_DURATION;
                self.state = AiState::Attack;
                let spin = if ctx.rng.next() < 0.5 { 1.0 } else { -1.0 };
                let toward = match ctx.target {
                    None => Vec3::new(0.0, 0.0, 1.0),
                    Some(target) => (target - self.pos).horizontal_normalized(),
                };
                self.dash_dir = Vec3::new(-toward.z * spin, 0.0, toward.x * spin).normalized();
                self.vel.x = self.dash_dir.x * boss_cfg::DASH_SPEED;
                self.vel.z = self.dash_dir.z * boss_cfg::DASH_SPEED;
                ctx.hooks.emit(&SimEvent::Telegraph {
                    at: Vec3::new(
                        self.pos.x + self.dash_dir.x * 22.0,
                        self.pos.y,
                        self.pos.z + self.dash_dir.z * 22.0,
                    ),
                    radius: 8.0,
                    duration: boss_cfg::DASH_DURATION,
                });
            } else {
                self.state = AiState::Telegraph;
                self.timer = 0.85 / aggressive;
                let target = ctx.target.unwrap_or(self.pos);
                ctx.hooks.emit(&SimEvent::Telegraph {
                    at: Vec3::new(target.x, 0.0, target.z),
                    radius: boss_cfg::SLAM_RADIUS * 0.6,
                    duration: self.timer,
                });
            }
        }
    }
}

/// The enemies in a mission: creation, stepping, and lookup.
#[derive(Debug, Clone, Default)]
pub struct EnemyRoster {
    pub list: Vec<Enemy>,
    next_id: u32,
}

impl EnemyRoster {
    pub fn new() -> Self {
        Self {
            list: Vec::new(),
            // Ids start past the player's, so nothing can collide with it.
            next_id: 1000,
        }
    }

    /// Get or create an enemy of a kind, reusing dead slots.
    ///
    /// Reuse rather than allocate is the original's choice and it matters more
    /// here than there: the roster is walked every step, and a mission that
    /// allocated a fresh unit per wave would grow the list without bound across
    /// restarts.
    pub fn obtain(&mut self, kind: EnemyKind) -> &mut Enemy {
        if let Some(index) = self.list.iter().position(|e| !e.alive && e.kind == kind) {
            return &mut self.list[index];
        }
        let mut enemy = Enemy::new(kind);
        enemy.id = self.next_id;
        self.next_id += 1;
        self.list.push(enemy);
        self.list.last_mut().expect("just pushed")
    }

    pub fn by_id(&self, id: u32) -> Option<&Enemy> {
        self.list.iter().find(|e| e.id == id)
    }

    pub fn by_id_mut(&mut self, id: u32) -> Option<&mut Enemy> {
        self.list.iter_mut().find(|e| e.id == id)
    }

    pub fn active_count(&self) -> usize {
        self.list.iter().filter(|e| e.alive).count()
    }

    /// Deactivate everything, for a restart.
    pub fn clear(&mut self) {
        for enemy in &mut self.list {
            enemy.alive = false;
        }
    }

    /// Restore every unit to full and put it back at `at`.
    pub fn revive_all(&mut self, at: Vec3) {
        for enemy in &mut self.list {
            enemy.revive(at);
        }
    }

    /// Advance every living unit.
    pub fn step(&mut self, dt: f32, ctx: &mut EnemyWorld<'_>) {
        for enemy in &mut self.list {
            if !enemy.alive {
                continue;
            }
            enemy.step(dt, ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::EventLog;

    fn world() -> CollisionWorld {
        CollisionWorld::new(0.0)
    }

    /// The player's position for a unit to react to.
    fn player_at(at: Vec3) -> Option<Vec3> {
        Some(at)
    }

    /// Run a unit for a while with a stationary player.
    fn run(enemy: &mut Enemy, seconds: f32, target: Vec3, collision: &CollisionWorld) -> EventLog {
        let mut projectiles = ProjectileSystem::new();
        let mut log = EventLog::new();
        let mut rng = Rng::default();
        let mut damageables: Vec<Damageable> = Vec::new();
        let dt = crate::config::sim::DT;

        let steps = (seconds / dt) as u32;
        for _ in 0..steps {
            let mut ctx = EnemyWorld {
                world: collision,
                projectiles: &mut projectiles,
                hooks: &mut log,
                target: player_at(target),
                target_alive: true,
                damageables: &mut damageables,
                rng: &mut rng,
                time: 0.0,
            };
            enemy.step(dt, &mut ctx);
        }
        log
    }

    #[test]
    fn a_unit_under_an_overhang_stays_on_the_floor() {
        // The player's ground probe had this bug and the units' probe had the
        // same one: it cast downward from above the head, saw whatever the unit
        // was standing under, and assigned that to the feet. A skirmisher
        // walking beneath a gantry ended up on the gantry.
        let mut collision = world();
        // Just above a skirmisher's head and below the top of the old probe.
        // A deck higher than the probe start was never found and a deck below
        // the head was never a bug, so the band that matters is narrow and this
        // has to sit inside it.
        collision.add_centred(Vec3::new(0.0, 5.2, 0.0), Vec3::new(40.0, 0.2, 40.0), "deck");

        let mut unit = Enemy::new(EnemyKind::Skirmisher);
        unit.revive(Vec3::new(0.0, 0.0, 0.0));
        run(&mut unit, 1.0, Vec3::new(0.0, 0.0, 20.0), &collision);

        assert!(
            unit.pos.y < 1.0,
            "the unit climbed onto the deck it walked under: y = {}",
            unit.pos.y
        );
    }

    #[test]
    fn every_archetype_gets_its_own_figures() {
        let skirmisher = Enemy::new(EnemyKind::Skirmisher);
        let artillery = Enemy::new(EnemyKind::Artillery);
        let boss = Enemy::new(EnemyKind::Boss);

        assert_eq!(skirmisher.name, skirmisher_cfg::NAME);
        assert_eq!(skirmisher.max_health, skirmisher_cfg::HEALTH);
        assert_eq!(artillery.max_health, artillery_cfg::HEALTH);
        assert_eq!(boss.max_health, boss_cfg::HEALTH);

        assert!(skirmisher.max_health < artillery.max_health);
        assert!(artillery.max_health < boss.max_health);
    }

    #[test]
    fn spawning_puts_a_unit_where_it_was_asked() {
        let mut enemy = Enemy::new(EnemyKind::Skirmisher);
        enemy.spawn(Vec3::new(10.0, 0.0, -20.0), 0.0);
        assert_eq!(enemy.pos, Vec3::new(10.0, 0.0, -20.0));
        assert_eq!(enemy.state, AiState::Engage);
        assert_eq!(enemy.vel, Vec3::ZERO);
    }

    #[test]
    fn reviving_restores_health_and_position() {
        let mut enemy = Enemy::new(EnemyKind::Skirmisher);
        enemy.health = 1.0;
        enemy.alive = false;
        enemy.revive(Vec3::new(5.0, 0.0, 5.0));
        assert_eq!(enemy.health, enemy.max_health);
        assert!(enemy.alive);
        assert_eq!(enemy.pos, Vec3::new(5.0, 0.0, 5.0));
    }

    // -- line of sight ----------------------------------------------------

    #[test]
    fn a_unit_behind_a_wall_loses_sight_and_holds_fire() {
        // The rule that makes cover mean something: nothing shoots through it.
        let mut collision = CollisionWorld::new(0.0);
        // Wide enough that a unit orbiting at its preferred range cannot get
        // around the end of it, which is what makes this a test of line of
        // sight rather than of steering.
        collision.add(crate::collision::Box::from_size(
            0.0, 3.0, 15.0, 300.0, 12.0, 4.0, "wall",
        ));
        let mut enemy = Enemy::new(EnemyKind::Skirmisher);
        enemy.spawn(Vec3::ZERO, 0.0);

        // Checked both before it moves and after it has had time to try.
        let mut projectiles = ProjectileSystem::new();
        let mut log = EventLog::new();
        let mut rng = Rng::default();
        let mut damageables: Vec<Damageable> = Vec::new();
        let mut ctx = EnemyWorld {
            world: &collision,
            projectiles: &mut projectiles,
            hooks: &mut log,
            target: player_at(Vec3::new(0.0, 0.0, 40.0)),
            target_alive: true,
            damageables: &mut damageables,
            rng: &mut rng,
            time: 0.0,
        };
        enemy.step(crate::config::sim::DT, &mut ctx);
        assert!(!enemy.has_los, "the wall should block the sensor");

        let log = run(&mut enemy, 4.0, Vec3::new(0.0, 0.0, 40.0), &collision);
        assert_eq!(log.count("enemy-fire"), 0, "and nothing should be fired");
    }

    #[test]
    fn a_unit_with_a_clear_shot_engages() {
        let collision = world();
        let mut enemy = Enemy::new(EnemyKind::Skirmisher);
        enemy.spawn(Vec3::new(0.0, 0.0, 0.0), 0.0);

        let log = run(&mut enemy, 6.0, Vec3::new(0.0, 0.0, 30.0), &collision);
        assert!(enemy.has_los);
        assert!(
            log.count("enemy-fire") > 0,
            "a skirmisher with a clear shot should fire"
        );
    }

    // -- archetypes -------------------------------------------------------

    #[test]
    fn a_skirmisher_fires_in_bursts_rather_than_a_stream() {
        let collision = world();
        let mut enemy = Enemy::new(EnemyKind::Skirmisher);
        enemy.spawn(Vec3::ZERO, 0.0);

        let log = run(&mut enemy, 6.0, Vec3::new(0.0, 0.0, 30.0), &collision);
        let shots = log.count("enemy-fire");
        // Six seconds of three-round bursts every ~2.3 s is a handful, not a
        // continuous stream.
        assert!(
            (3..=12).contains(&shots),
            "burst fire should be intermittent, got {shots} shots"
        );
    }

    #[test]
    fn artillery_telegraphs_before_it_fires() {
        // The telegraph is what makes a mortar avoidable. Firing without one
        // would be damage from nowhere.
        let collision = world();
        let mut enemy = Enemy::new(EnemyKind::Artillery);
        enemy.spawn(Vec3::ZERO, 0.0);

        let log = run(&mut enemy, 3.0, Vec3::new(0.0, 0.0, 70.0), &collision);
        assert!(log.any("telegraph"), "artillery should warn first");

        // The warn comes before the first shot.
        let telegraph = log
            .events
            .iter()
            .position(|e| e.kind() == "telegraph")
            .expect("a telegraph");
        let first_shot = log
            .events
            .iter()
            .position(|e| e.kind() == "enemy-fire")
            .expect("a shot");
        assert!(
            telegraph < first_shot,
            "the warning must come first, got telegraph at {telegraph} and shot at {first_shot}"
        );
    }

    #[test]
    fn artillery_relocates_when_the_player_closes_in() {
        let collision = world();
        let mut enemy = Enemy::new(EnemyKind::Artillery);
        enemy.spawn(Vec3::ZERO, 0.0);
        // Held off its firing cycle, so what is measured is the relocating and
        // not the time it spends standing still to shoot.
        enemy.cooldown = 100.0;
        let before = enemy.pos;

        // Player right on top of it, well inside the preferred range.
        run(&mut enemy, 3.0, Vec3::new(5.0, 0.0, 5.0), &collision);
        assert!(
            (enemy.pos - before).horizontal_length() > 2.0,
            "artillery should try to make room"
        );
    }

    // -- the boss ---------------------------------------------------------

    #[test]
    fn the_boss_gets_more_aggressive_as_it_is_hurt() {
        // The same attacks, arriving sooner, at two health thresholds.
        let collision = world();

        let rounds_at = |fraction: f32| {
            let mut enemy = Enemy::new(EnemyKind::Boss);
            enemy.spawn(Vec3::new(0.0, 0.0, 0.0), 0.0);
            enemy.health = enemy.max_health * fraction;
            // Past its opening cooldown so it starts acting at once.
            enemy.cooldown = 0.0;
            let log = run(&mut enemy, 8.0, Vec3::new(0.0, 0.0, 45.0), &collision);
            log.count("enemy-fire")
        };

        let healthy = rounds_at(0.9);
        let wounded = rounds_at(0.5);
        let desperate = rounds_at(0.2);

        assert!(
            healthy < wounded,
            "phase two should fire more than phase one: {healthy} vs {wounded}"
        );
        assert!(
            wounded < desperate,
            "phase three should fire more than phase two: {wounded} vs {desperate}"
        );
    }

    #[test]
    fn the_boss_slams_when_the_player_is_close() {
        let collision = world();
        let mut enemy = Enemy::new(EnemyKind::Boss);
        enemy.spawn(Vec3::ZERO, 0.0);
        enemy.cooldown = 0.0;

        let log = run(&mut enemy, 4.0, Vec3::new(6.0, 0.0, 6.0), &collision);
        assert!(log.any("telegraph"), "the boss should warn before a slam");
    }

    #[test]
    fn the_boss_can_be_staggered_but_not_held_down() {
        // The immunity window, and the reason the boss gets a longer one than
        // anything else.
        let collision = world();
        let mut enemy = Enemy::new(EnemyKind::Boss);
        enemy.spawn(Vec3::ZERO, 0.0);

        enemy.stagger_timer = 0.1;
        run(&mut enemy, 0.5, Vec3::new(0.0, 0.0, 45.0), &collision);

        assert!(!enemy.staggered(), "the stagger should have expired");
        assert!(
            enemy.stagger_immune > 4.0,
            "the boss should be immune for seconds afterwards, got {}",
            enemy.stagger_immune
        );
    }

    #[test]
    fn a_staggered_unit_does_not_act() {
        let collision = world();
        let mut enemy = Enemy::new(EnemyKind::Skirmisher);
        enemy.spawn(Vec3::ZERO, 0.0);
        enemy.stagger_timer = 1.0;

        let log = run(&mut enemy, 0.5, Vec3::new(0.0, 0.0, 30.0), &collision);
        assert_eq!(log.count("enemy-fire"), 0, "a staggered unit cannot shoot");
        assert_eq!(enemy.state, AiState::Staggered);
    }

    // -- movement ---------------------------------------------------------

    #[test]
    fn a_unit_slides_to_a_halt_while_staggered() {
        // Stopping dead would read as a pause rather than as being knocked off
        // balance.
        let collision = world();
        let mut enemy = Enemy::new(EnemyKind::Skirmisher);
        enemy.spawn(Vec3::ZERO, 0.0);
        enemy.vel = Vec3::new(10.0, 0.0, 0.0);
        enemy.stagger_timer = 1.0;

        run(&mut enemy, 0.5, Vec3::new(0.0, 0.0, 30.0), &collision);
        assert!(
            enemy.vel.horizontal_length() < 10.0,
            "it should have slowed, got {}",
            enemy.vel.horizontal_length()
        );
        assert!(
            enemy.vel.horizontal_length() > 0.0,
            "but not stopped instantly"
        );
    }

    #[test]
    fn a_unit_stays_on_the_ground() {
        let collision = world();
        let mut enemy = Enemy::new(EnemyKind::Skirmisher);
        enemy.spawn(Vec3::new(0.0, 20.0, 0.0), 0.0);

        run(&mut enemy, 3.0, Vec3::new(0.0, 0.0, 30.0), &collision);
        assert!(
            enemy.pos.y.abs() < 0.1,
            "it should have fallen to the floor, ended at {}",
            enemy.pos.y
        );
    }

    #[test]
    fn a_unit_cannot_leave_the_arena() {
        let collision = world();
        let mut enemy = Enemy::new(EnemyKind::Skirmisher);
        // Aimed well outside, with a goal to match.
        enemy.spawn(Vec3::new(140.0, 0.0, 0.0), 0.0);
        enemy.goal = Vec3::new(500.0, 0.0, 0.0);
        enemy.vel = Vec3::new(40.0, 0.0, 0.0);

        run(&mut enemy, 3.0, Vec3::new(140.0, 0.0, 0.0), &collision);
        let limit = world_cfg::HALF - enemy.radius - 2.0;
        assert!(
            enemy.pos.x <= limit + 0.01,
            "escaped to {}, limit {limit}",
            enemy.pos.x
        );
    }

    #[test]
    fn a_stuck_unit_re_routes_instead_of_grinding() {
        // The stuck detector. A unit pinned against geometry it cannot steer
        // around should pick a new goal rather than push forever.
        let mut collision = CollisionWorld::new(0.0);
        // A wall directly between the unit and where it wants to go.
        collision.add(crate::collision::Box::from_size(
            0.0, 5.0, 6.0, 60.0, 10.0, 2.0, "wall",
        ));

        let mut enemy = Enemy::new(EnemyKind::Skirmisher);
        enemy.spawn(Vec3::new(0.0, 0.0, 0.0), 0.0);
        // Goal beyond the wall, so the straight line is blocked.
        enemy.goal = Vec3::new(0.0, 0.0, 40.0);
        enemy.cooldown = 100.0; // not shooting; this is about movement

        let before = enemy.goal;
        run(&mut enemy, 4.0, Vec3::new(0.0, 0.0, 40.0), &collision);
        assert!(
            (enemy.goal - before).horizontal_length() > 1.0,
            "the goal should have been re-picked rather than pushed at"
        );
    }

    // -- the roster -------------------------------------------------------

    #[test]
    fn the_roster_reuses_dead_slots() {
        // Reuse rather than allocate, so a long mission with many waves does
        // not grow the list without bound.
        let mut roster = EnemyRoster::new();
        let first = roster.obtain(EnemyKind::Skirmisher).id;
        roster.by_id_mut(first).expect("it exists").alive = false;

        let second = roster.obtain(EnemyKind::Skirmisher).id;
        assert_eq!(first, second, "the dead slot should have been reused");
        assert_eq!(roster.list.len(), 1);
    }

    #[test]
    fn the_roster_does_not_reuse_a_slot_for_a_different_archetype() {
        // A skirmisher's slot is not an artillery's: the figures differ, and a
        // reused unit would keep the wrong ones.
        let mut roster = EnemyRoster::new();
        let skirmisher = roster.obtain(EnemyKind::Skirmisher).id;
        roster.by_id_mut(skirmisher).expect("it exists").alive = false;

        let artillery = roster.obtain(EnemyKind::Artillery).id;
        assert_ne!(skirmisher, artillery);
        assert_eq!(roster.list.len(), 2);
    }

    #[test]
    fn roster_identifiers_are_unique() {
        let mut roster = EnemyRoster::new();
        let mut ids = std::collections::HashSet::new();
        for kind in [
            EnemyKind::Skirmisher,
            EnemyKind::Artillery,
            EnemyKind::Boss,
            EnemyKind::Skirmisher,
        ] {
            let id = roster.obtain(kind).id;
            assert!(ids.insert(id), "id {id} was handed out twice");
        }
    }

    #[test]
    fn enemy_identifiers_do_not_collide_with_the_player() {
        // The player is id 1; enemies start past 1000.
        let mut roster = EnemyRoster::new();
        assert!(roster.obtain(EnemyKind::Skirmisher).id >= 1000);
    }

    #[test]
    fn clearing_the_roster_deactivates_everything() {
        let mut roster = EnemyRoster::new();
        roster.obtain(EnemyKind::Skirmisher);
        roster.obtain(EnemyKind::Artillery);
        assert_eq!(roster.active_count(), 2);
        roster.clear();
        assert_eq!(roster.active_count(), 0);
    }

    #[test]
    fn the_roster_steps_only_living_units() {
        let collision = world();
        let mut roster = EnemyRoster::new();
        let a = roster.obtain(EnemyKind::Skirmisher).id;
        let b = roster.obtain(EnemyKind::Skirmisher).id;
        roster.by_id_mut(a).expect("exists").spawn(Vec3::ZERO, 0.0);
        roster
            .by_id_mut(b)
            .expect("exists")
            .spawn(Vec3::new(20.0, 0.0, 0.0), 0.0);
        roster.by_id_mut(b).expect("exists").alive = false;

        let mut projectiles = ProjectileSystem::new();
        let mut log = EventLog::new();
        let mut damageables: Vec<Damageable> = Vec::new();
        let mut rng = Rng::default();
        let dt = crate::config::sim::DT;

        for _ in 0..240 {
            let mut ctx = EnemyWorld {
                world: &collision,
                projectiles: &mut projectiles,
                hooks: &mut log,
                target: player_at(Vec3::new(0.0, 0.0, 30.0)),
                target_alive: true,
                damageables: &mut damageables,
                rng: &mut rng,
                time: 0.0,
            };
            roster.step(dt, &mut ctx);
        }

        let dead = roster.by_id(b).expect("exists");
        assert_eq!(
            dead.pos,
            Vec3::new(20.0, 0.0, 0.0),
            "a dead unit should not have moved"
        );
    }

    #[test]
    fn the_roster_generator_is_reproducible() {
        // A replay has to reproduce enemy scatter, or the fight diverges.
        let collision = world();
        let run_once = || {
            let mut roster = EnemyRoster::new();
            let id = roster.obtain(EnemyKind::Skirmisher).id;
            roster.by_id_mut(id).expect("exists").spawn(Vec3::ZERO, 0.0);

            let mut projectiles = ProjectileSystem::new();
            let mut log = EventLog::new();
            let mut damageables: Vec<Damageable> = Vec::new();
            let mut rng = Rng::default();
            let dt = crate::config::sim::DT;

            for _ in 0..480 {
                let mut ctx = EnemyWorld {
                    world: &collision,
                    projectiles: &mut projectiles,
                    hooks: &mut log,
                    target: player_at(Vec3::new(0.0, 0.0, 30.0)),
                    target_alive: true,
                    damageables: &mut damageables,
                    rng: &mut rng,
                    time: 0.0,
                };
                roster.step(dt, &mut ctx);
            }
            (
                roster.list[0].pos,
                log.events
                    .iter()
                    .filter(|e| e.kind() == "enemy-fire")
                    .count(),
            )
        };

        assert_eq!(run_once(), run_once(), "two identical runs must agree");
    }
}
