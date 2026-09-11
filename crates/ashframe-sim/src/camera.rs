//! Third-person camera.
//!
//! Ported from the original's `game/camera.ts`: *"Over-the-shoulder follow with
//! mouse look, pitch limits, an obstruction probe that pulls the camera in so
//! walls never leave you inside geometry, a soft lock-on assist that never
//! fights deliberate input, and restrained shake."*
//!
//! The original wrote its result straight into a Three.js `PerspectiveCamera`.
//! Here it produces a [`CameraPose`] instead, which is what lets the same camera
//! run in a headless test: the pull-in against a wall, the ground clamp and the
//! FOV curve are all assertions about numbers, and none of them ever needed a
//! renderer to be true.
//!
//! The important thing the original said about this module is kept, because it
//! is the reason it exists separately from rendering at all: *"The camera writes
//! its pose into `Simulation.view` before the simulation steps, which is what
//! makes aiming start from what you actually see."* [`Camera::write_view`] is
//! that write. A camera that aimed from anywhere else would put the reticle
//! somewhere the player cannot see, which is the single most common way a
//! third-person shooter feels broken.

use crate::collision::CollisionWorld;
use crate::config::camera as cfg;
use crate::math::clamp;
use crate::targeting::TargetingView;
use crate::types::Vec3;

/// What the camera is following.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraTarget {
    pub at: Vec3,
    /// Height of the mech, used to place the pivot.
    pub height: f32,
    /// Body yaw, used for the over-shoulder offset.
    pub yaw: f32,
    pub assaulting: bool,
    pub speed: f32,
}

/// Where the camera ended up, and how wide it is looking.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraPose {
    pub position: Vec3,
    pub look_at: Vec3,
    pub fov: f32,
}

impl Default for CameraPose {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            look_at: Vec3::new(0.0, 0.0, 1.0),
            fov: cfg::FOV,
        }
    }
}

/// Yaw and pitch assist contributed by the targeting system.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Assist {
    pub yaw: f32,
    pub pitch: f32,
}

/// The camera itself.
#[derive(Debug, Clone)]
pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub sensitivity: f32,
    /// Extra pitch added by recoil and impacts, decaying quickly.
    kick_pitch: f32,
    kick_yaw: f32,
    shake: f32,
    shake_time: f32,
    fov: f32,
    pivot: Vec3,
    initialized: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Self::new()
    }
}

impl Camera {
    pub fn new() -> Self {
        Self {
            yaw: 0.0,
            pitch: -0.06,
            sensitivity: cfg::MOUSE_SENSITIVITY,
            kick_pitch: 0.0,
            kick_yaw: 0.0,
            shake: 0.0,
            shake_time: 0.0,
            fov: cfg::FOV,
            pivot: Vec3::ZERO,
            initialized: false,
        }
    }

    pub fn reset(&mut self, yaw: f32, target: CameraTarget) {
        self.yaw = yaw;
        self.pitch = -0.06;
        self.kick_pitch = 0.0;
        self.kick_yaw = 0.0;
        self.shake = 0.0;
        self.fov = cfg::FOV;
        self.pivot = Vec3::new(target.at.x, target.at.y + target.height * 0.62, target.at.z);
        self.initialized = true;
    }

    /// Whether the camera has been placed. A camera that has never been reset
    /// has no pivot, and stepping one would smear it in from the origin.
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Apply a mouse-look delta in device pixels.
    pub fn look(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * self.sensitivity;
        self.pitch -= dy * self.sensitivity;
        self.pitch = clamp(self.pitch, cfg::MIN_PITCH, cfg::MAX_PITCH);
        // Kept in a readable range for diagnostics rather than for the maths:
        // every use of yaw is through a sine or cosine, so an accumulating yaw
        // would work and would be useless to print.
        if self.yaw > std::f32::consts::PI {
            self.yaw -= std::f32::consts::PI * 2.0;
        }
        if self.yaw < -std::f32::consts::PI {
            self.yaw += std::f32::consts::PI * 2.0;
        }
    }

