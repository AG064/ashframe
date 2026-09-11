//! Target lock.
//!
//! Ported from the original's `game/targeting.ts`: *"Locking scores candidates
//! by how close they sit to the reticle, then holds them through a grace window
//! so a brief occlusion or a fast strafe does not flicker the lock. It never
//! snaps to an unrelated enemy: acquisition only happens inside a narrow cone,
//! and breaking requires leaving a wider one."*
//!
//! ## One structural difference from the original
//!
//! The original held a direct reference to the locked `Damageable`. Rust will
//! not allow that beside the enemy list it came from — the borrow checker is
//! right, and the alternative would be a reference-counted cell on a hot path.
//!
//! So this stores the locked **id** and resolves it against the enemy list in
//! the methods that need a position. The behaviour is identical, and the
//! indirection buys something: a lock can no longer point at an enemy that has
//! been removed from the list, which in the original would have been a dangling
//! reference the moment the array was rebuilt.

use crate::collision::CollisionWorld;
use crate::config::targeting as cfg;
use crate::types::{Damageable, Hooks, SimEvent, Vec3};

/// Where the reticle is and which way it points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TargetingView {
    pub yaw: f32,
    pub pitch: f32,
    pub at: Vec3,
}

impl TargetingView {
    /// The unit vector the reticle points along.
    ///
    /// Yaw is measured from `+Z` and pitch from the horizon, matching the
    /// original's convention throughout.
    pub fn forward(&self) -> Vec3 {
        let cos_pitch = self.pitch.cos();
        Vec3::new(
            self.yaw.sin() * cos_pitch,
            self.pitch.sin(),
            self.yaw.cos() * cos_pitch,
        )
    }
}

/// One enemy that could be locked, and how good a choice it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LockCandidate {
    pub target_id: u32,
    /// Angle in radians from the reticle.
    pub angle: f32,
    pub distance: f32,
    /// Lower is better.
    pub score: f32,
}

/// The lock.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TargetingSystem {
    locked_id: Option<u32>,
    /// Smoothed lock point, used by the HUD and by missile guidance.
    pub lock_point: Vec3,
    /// True while a lock exists but is temporarily invalid, inside the grace
    /// window. The HUD shows a different reticle for it.
    pub holding: bool,
    grace: f32,
}

impl TargetingSystem {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn locked_id(&self) -> Option<u32> {
        self.locked_id
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Score every visible enemy against the current reticle.
    ///
    /// Sorted best-first, so `acquire` takes the head and `cycle` walks the
    /// list in the order a player would expect.
    pub fn candidates(
        &self,
        view: &TargetingView,
        enemies: &[Damageable],
        world: &CollisionWorld,
    ) -> Vec<LockCandidate> {
        let forward = view.forward();
        let mut out = Vec::new();

        for enemy in enemies {
            if !enemy.alive {
                continue;
            }
            let aim = enemy.pos + Vec3::new(0.0, enemy.height * 0.5, 0.0);
            let offset = aim - view.at;
            let distance = offset.length();

            // Outside the range worth considering, at either end. The lower
            // bound matters: dividing by a near-zero distance makes the angle
            // jump around, so an enemy standing on the reticle would flicker in
            // and out of the candidate list.
            if !(1.0..=cfg::MAX_RANGE).contains(&distance) {
                continue;
            }

            let dot = (offset.dot(forward) / distance).clamp(-1.0, 1.0);
            let angle = dot.acos();
            // Acquisition is refused outside the *break* cone, so anything the
            // player could not hold is never offered.
            if angle > cfg::BREAK_CONE {
                continue;
            }

            // Cover blocks acquisition exactly as it blocks a shot. Without
            // this a lock could be taken on something the player cannot hit,
            // and the missile would fly into the wall.
            if !world.line_of_sight(view.at, aim) {
                continue;
            }

            // Prefer what is closest to the reticle, then what is nearest.
            let score = angle * 2.4 + (distance / cfg::MAX_RANGE) * 0.6;
            out.push(LockCandidate {
                target_id: enemy.id,
                angle,
                distance,
                score,
            });
        }

        out.sort_by(|a, b| a.score.total_cmp(&b.score));
        out
    }