    /// Point the camera at a yaw and pitch, ignoring mouse input.
    pub fn set_look(&mut self, yaw: f32, pitch: f32) {
        self.yaw = yaw;
        self.pitch = clamp(pitch, cfg::MIN_PITCH, cfg::MAX_PITCH);
    }

    pub fn add_shake(&mut self, amount: f32) {
        self.shake = (self.shake + amount).min(1.6);
    }

    /// Current shake magnitude, for tests and debug readouts.
    pub fn shake(&self) -> f32 {
        self.shake
    }

    /// A small upward kick, used when heavy weapons fire.
    pub fn kick(&mut self, pitch: f32, yaw: f32) {
        self.kick_pitch += pitch;
        self.kick_yaw += yaw;
    }

    /// Advance the camera and return where it ended up.
    ///
    /// `locked` tightens the framing and speeds up the pivot, nothing more: a
    /// lock is an aid to aiming, never a change in where the player is looking.
    pub fn update(
        &mut self,
        dt: f32,
        target: CameraTarget,
        world: &CollisionWorld,
        locked: bool,
        assist: Assist,
    ) -> CameraPose {
        if !self.initialized {
            self.reset(target.yaw, target);
        }

        self.yaw += assist.yaw;
        self.pitch = clamp(self.pitch + assist.pitch, cfg::MIN_PITCH, cfg::MAX_PITCH);

        // Recoil kick decays fast. It is feel, not aim error: a kick that
        // survived long enough to matter would make sustained fire unusable.
        self.yaw += self.kick_yaw * dt * 6.0;
        self.pitch = clamp(
            self.pitch + self.kick_pitch * dt * 6.0,
            cfg::MIN_PITCH,
            cfg::MAX_PITCH,
        );
        self.kick_pitch *= (-9.0 * dt).exp();
        self.kick_yaw *= (-9.0 * dt).exp();

        // The pivot follows the mech's chest with a gentle spring. Exponential
        // rather than a fixed fraction per frame, so the follow behaves the
        // same at any step size.
        let desired_pivot = Vec3::new(target.at.x, target.at.y + target.height * 0.62, target.at.z);
        let rate = if locked {
            cfg::AIM_FOLLOW_RATE
        } else {
            cfg::FOLLOW_RATE
        };
        let k = 1.0 - (-rate * dt).exp();
        self.pivot = self.pivot + (desired_pivot - self.pivot) * k;

        let distance = if locked {
            cfg::LOCK_DISTANCE
        } else {
            cfg::DISTANCE
        };
        let cp = self.pitch.cos();
        let forward = Vec3::new(self.yaw.sin() * cp, self.pitch.sin(), self.yaw.cos() * cp);
        // Right vector on the horizontal plane, for the shoulder offset.
        let right = Vec3::new(self.yaw.cos(), 0.0, -self.yaw.sin());

        let desired = self.pivot - forward * distance
            + right * cfg::SHOULDER
            + Vec3::new(0.0, cfg::HEIGHT * 0.35, 0.0);

        // Obstruction: sweep from the pivot to the seat the camera wants and
        // pull in to just short of whatever it meets. Backing off by more than
        // the probe radius is what stops the near plane clipping into the wall
        // the camera is now sitting against.
        let offset = desired - self.pivot;
        let len = offset.length();
        let mut position = if len > 1e-4 {
            let direction = offset * (1.0 / len);
            let place = match world.sweep_sphere(self.pivot, direction, cfg::PROBE_RADIUS, len) {
                Some(hit) => (hit.t - 0.35).max(cfg::MIN_DISTANCE),
                None => distance,
            };
            self.pivot + offset * (place / len)
        } else {
            desired
        };

        // Never let the camera sink below the ground it is flying over.
        let ground = world.ground_at(position.x, position.z, position.y + 12.0, 60.0);
        if position.y < ground + 1.1 {
            position.y = ground + 1.1;
        }

        self.shake_time += dt;
        self.shake = (self.shake - cfg::SHAKE_DECAY * dt * self.shake - dt * 0.6).max(0.0);
        if self.shake > 0.001 {
            let s = self.shake * 0.5;
            position += Vec3::new(
                (self.shake_time * 47.3).sin() * s,
                (self.shake_time * 61.7).sin() * s,
                (self.shake_time * 39.1).sin() * s,
            );
        }

        // FOV: a small kick under assault boost, never a fisheye.
        let speed_norm = clamp(target.speed / 48.0, 0.0, 1.4);
        let target_fov = cfg::FOV
            + if target.assaulting {
                cfg::FOV_BOOST
            } else {
                0.0
            }
            + speed_norm * 3.0;
        let fk = 1.0 - (-6.0 * dt).exp();
        self.fov += (target_fov - self.fov) * fk;

        CameraPose {
            position,
            look_at: self.pivot + forward * 12.0,
            fov: self.fov,
        }
    }

    /// Write the pose into the simulation's aiming view.
    ///
    /// This is the whole reason the camera is stepped before the simulation:
    /// aiming reads this view, so the reticle has to be the camera's before
    /// anything asks where the player is pointing.
    pub fn write_view(&self, pose: &CameraPose, view: &mut TargetingView) {
        view.yaw = self.yaw;
        view.pitch = self.pitch;
        view.at = pose.position;
    }
}

/// The world direction a camera yaw and pitch are looking along.
pub fn camera_forward(yaw: f32, pitch: f32) -> Vec3 {
    let cp = pitch.cos();
    Vec3::new(yaw.sin() * cp, pitch.sin(), yaw.cos() * cp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::Box as Solid;

    fn target(at: Vec3) -> CameraTarget {
        CameraTarget {
            at,
            height: 6.0,
            yaw: 0.0,
            assaulting: false,
            speed: 0.0,
        }
    }

    fn flat() -> CollisionWorld {
        CollisionWorld::new(0.0)
    }

    fn settle(camera: &mut Camera, world: &CollisionWorld, t: CameraTarget) -> CameraPose {
        let mut pose = CameraPose::default();
        for _ in 0..240 {
            pose = camera.update(1.0 / 60.0, t, world, false, Assist::default());
        }
        pose
    }

    #[test]
    fn the_camera_sits_behind_and_above_the_mech() {
        let world = flat();
        let mut camera = Camera::new();
        let pose = settle(&mut camera, &world, target(Vec3::new(0.0, 0.0, 0.0)));
        // Facing +Z, so "behind" is -Z.
        assert!(pose.position.z < -8.0, "{:?}", pose.position);
        assert!(pose.position.y > 2.0, "{:?}", pose.position);
        assert!(pose.look_at.z > pose.position.z, "it should look forward");
    }

    #[test]
    fn looking_around_stops_at_the_pitch_limits() {
        let mut camera = Camera::new();
        camera.look(0.0, -100000.0);
        assert!((camera.pitch - cfg::MAX_PITCH).abs() < 1e-5);
        camera.look(0.0, 100000.0);
        assert!((camera.pitch - cfg::MIN_PITCH).abs() < 1e-5);
    }

    #[test]
    fn yaw_wraps_rather_than_accumulating() {
        let mut camera = Camera::new();
        for _ in 0..1000 {
            camera.look(100.0, 0.0);
            assert!(camera.yaw.abs() <= std::f32::consts::PI + 1e-3);
        }
    }

    #[test]
    fn a_wall_behind_the_mech_pulls_the_camera_in() {
        let mut world = flat();
        // A slab directly behind the mech, well inside the seat distance.
        world.add(Solid::from_size(0.0, 3.0, -6.0, 20.0, 6.0, 1.0, "wall"));

        let mut camera = Camera::new();
        let pose = settle(&mut camera, &world, target(Vec3::new(0.0, 0.0, 0.0)));
        assert!(
            pose.position.z > -6.0,
            "the camera pushed through the wall: {:?}",
            pose.position
        );
    }

    #[test]
    fn the_camera_never_sinks_into_the_floor() {
        let world = flat();
        let mut camera = Camera::new();
        // Looking up hard drives the seat downward; the ground clamp is what
        // stops it ending up under the arena.
        camera.set_look(0.0, cfg::MIN_PITCH);
        let pose = settle(&mut camera, &world, target(Vec3::new(0.0, 0.0, 0.0)));
        assert!(pose.position.y >= 1.1 - 1e-4, "{:?}", pose.position);
    }

    #[test]
    fn a_pull_in_is_bounded_by_the_minimum_distance() {
        let mut world = flat();
        // A wall slamming right up against the pivot.
        world.add(Solid::from_size(0.0, 3.0, -0.5, 20.0, 6.0, 1.0, "wall"));
        let mut camera = Camera::new();
        let pose = settle(&mut camera, &world, target(Vec3::new(0.0, 0.0, 0.0)));
        let distance = (pose.position - Vec3::new(0.0, 3.72, 0.0)).length();
        assert!(
            distance >= cfg::MIN_DISTANCE - 1e-3,
            "the camera collapsed onto the mech: {distance}"
        );
    }

    #[test]
    fn the_assault_boost_widens_the_lens_and_it_comes_back() {
        let world = flat();
        let mut camera = Camera::new();
        let base = settle(&mut camera, &world, target(Vec3::ZERO)).fov;

        let mut boosting = target(Vec3::ZERO);
        boosting.assaulting = true;
        let wide = settle(&mut camera, &world, boosting).fov;
        assert!(wide > base + 1.0, "{base} -> {wide}");

        let back = settle(&mut camera, &world, target(Vec3::ZERO)).fov;
        assert!((back - base).abs() < 0.5, "FOV did not settle back: {back}");
    }

    #[test]
    fn speed_opens_the_lens_a_little_and_never_past_the_boost() {
        let world = flat();
        let mut slow = Camera::new();
        let mut fast = Camera::new();
        let mut sprinting = target(Vec3::ZERO);
        sprinting.speed = 48.0;
        let a = settle(&mut slow, &world, target(Vec3::ZERO)).fov;
        let b = settle(&mut fast, &world, sprinting).fov;
        assert!(b > a, "speed should widen the view");
        assert!(
            b <= cfg::FOV + cfg::FOV_BOOST + 3.0 + 0.5,
            "the lens ran away: {b}"
        );
    }

    #[test]
    fn shake_decays_back_to_nothing() {
        let world = flat();
        let mut camera = Camera::new();
        camera.add_shake(1.2);
        assert!(camera.shake() > 1.0);
        settle(&mut camera, &world, target(Vec3::ZERO));
        assert!(camera.shake() < 0.001, "shake never settled");
    }

    #[test]
    fn shake_is_capped() {
        let mut camera = Camera::new();
        for _ in 0..50 {
            camera.add_shake(1.0);
        }
        assert!(camera.shake() <= 1.6 + 1e-6);
    }

    #[test]
    fn a_kick_climbs_the_aim_and_the_climb_settles() {
        let world = flat();
        let mut camera = Camera::new();
        settle(&mut camera, &world, target(Vec3::ZERO));
        let before = camera.pitch;

        camera.kick(0.2, 0.0);
        let after = camera.update(
            1.0 / 60.0,
            target(Vec3::ZERO),
            &world,
            false,
            Assist::default(),
        );
        assert!(after.look_at.y > 0.0);
        assert!(camera.pitch > before, "the kick did not lift the aim");

        // Recoil is climb, not bounce. The original accumulated the decaying
        // kick into pitch and never took it back, so the aim ends higher and
        // stays there; what is worth pinning is that the total is finite. A
        // kick that kept contributing would walk the aim into the sky over a
        // long burst.
        //
        // The exact total is a geometric sum over frames rather than the
        // continuous 0.2 * 6 / 9 = 0.133, so it is bounded rather than pinned:
        // 0.144 at 120 Hz, drifting toward 0.133 as the step shrinks.
        settle(&mut camera, &world, target(Vec3::ZERO));
        let climb = camera.pitch - before;
        assert!(
            (0.13..0.16).contains(&climb),
            "unexpected climb for a 0.2 kick: {climb}"
        );

        let settled = camera.pitch;
        settle(&mut camera, &world, target(Vec3::ZERO));
        assert!(
            (camera.pitch - settled).abs() < 1e-4,
            "the kick was still moving the aim: {settled} -> {}",
            camera.pitch
        );
    }

    #[test]
    fn locking_tightens_the_framing() {
        let world = flat();
        let mut free = Camera::new();
        let mut locked = Camera::new();
        let t = target(Vec3::ZERO);
        let mut a = CameraPose::default();
        let mut b = CameraPose::default();
        for _ in 0..240 {
            a = free.update(1.0 / 60.0, t, &world, false, Assist::default());
            b = locked.update(1.0 / 60.0, t, &world, true, Assist::default());
        }
        let da = (a.position - Vec3::new(0.0, 3.72, 0.0)).length();
        let db = (b.position - Vec3::new(0.0, 3.72, 0.0)).length();
        assert!(db < da, "a lock should sit closer: {db} vs {da}");
    }

    #[test]
    fn assist_moves_the_camera_without_a_mouse() {
        let world = flat();
        let mut camera = Camera::new();
        settle(&mut camera, &world, target(Vec3::ZERO));
        let yaw = camera.yaw;
        camera.update(
            1.0 / 60.0,
            target(Vec3::ZERO),
            &world,
            true,
            Assist {
                yaw: 0.3,
                pitch: 0.0,
            },
        );
        assert!((camera.yaw - yaw - 0.3).abs() < 1e-5);
    }

    #[test]
    fn the_written_view_is_the_camera_the_aim_reads() {
        let world = flat();
        let mut camera = Camera::new();
        let pose = settle(&mut camera, &world, target(Vec3::new(4.0, 0.0, 4.0)));
        let mut view = TargetingView::default();
        camera.write_view(&pose, &mut view);

        assert_eq!(view.yaw, camera.yaw);
        assert_eq!(view.pitch, camera.pitch);
        assert_eq!(view.at, pose.position);
        // And the view's own forward helper agrees with the camera's.
        let forward = camera_forward(view.yaw, view.pitch);
        assert!((forward - view.forward()).length() < 1e-5);
    }

    #[test]
    fn an_uninitialised_camera_places_itself_on_the_first_step() {
        let world = flat();
        let mut camera = Camera::new();
        assert!(!camera.is_initialized());
        camera.update(
            1.0 / 60.0,
            target(Vec3::ZERO),
            &world,
            false,
            Assist::default(),
        );
        assert!(camera.is_initialized());
    }

    #[test]
    fn the_pivot_follows_the_mech_without_snapping() {
        let world = flat();
        let mut camera = Camera::new();
        settle(&mut camera, &world, target(Vec3::ZERO));

        // Teleport the mech. The seat should travel, not jump.
        let moved = target(Vec3::new(40.0, 0.0, 0.0));
        let first = camera.update(1.0 / 60.0, moved, &world, false, Assist::default());
        assert!(
            first.position.x < 20.0,
            "the camera snapped to the new position: {:?}",
            first.position
        );

        let settled = settle(&mut camera, &world, moved);
        assert!(settled.position.x > 30.0, "{:?}", settled.position);
    }
}