    /// Acquire the best candidate, as the E key does.
    ///
    /// Returns whether a lock was taken. Acquisition is stricter than
    /// candidate scoring: only something inside the narrow cone is eligible, so
    /// pressing the key never snaps to an enemy off to one side.
    pub fn acquire(
        &mut self,
        view: &TargetingView,
        enemies: &[Damageable],
        world: &CollisionWorld,
        hooks: &mut dyn Hooks,
    ) -> bool {
        let list = self.candidates(view, enemies, world);
        let Some(best) = list.iter().find(|c| c.angle <= cfg::CONE) else {
            return false;
        };
        let id = best.target_id;
        self.set_lock(id, enemies, hooks);
        true
    }

    /// Cycle to the next candidate in screen order, as the mouse wheel does.
    ///
    /// Wraps at both ends, so a scroll in one direction keeps producing new
    /// targets rather than stopping at the last one.
    pub fn cycle(
        &mut self,
        direction: f32,
        view: &TargetingView,
        enemies: &[Damageable],
        world: &CollisionWorld,
        hooks: &mut dyn Hooks,
    ) {
        let list = self.candidates(view, enemies, world);
        if list.is_empty() {
            self.clear_lock(hooks);
            return;
        }
        let current = self
            .locked_id
            .and_then(|id| list.iter().position(|c| c.target_id == id));
        let len = list.len() as i32;
        let step = if direction >= 0.0 { 1 } else { -1 };
        let next = match current {
            None => 0,
            Some(index) => (index as i32 + step + len) % len,
        };
        let id = list[next as usize].target_id;
        self.set_lock(id, enemies, hooks);
    }

    fn set_lock(&mut self, id: u32, enemies: &[Damageable], hooks: &mut dyn Hooks) {
        if self.locked_id == Some(id) {
            return;
        }
        let Some(target) = enemies.iter().find(|e| e.id == id) else {
            return;
        };
        if let Some(previous) = self.locked_id {
            hooks.emit(&SimEvent::LockLost { target: previous });
        }
        self.locked_id = Some(id);
        self.grace = cfg::GRACE;
        self.holding = false;
        self.lock_point = target.pos + Vec3::new(0.0, target.height * 0.5, 0.0);
        hooks.emit(&SimEvent::LockAcquired { target: id });
    }

    pub fn clear_lock(&mut self, hooks: &mut dyn Hooks) {
        let Some(previous) = self.locked_id else {
            return;
        };
        hooks.emit(&SimEvent::LockLost { target: previous });
        self.locked_id = None;
        self.holding = false;
        self.grace = 0.0;
    }

    /// The current lock, if it is still alive.
    pub fn target<'a>(&self, enemies: &'a [Damageable]) -> Option<&'a Damageable> {
        let id = self.locked_id?;
        enemies.iter().find(|e| e.id == id && e.alive)
    }

    /// Validate and track the lock each step.
    pub fn update(
        &mut self,
        dt: f32,
        view: &TargetingView,
        enemies: &[Damageable],
        world: &CollisionWorld,
        hooks: &mut dyn Hooks,
    ) {
        let Some(id) = self.locked_id else {
            return;
        };
        let Some(target) = enemies.iter().find(|e| e.id == id) else {
            self.clear_lock(hooks);
            return;
        };
        if !target.alive {
            self.clear_lock(hooks);
            return;
        }

        let aim = target.pos + Vec3::new(0.0, target.height * 0.5, 0.0);
        let offset = aim - view.at;
        let distance = offset.length();
        let forward = view.forward();

        // The guard is on the divisor rather than assumed: at zero distance the
        // angle would be `NaN`, every comparison against it false, and the lock
        // would be dropped for a reason nothing reports.
        let dot = (offset.dot(forward) / distance.max(1e-4)).clamp(-1.0, 1.0);
        let angle = dot.acos();
        let visible = world.line_of_sight(view.at, aim);

        // The range is generous here relative to acquisition. A target that
        // starts running should stay locked for a moment rather than breaking
        // the instant it crosses a line.
        let valid = distance <= cfg::MAX_RANGE * 1.15 && angle <= cfg::BREAK_CONE && visible;

        if valid {
            self.grace = cfg::GRACE;
            self.holding = false;
        } else {
            self.grace -= dt;
            self.holding = true;
            if self.grace <= 0.0 {
                self.clear_lock(hooks);
                return;
            }
        }

        // Smoothed so the reticle and the missiles track without jitter. The
        // frame-rate independence comes from `damp`'s exponential, which is why
        // this is written as one rather than as a plain lerp by `dt`.
        let k = 1.0 - (-cfg::TRACK_RATE * dt).exp();
        self.lock_point += (aim - self.lock_point) * k;
    }

    /// How long the lock has left before it breaks, for a HUD that shows it.
    pub fn grace_remaining(&self) -> f32 {
        self.grace
    }

    /// Soft camera assist toward the locked target.
    ///
    /// Returns an additive yaw and pitch, not an absolute aim: the player keeps
    /// authority and this only leans. The fade is the important part — assist
    /// drops to nothing beyond 0.7 radians, so it never fights a deliberate
    /// turn. Without it, swinging the camera away from a locked target feels
    /// like the game is arguing.
    pub fn assist(&self, dt: f32, view: &TargetingView, enemies: &[Damageable]) -> (f32, f32) {
        let Some(target) = self.target(enemies) else {
            return (0.0, 0.0);
        };

        let aim = target.pos + Vec3::new(0.0, target.height * 0.55, 0.0);
        let offset = aim - view.at;
        let want_yaw = offset.x.atan2(offset.z);
        let flat = (offset.x * offset.x + offset.z * offset.z).sqrt();
        let want_pitch = offset.y.atan2(flat);

        let mut delta_yaw = want_yaw - view.yaw;
        while delta_yaw > std::f32::consts::PI {
            delta_yaw -= std::f32::consts::PI * 2.0;
        }
        while delta_yaw < -std::f32::consts::PI {
            delta_yaw += std::f32::consts::PI * 2.0;
        }
        let delta_pitch = want_pitch - view.pitch;

        let fade = |v: f32| {
            let a = v.abs();
            // Written as the positive case so the range is explicit: the pull
            // is full at zero error and falls linearly to nothing at 0.7.
            if a <= 0.7 {
                1.0 - a / 0.7
            } else {
                0.0
            }
        };

        let k = 1.0 - (-6.0 * dt).exp();
        (
            delta_yaw * k * fade(delta_yaw) * (cfg::ASSIST_YAW / 2.1),
            delta_pitch * k * fade(delta_pitch) * (cfg::ASSIST_PITCH / 1.6),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::EventLog;

    fn enemy(id: u32, at: Vec3) -> Damageable {
        Damageable {
            id,
            faction: crate::types::Faction::Enemy,
            name: format!("e{id}"),
            pos: at,
            vel: Vec3::ZERO,
            radius: 1.35,
            height: 4.6,
            health: 240.0,
            max_health: 240.0,
            stability: 60.0,
            max_stability: 60.0,
            alive: true,
            stagger_timer: 0.0,
            invuln_timer: 0.0,
            stagger_armed: true,
        }
    }

    /// Looking straight down `+Z` from the origin.
    fn view() -> TargetingView {
        TargetingView {
            yaw: 0.0,
            pitch: 0.0,
            at: Vec3::ZERO,
        }
    }

    fn empty_world() -> CollisionWorld {
        CollisionWorld::new(0.0)
    }

    #[test]
    fn forward_follows_yaw_and_pitch() {
        let f = view().forward();
        assert!((f.z - 1.0).abs() < 1e-5, "yaw 0 should look down +Z");

        let quarter = TargetingView {
            yaw: std::f32::consts::FRAC_PI_2,
            pitch: 0.0,
            at: Vec3::ZERO,
        };
        let f = quarter.forward();
        assert!(
            (f.x - 1.0).abs() < 1e-5,
            "a quarter turn should look down +X"
        );

        let up = TargetingView {
            yaw: 0.0,
            pitch: std::f32::consts::FRAC_PI_2,
            at: Vec3::ZERO,
        };
        let f = up.forward();
        assert!((f.y - 1.0).abs() < 1e-5, "pitch up should look up");
    }

    #[test]
    fn an_enemy_in_front_is_a_candidate() {
        let world = empty_world();
        let enemies = [enemy(1, Vec3::new(0.0, 0.0, 40.0))];
        let list = TargetingSystem::new().candidates(&view(), &enemies, &world);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].target_id, 1);
        assert!(
            list[0].angle < 0.1,
            "straight ahead should be a small angle, got {}",
            list[0].angle
        );
    }

    #[test]
    fn an_enemy_behind_is_not_a_candidate() {
        let world = empty_world();
        let enemies = [enemy(1, Vec3::new(0.0, 0.0, -40.0))];
        assert!(TargetingSystem::new()
            .candidates(&view(), &enemies, &world)
            .is_empty());
    }

    #[test]
    fn a_dead_enemy_is_not_a_candidate() {
        let world = empty_world();
        let mut dead = enemy(1, Vec3::new(0.0, 0.0, 40.0));
        dead.alive = false;
        assert!(TargetingSystem::new()
            .candidates(&view(), &[dead], &world)
            .is_empty());
    }

    #[test]
    fn a_distant_enemy_is_out_of_range() {
        let world = empty_world();
        let far = enemy(1, Vec3::new(0.0, 0.0, cfg::MAX_RANGE + 10.0));
        assert!(TargetingSystem::new()
            .candidates(&view(), &[far], &world)
            .is_empty());
    }

    #[test]
    fn cover_blocks_acquisition() {
        // Otherwise a lock is taken on something the player cannot hit, and the
        // missile flies into the wall.
        let mut world = CollisionWorld::new(0.0);
        world.add(crate::collision::Box::from_size(
            0.0, 2.0, 20.0, 10.0, 8.0, 4.0, "wall",
        ));
        let enemies = [enemy(1, Vec3::new(0.0, 0.0, 40.0))];
        assert!(TargetingSystem::new()
            .candidates(&view(), &enemies, &world)
            .is_empty());
    }

    #[test]
    fn candidates_are_ordered_best_first() {
        let world = empty_world();
        let enemies = [
            enemy(1, Vec3::new(18.0, 0.0, 60.0)), // off to the side
            enemy(2, Vec3::new(0.0, 0.0, 40.0)),  // dead ahead
            enemy(3, Vec3::new(0.0, 0.0, 90.0)),  // dead ahead but further
        ];
        let list = TargetingSystem::new().candidates(&view(), &enemies, &world);
        assert_eq!(list[0].target_id, 2, "closest to the reticle wins");
        assert_eq!(list[1].target_id, 3, "then the nearer of the rest");
        assert!(list[0].score <= list[1].score);
        assert!(list[1].score <= list[2].score);
    }

    // -- acquiring --------------------------------------------------------

    #[test]
    fn acquiring_takes_the_enemy_in_front() {
        let world = empty_world();
        let enemies = [enemy(7, Vec3::new(0.0, 0.0, 40.0))];
        let mut system = TargetingSystem::new();
        let mut log = EventLog::new();

        assert!(system.acquire(&view(), &enemies, &world, &mut log));
        assert_eq!(system.locked_id(), Some(7));
        assert_eq!(log.count("lock-acquired"), 1);
    }

    #[test]
    fn acquiring_nothing_reports_nothing() {
        let world = empty_world();
        let mut system = TargetingSystem::new();
        let mut log = EventLog::new();
        assert!(!system.acquire(&view(), &[], &world, &mut log));
        assert_eq!(system.locked_id(), None);
        assert!(log.events.is_empty());
    }

    #[test]
    fn acquisition_is_stricter_than_candidacy() {
        // An enemy inside the break cone but outside the acquisition cone is
        // visible to `cycle` but must not be taken by pressing E, or the key
        // would snap to something off to one side.
        let world = empty_world();
        // Placed so the angle sits between the two cones.
        let angle = (cfg::CONE + cfg::BREAK_CONE) / 2.0;
        let distance = 60.0;
        let at = Vec3::new(angle.sin() * distance, 0.0, angle.cos() * distance);
        let enemies = [enemy(1, at)];

        let system = TargetingSystem::new();
        assert_eq!(
            system.candidates(&view(), &enemies, &world).len(),
            1,
            "it should be a candidate"
        );

        let mut system = TargetingSystem::new();
        let mut log = EventLog::new();
        assert!(
            !system.acquire(&view(), &enemies, &world, &mut log),
            "but acquiring should refuse it"
        );
    }

    #[test]
    fn re_acquiring_the_same_target_does_not_re_announce_it() {
        // The HUD plays a sound on acquisition; repeating it every frame the
        // key is held would be a buzz.
        let world = empty_world();
        let enemies = [enemy(1, Vec3::new(0.0, 0.0, 40.0))];
        let mut system = TargetingSystem::new();
        let mut log = EventLog::new();

        system.acquire(&view(), &enemies, &world, &mut log);
        system.acquire(&view(), &enemies, &world, &mut log);
        assert_eq!(log.count("lock-acquired"), 1);
    }

    #[test]
    fn acquiring_a_new_target_announces_both_halves() {
        // No world needed: this is about the events, not about visibility.
        let enemies = [
            enemy(1, Vec3::new(0.0, 0.0, 40.0)),
            enemy(2, Vec3::new(0.0, 0.0, 50.0)),
        ];
        let mut system = TargetingSystem::new();
        let mut log = EventLog::new();

        system.set_lock(1, &enemies, &mut log);
        log.clear();
        system.set_lock(2, &enemies, &mut log);

        assert_eq!(log.count("lock-lost"), 1, "the old lock is released");
        assert_eq!(log.count("lock-acquired"), 1, "and the new one taken");
    }

    // -- cycling ----------------------------------------------------------

    #[test]
    fn cycling_advances_and_wraps() {
        let world = empty_world();
        let enemies = [
            enemy(1, Vec3::new(0.0, 0.0, 40.0)),
            enemy(2, Vec3::new(0.0, 0.0, 60.0)),
            enemy(3, Vec3::new(0.0, 0.0, 80.0)),
        ];
        let mut system = TargetingSystem::new();
        let mut log = EventLog::new();

        system.cycle(1.0, &view(), &enemies, &world, &mut log);
        let first = system.locked_id().unwrap();
        system.cycle(1.0, &view(), &enemies, &world, &mut log);
        let second = system.locked_id().unwrap();
        assert_ne!(first, second, "cycling should move");

        // Two more steps from anywhere returns to where it started, because
        // there are three candidates.
        system.cycle(1.0, &view(), &enemies, &world, &mut log);
        system.cycle(1.0, &view(), &enemies, &world, &mut log);
        assert_eq!(system.locked_id(), Some(first), "cycling should wrap");
    }

    #[test]
    fn cycling_backwards_moves_the_other_way() {
        let world = empty_world();
        let enemies = [
            enemy(1, Vec3::new(0.0, 0.0, 40.0)),
            enemy(2, Vec3::new(0.0, 0.0, 60.0)),
        ];
        let mut system = TargetingSystem::new();
        let mut log = EventLog::new();

        system.cycle(1.0, &view(), &enemies, &world, &mut log);
        let forward = system.locked_id().unwrap();
        system.cycle(-1.0, &view(), &enemies, &world, &mut log);
        let back = system.locked_id().unwrap();
        assert_ne!(forward, back);
        system.cycle(-1.0, &view(), &enemies, &world, &mut log);
        assert_eq!(system.locked_id(), Some(forward), "two back is one forward");
    }

    #[test]
    fn cycling_with_nothing_to_lock_clears_the_lock() {
        let world = empty_world();
        let mut system = TargetingSystem::new();
        let mut log = EventLog::new();
        system.set_lock(1, &[enemy(1, Vec3::ZERO)], &mut log);
        log.clear();

        system.cycle(1.0, &view(), &[], &world, &mut log);
        assert_eq!(system.locked_id(), None);
        assert_eq!(log.count("lock-lost"), 1);
    }

    // -- holding and breaking ---------------------------------------------

    #[test]
    fn a_visible_target_keeps_its_lock() {
        let world = empty_world();
        let enemies = [enemy(1, Vec3::new(0.0, 0.0, 40.0))];
        let mut system = TargetingSystem::new();
        let mut log = EventLog::new();
        system.acquire(&view(), &enemies, &world, &mut log);

        for _ in 0..120 {
            system.update(1.0 / 120.0, &view(), &enemies, &world, &mut log);
        }
        assert_eq!(system.locked_id(), Some(1));
        assert!(!system.holding);
        assert_eq!(log.count("lock-lost"), 0);
    }

    #[test]
    fn a_brief_occlusion_holds_then_breaks() {
        // The grace window's whole purpose: a strafing enemy should not make
        // the reticle flicker.
        let world = empty_world();
        let enemies = [enemy(1, Vec3::new(0.0, 0.0, 40.0))];
        let mut system = TargetingSystem::new();
        let mut log = EventLog::new();
        system.acquire(&view(), &enemies, &world, &mut log);

        // Swing the view away far enough to break.
        let away = TargetingView {
            yaw: cfg::BREAK_CONE + 0.4,
            pitch: 0.0,
            at: Vec3::ZERO,
        };

        let step = 1.0 / 120.0;
        let mut held_steps = 0;
        while system.locked_id().is_some() && held_steps < 1000 {
            system.update(step, &away, &enemies, &world, &mut log);
            held_steps += 1;
            if system.locked_id().is_some() {
                assert!(system.holding, "an invalid lock should report holding");
            }
        }

        let held_seconds = held_steps as f32 * step;
        assert!(
            (held_seconds - cfg::GRACE).abs() < 0.05,
            "the lock should survive about {}s, survived {held_seconds}s",
            cfg::GRACE
        );
        assert_eq!(log.count("lock-lost"), 1);
    }

    #[test]
    fn a_target_dying_breaks_the_lock_immediately() {
        // No grace for death: the enemy is gone, and holding a lock on a
        // corpse would point the missiles at nothing.
        let world = empty_world();
        let mut enemies = [enemy(1, Vec3::new(0.0, 0.0, 40.0))];
        let mut system = TargetingSystem::new();
        let mut log = EventLog::new();
        system.acquire(&view(), &enemies, &world, &mut log);

        enemies[0].alive = false;
        system.update(1.0 / 120.0, &view(), &enemies, &world, &mut log);
        assert_eq!(system.locked_id(), None);
        assert_eq!(log.count("lock-lost"), 1);
    }

    #[test]
    fn a_target_removed_from_the_list_breaks_the_lock() {
        // The case the id indirection makes safe. In the original this was a
        // reference into an array that had been rebuilt.
        let world = empty_world();
        let mut system = TargetingSystem::new();
        let mut log = EventLog::new();
        system.set_lock(1, &[enemy(1, Vec3::ZERO)], &mut log);

        system.update(1.0 / 120.0, &view(), &[], &world, &mut log);
        assert_eq!(system.locked_id(), None);
    }

    #[test]
    fn the_lock_point_smooths_toward_a_moving_target() {
        // Acquiring puts the point straight on the target, so the smoothing
        // only shows once the target moves. A point that snapped instead would
        // make the reticle and the missile guidance jitter every step.
        let world = empty_world();
        let mut enemies = [enemy(1, Vec3::new(0.0, 0.0, 40.0))];
        let mut system = TargetingSystem::new();
        let mut log = EventLog::new();
        system.acquire(&view(), &enemies, &world, &mut log);
        assert!(
            (system.lock_point.x).abs() < 1e-4,
            "acquiring should put the point on the target"
        );

        // Strafe ten metres to the side.
        enemies[0].pos = Vec3::new(10.0, 0.0, 40.0);
        system.update(1.0 / 120.0, &view(), &enemies, &world, &mut log);
        let after_one = system.lock_point.x;
        assert!(after_one > 0.0, "it should have started moving");
        assert!(
            after_one < 10.0,
            "one step should not arrive, got {after_one}"
        );

        for _ in 0..600 {
            system.update(1.0 / 120.0, &view(), &enemies, &world, &mut log);
        }
        assert!(
            (system.lock_point.x - 10.0).abs() < 0.1,
            "it should converge on the new position, got {}",
            system.lock_point.x
        );
    }

    #[test]
    fn the_lock_point_converges_the_same_at_any_frame_rate() {
        // The exponential, not a per-step fraction, is what makes this true.
        // The target is moved first, because tracking a stationary one would
        // agree trivially and prove nothing.
        let world = empty_world();

        // A function rather than a closure, so each run gets its own enemy
        // rather than borrowing the one the other run has already moved.
        fn track(world: &CollisionWorld, steps: usize, dt: f32) -> f32 {
            let mut enemies = [enemy(1, Vec3::new(0.0, 0.0, 40.0))];
            let mut system = TargetingSystem::new();
            let mut log = EventLog::new();
            system.acquire(&view(), &enemies, world, &mut log);
            enemies[0].pos = Vec3::new(30.0, 0.0, 40.0);
            for _ in 0..steps {
                system.update(dt, &view(), &enemies, world, &mut log);
            }
            system.lock_point.x
        }

        let coarse = track(&world, 12, 1.0 / 12.0);
        let fine = track(&world, 120, 1.0 / 120.0);
        assert!(
            (coarse - fine).abs() < 0.5,
            "1 second of tracking diverged: {coarse} vs {fine}"
        );
    }

    // -- assist -----------------------------------------------------------

    #[test]
    fn assist_leans_toward_a_locked_target() {
        let enemies = [enemy(1, Vec3::new(10.0, 0.0, 40.0))];
        let mut system = TargetingSystem::new();
        let mut log = EventLog::new();
        system.set_lock(1, &enemies, &mut log);

        let (yaw, _pitch) = system.assist(1.0 / 120.0, &view(), &enemies);
        assert!(
            yaw > 0.0,
            "the target is to the right, so assist should pull right"
        );
    }

    #[test]
    fn assist_fades_out_for_a_large_error() {
        // Otherwise swinging the camera away from a lock feels like the game
        // is arguing with the player.
        let enemies = [enemy(1, Vec3::new(0.0, 0.0, -40.0))];
        let mut system = TargetingSystem::new();
        let mut log = EventLog::new();
        system.set_lock(1, &enemies, &mut log);

        let (yaw, pitch) = system.assist(1.0 / 120.0, &view(), &enemies);
        assert_eq!(
            yaw, 0.0,
            "a target a half-turn away should be left entirely alone"
        );
        assert!(
            pitch.abs() < 0.01,
            "and the vertical pull should be negligible, got {pitch}"
        );
    }

    #[test]
    fn assist_does_nothing_without_a_lock() {
        let system = TargetingSystem::new();
        assert_eq!(system.assist(1.0 / 120.0, &view(), &[]), (0.0, 0.0));
    }

    #[test]
    fn assist_does_not_point_at_a_dead_target() {
        let mut dead = enemy(1, Vec3::new(10.0, 0.0, 40.0));
        dead.alive = false;
        let enemies = [dead];
        let mut system = TargetingSystem::new();
        // Locked while alive, then killed.
        let mut live = enemies.clone();
        live[0].alive = true;
        let mut log = EventLog::new();
        system.set_lock(1, &live, &mut log);

        assert_eq!(system.assist(1.0 / 120.0, &view(), &enemies), (0.0, 0.0));
    }

    #[test]
    fn resetting_clears_everything() {
        let enemies = [enemy(1, Vec3::new(0.0, 0.0, 40.0))];
        let mut system = TargetingSystem::new();
        let mut log = EventLog::new();
        system.set_lock(1, &enemies, &mut log);

        system.reset();
        assert_eq!(system.locked_id(), None);
        assert_eq!(system.lock_point, Vec3::ZERO);
        assert!(!system.holding);
        assert_eq!(system.grace_remaining(), 0.0);
        assert!(system.target(&enemies).is_none());
    }
}
