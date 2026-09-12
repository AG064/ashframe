//! The game node: the only thing GDScript talks to.
//!
//! It owns the simulation, the camera and everything drawn, and it is
//! deliberately the only entry point. A HUD that could reach into the
//! simulation would eventually start making decisions with it, and the split
//! between what the game *is* and what it *looks like* is what let the renderer
//! be replaced — Three.js for Godot — without rewriting the fight.
//!
//! Everything GDScript can do to this node is a getter, plus the four verbs the
//! scene layer legitimately owns: deploy, restart, pause, unpause. There is no
//! setter for game state and no way to ask for something the player could not
//! do with the controls, so a HUD bug cannot become a gameplay bug.

use godot::classes::{
    Camera3D, DirectionalLight3D, DisplayServer, Environment, INode3D, Input, InputEvent,
    InputEventMouseButton, InputEventMouseMotion, Node3D, ProceduralSkyMaterial, Sky,
    WorldEnvironment,
};
use godot::prelude::*;

use ashframe_sim::camera::Camera;
use ashframe_sim::config::{blade as blade_cfg, player as player_cfg, sim as sim_cfg};
use ashframe_sim::enemies::Enemy;
use ashframe_sim::sim::{ControlState, Simulation};
use ashframe_sim::types::{BladePhase, Hooks, MissionPhase, SimEvent};

use crate::arena_view::ArenaView;
use crate::audio::{Audio, Sound};
use crate::effects::Effects;
use crate::input::{self, keys, LookState, Wheel};
use crate::palette::to_godot;
use crate::rig::{Frame, MechPose, MechRig};

/// What the game is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The briefing. The world is built and lit; nothing is happening in it.
    Ready,
    Playing,
    Paused,
    /// Victory or defeat, with the results up.
    Results,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Playing => "playing",
            Self::Paused => "paused",
            Self::Results => "results",
        }
    }
}

/// The event sink: turns simulation events into things on screen.
struct Sink<'a> {
    effects: &'a mut Effects,
    audio: &'a mut Audio,
    /// Where the player is, for the audio range cull.
    listener: ashframe_sim::types::Vec3,
    /// Events the frame's own reactions read, cleared each step.
    log: &'a mut Vec<SimEvent>,
}

impl Hooks for Sink<'_> {
    fn emit(&mut self, event: &SimEvent) {
        match event {
            SimEvent::Fire {
                origin, direction, ..
            } => {
                self.effects.tracer(*origin, *direction);
                self.effects.muzzle_flash(*origin, 1.0);
                self.audio
                    .play_at(Sound::Autocannon, *origin, self.listener, 1.0);
            }
            SimEvent::EnemyFire { at } => {
                self.effects.muzzle_flash(*at, 0.7);
                self.audio
                    .play_at(Sound::EnemyFire, *at, self.listener, 1.0);
            }
            SimEvent::MissileLaunch { origin, .. } => {
                self.effects.muzzle_flash(*origin, 1.4);
                self.audio
                    .play_at(Sound::MissileLaunch, *origin, self.listener, 1.0);
            }

            SimEvent::BladeHit { at, .. } => {
                self.audio.play_at(Sound::BladeHit, *at, self.listener, 1.0);
            }
            // A hard surface rings and a soft one thuds, and the simulation
            // already tags every prop with what it is made of.
            SimEvent::Impact { at, surface, .. } => {
                self.effects.impact(*at);
                let hard = matches!(
                    surface.as_str(),
                    "steel" | "plating" | "armor" | "player" | "tank" | "container" | "glass"
                );
                self.audio.play_at(
                    if hard {
                        Sound::ImpactHard
                    } else {
                        Sound::ImpactSoft
                    },
                    *at,
                    self.listener,
                    1.0,
                );
            }
            SimEvent::Explosion { at, radius } => {
                self.effects.explosion(*at, *radius);
                self.audio
                    .play_at(Sound::Explosion, *at, self.listener, 1.0);
            }
            SimEvent::Telegraph {
                at,
                radius,
                duration,
            } => {
                self.effects.telegraph(*at, *radius, *duration);
                self.audio
                    .play_at(Sound::Telegraph, *at, self.listener, 1.0);
            }
            SimEvent::Hit { at, .. } => self.effects.impact(*at),
            // The blade is heard when it is swung rather than when it connects;
            // the windup is the phase the player is committing to.
            SimEvent::Blade {
                phase: BladePhase::Windup,
            } => self
                .audio
                .play_at(Sound::BladeSwing, self.listener, self.listener, 1.0),
            SimEvent::Jump { .. } => {
                self.audio
                    .play_at(Sound::Boost, self.listener, self.listener, 0.8)
            }
            SimEvent::QuickBoost { .. } => {
                self.audio
                    .play_at(Sound::Boost, self.listener, self.listener, 1.0)
            }
            SimEvent::AssaultStart => {
                self.audio
                    .play_at(Sound::Boost, self.listener, self.listener, 0.7)
            }
            SimEvent::Land { speed, .. } => {
                let gain = (speed / 40.0).clamp(0.2, 1.0);
                self.audio
                    .play_at(Sound::Landing, self.listener, self.listener, gain);
            }
            SimEvent::LockAcquired { .. } => self.audio.play_ui(Sound::LockAcquired),
            SimEvent::LockLost { .. } => self.audio.play_ui(Sound::LockLost),
            SimEvent::RepairStart { .. } => self.audio.play_ui(Sound::Repair),
            SimEvent::EnergyEmpty => self.audio.energy_warning(),
            SimEvent::PlayerStagger => {
                self.audio
                    .play_at(Sound::PlayerStagger, self.listener, self.listener, 1.0)
            }
            SimEvent::PlayerDamage { at, .. } => self.audio.damage(*at, self.listener),
            SimEvent::Destroy { at, .. } => {
                self.audio
                    .play_at(Sound::Explosion, *at, self.listener, 1.0)
            }
            SimEvent::MissionComplete { .. } => self.audio.play_ui(Sound::MissionComplete),
            SimEvent::MissionFailed => self.audio.play_ui(Sound::MissionFailed),
            _ => {}
        }
        // Bounded, so a long fight cannot grow this through the HUD channel.
        if self.log.len() < 512 {
            self.log.push(event.clone());
        }
    }
}

/// The game.
#[derive(GodotClass)]
#[class(base=Node3D)]
pub struct AshframeGame {
    base: Base<Node3D>,

    sim: Simulation,
    camera: Camera,
    phase: Phase,

    arena: Option<ArenaView>,
    effects: Option<Effects>,
    audio: Option<Audio>,
    /// Index zero is the player; the rest are hostiles, paired with roster
    /// entries by position.
    rigs: Vec<MechRig>,
    enemy_rigs: Vec<usize>,
    camera_node: Option<Gd<Camera3D>>,

    look: LookState,
    wheel: Wheel,
    /// Which one-shot bindings were down at the previous step.
    prev: input::Edges,
    pending_jump: bool,
    pending_boost: bool,
    pending_missile: bool,
    pending_blade: bool,
    pending_reload: bool,
    pending_repair: bool,
    pending_lock: bool,

    events: Vec<SimEvent>,
    accumulator: f32,
    objective: GString,
    hint: GString,
    hint_left: f32,
    idle_orbit: f32,
    /// Walk-cycle phase, derived from speed and time. Presentation only: it
    /// never feeds back into movement, which is why it lives here rather than in
    /// the simulation.
    gait: f32,
    /// Nothing is stepped while the window has no focus, so alt-tabbing does
    /// not drop the player into a firefight they cannot see.
    focus: bool,
    /// Whether the window has ever held focus. See `physics_process`.
    ever_focused: bool,
    /// Whether to print a state trace every second, for the capture harness.
    trace: bool,
    /// Whether to hold a fixed three-quarter view of the player.
    debug_cam: bool,
    /// Whether to drive a canned demonstration instead of reading the keyboard.
    autoplay: bool,
    /// Whether the demonstration should also win the mission, so the results
    /// screen can be captured.
    demo_win: bool,

    frames: u64,
    shot_path: Option<String>,
    shot_frame: u64,
    shot_quit: bool,
}

#[godot_api]
impl INode3D for AshframeGame {
    fn init(base: Base<Node3D>) -> Self {
        Self {
            base,
            sim: Simulation::new(Default::default()),
            camera: Camera::new(),
            phase: Phase::Ready,
            arena: None,
            effects: None,
            audio: None,
            rigs: Vec::new(),
            enemy_rigs: Vec::new(),
            camera_node: None,
            look: LookState::default(),
            wheel: Wheel::default(),
            prev: input::Edges::default(),
            pending_jump: false,
            pending_boost: false,
            pending_missile: false,
            pending_blade: false,
            pending_reload: false,
            pending_repair: false,
            pending_lock: false,
            events: Vec::new(),
            accumulator: 0.0,
            objective: GString::from(""),
            hint: GString::from(""),
            hint_left: 0.0,
            idle_orbit: 0.0,
            gait: 0.0,
            focus: true,
            ever_focused: false,
            trace: false,
            debug_cam: false,
            autoplay: false,
            demo_win: false,
            frames: 0,
            shot_path: None,
            shot_frame: 0,
            shot_quit: false,
        }
    }

    fn ready(&mut self) {
        self.build_world();
        self.read_command_line();
        self.objective = GString::from(self.sim.mission.objective());
        self.place_camera_idle(0.0);
    }

    fn unhandled_input(&mut self, event: Gd<InputEvent>) {
        if let Ok(motion) = event.clone().try_cast::<InputEventMouseMotion>() {
            if Input::singleton().get_mouse_mode() == godot::classes::input::MouseMode::CAPTURED {
                let rel = motion.get_relative();
                self.look.add(rel.x, rel.y);
            }
            return;
        }
        if let Ok(button) = event.clone().try_cast::<InputEventMouseButton>() {
            if button.is_pressed() {
                let index = button.get_button_index();
                if index == godot::global::MouseButton::WHEEL_UP {
                    self.wheel.add(1.0);
                } else if index == godot::global::MouseButton::WHEEL_DOWN {
                    self.wheel.add(-1.0);
                }
            }
            return;
        }

        // Menu keys are handled here rather than polled, because "the player
        // pressed escape" is an event and polling it would also fire while the
        // key is held down.
        let key = event
            .try_cast::<godot::classes::InputEventKey>()
            .ok()
            .filter(|k| k.is_pressed() && !k.is_echo())
            .map(|k| k.get_physical_keycode());
        match key {
            Some(k) if k == keys::PAUSE => match self.phase {
                Phase::Playing | Phase::Paused => self.toggle_pause(),
                _ => {}
            },
            Some(k) if k == keys::CONFIRM => match self.phase {
                Phase::Ready => self.deploy(),
                Phase::Results => self.restart(),
                _ => {}
            },
            Some(k)
                if k == keys::RELOAD
                    && (self.phase == Phase::Results || self.phase == Phase::Paused) =>
            {
                self.restart()
            }
            _ => {}
        }
    }

    fn physics_process(&mut self, delta: f64) {
        let dt = delta as f32;

        // Losing the window pauses the fight. Alt-tabbing into a firefight the
        // player cannot see is the one way this game can kill somebody who is
        // not at the keyboard, and it is a one-line fix.
        //
        // Polled rather than received: gdext exposes no notification virtual on
        // a node, and the display server knows the answer anyway.
        // Only a *lost* focus pauses. A game that has never been focused — one
        // launched by a harness, or started behind another window — would
        // otherwise pause itself before the player had seen anything.
        let focused = DisplayServer::singleton().window_is_focused();
        if focused {
            self.ever_focused = true;
        }
        if self.ever_focused && !focused && self.focus && self.phase == Phase::Playing {
            self.phase = Phase::Paused;
            self.capture_mouse(false);
        }
        self.focus = focused;

        if self.phase == Phase::Playing && (self.focus || !self.ever_focused) {
            self.accumulator += dt.min(sim_cfg::MAX_CATCH_UP);
            let mut steps = 0;
            while self.accumulator >= sim_cfg::DT && steps < sim_cfg::MAX_STEPS {
                self.accumulator -= sim_cfg::DT;
                steps += 1;
                self.tick(sim_cfg::DT);
            }
        } else {
            self.accumulator = 0.0;
            if self.phase == Phase::Ready {
                self.place_camera_idle(dt);
            }
            if let Some(audio) = self.audio.as_mut() {
                audio.hush();
            }
        }
        self.present(dt);
    }

    fn process(&mut self, delta: f64) {
        let dt = delta as f32;
        if let Some(effects) = self.effects.as_mut() {
            effects.step(dt);
        }
        if let Some(audio) = self.audio.as_mut() {
            audio.step(dt);
        }
        if self.hint_left > 0.0 {
            self.hint_left -= dt;
            if self.hint_left <= 0.0 {
                self.hint = GString::from("");
            }
        }
        self.frames += 1;
        // A trace, for the capture harness. A screenshot says what the frame
        // looked like; this says what the simulation was doing at the time,
        // which is the difference between "the camera is wrong" and "the mech
        // is somewhere unexpected".
        if self.trace && self.frames.is_multiple_of(60) {
            let p = &self.sim.player;
            godot::global::godot_print!(
                "f{} pos=({:.2},{:.2},{:.2}) vel=({:.2},{:.2},{:.2}) lean=({:.3},{:.3}) grounded={} speed={:.2}",
                self.frames, p.pos.x, p.pos.y, p.pos.z,
                p.vel.x, p.vel.y, p.vel.z, p.lean_x, p.lean_z,
                p.grounded, p.speed()
            );
        }
        if let Some(path) = self.shot_path.clone() {
            if self.frames >= self.shot_frame {
                self.write_screenshot(&path);
                self.shot_path = None;
                if self.shot_quit {
                    self.base().get_tree().quit();
                }
            }
        }
    }
}

#[godot_api]
impl AshframeGame {
    // ── the verbs the scene layer owns ──────────────────────────────────────

    /// Start the operation.
    #[func]
    fn deploy(&mut self) {
        if self.phase != Phase::Ready && self.phase != Phase::Results {
            return;
        }
        self.sim.reset(Default::default());
        self.camera = Camera::new();
        self.rebuild_rigs();
        self.events.clear();
        self.accumulator = 0.0;
        self.gait = 0.0;
        self.phase = Phase::Playing;
        self.objective = GString::from(self.sim.mission.objective());
        self.hint = GString::from("");
        self.hint_left = 0.0;
        self.capture_mouse(true);
        if let Some(audio) = self.audio.as_mut() {
            audio.play_ui(Sound::UiConfirm);
        }
    }

    /// Back to the briefing.
    #[func]
    fn restart(&mut self) {
        self.phase = Phase::Ready;
        self.sim.reset(Default::default());
        self.camera = Camera::new();
        self.rebuild_rigs();
        self.events.clear();
        self.gait = 0.0;
        self.objective = GString::from(self.sim.mission.objective());
        self.hint = GString::from("");
        self.hint_left = 0.0;
        self.capture_mouse(false);
    }

    #[func]
    fn toggle_pause(&mut self) {
        match self.phase {
            Phase::Playing => {
                self.phase = Phase::Paused;
                self.capture_mouse(false);
                if let Some(audio) = self.audio.as_mut() {
                    audio.play_ui(Sound::UiClick);
                }
            }
            Phase::Paused => {
                self.phase = Phase::Playing;
                self.capture_mouse(true);
                if let Some(audio) = self.audio.as_mut() {
                    audio.play_ui(Sound::UiClick);
                }
            }
            _ => {}
        }
    }

    // ── what the HUD reads ──────────────────────────────────────────────────

    #[func]
    fn phase_label(&self) -> GString {
        GString::from(self.phase.label())
    }

    #[func]
    fn objective(&self) -> GString {
        self.objective.clone()
    }

    #[func]
    fn hint(&self) -> GString {
        self.hint.clone()
    }

    #[func]
    fn mission_phase(&self) -> GString {
        GString::from(self.sim.mission.phase.label())
    }

    #[func]
    fn health(&self) -> f32 {
        self.sim.player.health
    }

    #[func]
    fn health_max(&self) -> f32 {
        self.sim.player.max_health
    }

    #[func]
    fn stability(&self) -> f32 {
        self.sim.player.stability
    }

    #[func]
    fn stability_max(&self) -> f32 {
        self.sim.player.max_stability
    }

    #[func]
    fn energy(&self) -> f32 {
        self.sim.player.energy
    }

    #[func]
    fn energy_max(&self) -> f32 {
        self.sim.player.energy_max
    }

    #[func]
    fn ammo(&self) -> i32 {
        self.sim.player.ammo as i32
    }

    #[func]
    fn magazine(&self) -> i32 {
        ashframe_sim::config::rifle::MAGAZINE as i32
    }

    #[func]
    fn repairs(&self) -> i32 {
        self.sim.player.repairs as i32
    }

    #[func]
    fn repairing(&self) -> bool {
        self.sim.player.repair_timer > 0.0
    }

    #[func]
    fn reloading(&self) -> bool {
        self.sim.player.reload_timer > 0.0
    }

    #[func]
    fn reload_progress(&self) -> f32 {
        let total = ashframe_sim::config::rifle::RELOAD_TIME;
        if total <= 0.0 {
            return 0.0;
        }
        (1.0 - self.sim.player.reload_timer / total).clamp(0.0, 1.0)
    }

    #[func]
    fn blade_phase(&self) -> GString {
        GString::from(self.sim.player.blade_phase.label())
    }

    #[func]
    fn blade_ready(&self) -> bool {
        self.sim.player.blade_phase == BladePhase::Idle && self.sim.player.blade_cooldown <= 0.0
    }

    #[func]
    fn missiles_ready(&self) -> bool {
        self.sim.player.missile_cooldown <= 0.0 && self.sim.targeting.locked_id().is_some()
    }

    #[func]
    fn missiles_locked(&self) -> bool {
        self.sim.targeting.locked_id().is_some()
    }

    #[func]
    fn speed(&self) -> f32 {
        self.sim.player.speed()
    }

    #[func]
    fn altitude(&self) -> f32 {
        self.sim.player.pos.y
    }

    #[func]
    fn grounded(&self) -> bool {
        self.sim.player.grounded
    }

    /// Whether the assault boost is lit. Named for what the HUD shows rather
    /// than for the field, because the boost is what the player pressed and the
    /// mechanics of how it is implemented are not the HUD's business.
    #[func]
    fn assault_boosting(&self) -> bool {
        self.sim.player.assaulting
    }

    #[func]
    fn staggered(&self) -> bool {
        self.sim.player.staggered()
    }

    #[func]
    fn enemies_left(&self) -> i32 {
        self.sim.roster.active_count() as i32
    }

    #[func]
    fn boss_present(&self) -> bool {
        self.sim.boss().is_some()
    }

    #[func]
    fn boss_health(&self) -> f32 {
        self.sim.boss().map(|b| b.health).unwrap_or(0.0)
    }

    #[func]
    fn boss_health_max(&self) -> f32 {
        self.sim
            .boss()
            .map(|b| b.max_health)
            .unwrap_or(ashframe_sim::config::boss::HEALTH)
    }

    /// Name of the locked target, or empty.
    #[func]
    fn lock_name(&self) -> GString {
        match self.locked() {
            Some(enemy) => GString::from(enemy.name),
            None => GString::from(""),
        }
    }

    #[func]
    fn lock_health(&self) -> f32 {
        self.locked().map(|e| e.health_fraction()).unwrap_or(0.0)
    }

    #[func]
    fn lock_distance(&self) -> f32 {
        match self.locked() {
            Some(enemy) => (enemy.pos - self.sim.player.pos).length(),
            None => 0.0,
        }
    }

    /// Where the locked target is, for the HUD to project onto the screen.
    ///
    /// The origin means "nothing locked". The arena is centred there, so a zero
    /// that stood for a real position would draw a lock bracket on the floor.
    #[func]
    fn lock_world_position(&self) -> Vector3 {
        match self.locked() {
            Some(enemy) => to_godot(ashframe_sim::types::Vec3::new(
                enemy.pos.x,
                enemy.pos.y + enemy.height * 0.62,
                enemy.pos.z,
            )),
            None => Vector3::ZERO,
        }
    }

    #[func]
    fn kills(&self) -> i32 {
        self.sim.mission.stats.kills as i32
    }

    #[func]
    fn score(&self) -> i32 {
        self.sim.mission.stats.score as i32
    }

    #[func]
    fn elapsed(&self) -> f32 {
        self.sim.mission.stats.elapsed
    }

    #[func]
    fn damage_taken(&self) -> f32 {
        self.sim.mission.stats.damage_taken
    }

    #[func]
    fn shots_fired(&self) -> i32 {
        self.sim.mission.stats.shots_fired as i32
    }

    #[func]
    fn repairs_used(&self) -> i32 {
        self.sim.mission.stats.repairs_used as i32
    }

    #[func]
    fn won(&self) -> bool {
        self.sim.mission.phase == MissionPhase::Victory
    }

    #[func]
    fn lost(&self) -> bool {
        self.sim.mission.phase == MissionPhase::Defeat
    }

    #[func]
    fn accuracy(&self) -> f32 {
        let fired = self.sim.mission.stats.shots_fired;
        if fired == 0 {
            return 0.0;
        }
        // Kills over rounds is not accuracy, but it is the only honest ratio
        // available without tracking every impact, and it is labelled as a
        // ratio rather than as a hit rate.
        (self.sim.mission.stats.kills as f32 / fired as f32 * 100.0).clamp(0.0, 100.0)
    }

    #[func]
    fn frame_count(&self) -> i32 {
        self.frames as i32
    }

    #[func]
    fn effect_count(&self) -> i32 {
        self.effects.as_ref().map(|e| e.live_count()).unwrap_or(0) as i32
    }

    #[func]
    fn prop_count(&self) -> i32 {
        self.arena.as_ref().map(|a| a.solid_count()).unwrap_or(0) as i32
    }

    #[func]
    fn control_help(&self) -> GString {
        GString::from(
            "WASD move   SHIFT boost   SPACE jump   CTRL assault\n\
             LMB autocannon   RMB missile volley   V blade   R reload   F repair\n\
             Q lock on   WHEEL cycle targets   ESC pause",
        )
    }
}

impl AshframeGame {
    fn capture_mouse(&self, captured: bool) {
        let mode = if captured {
            godot::classes::input::MouseMode::CAPTURED
        } else {
            godot::classes::input::MouseMode::VISIBLE
        };
        Input::singleton().set_mouse_mode(mode);
    }

    fn locked(&self) -> Option<&Enemy> {
        let id = self.sim.targeting.locked_id()?;
        self.sim.roster.by_id(id).filter(|e| e.alive)
    }

    /// One fixed simulation step, and everything that follows from it.
    fn tick(&mut self, dt: f32) {
        self.read_bindings();
        let control = self.control_state();

        // The camera runs first, always. Aiming reads the view it writes, so a
        // camera stepped afterwards would aim the player at where they were
        // looking a frame ago.
        self.step_camera(dt);

        self.events.clear();
        {
            let listener = self.sim.player.pos;
            let effects = self.effects.as_mut().expect("effects are built in ready");
            let audio = self.audio.as_mut().expect("audio is built in ready");
            let mut sink = Sink {
                effects,
                audio,
                listener,
                log: &mut self.events,
            };
            self.sim.step(dt, &control, &mut sink);
        }
        self.demo_win_tick();
        self.react_to_events();

        if let Some(audio) = self.audio.as_mut() {
            audio.drive(
                self.sim.player.speed(),
                thrust_level(&self.sim.player),
                self.sim.player.assaulting,
                self.sim.player.grounded,
            );
        }

        // The walk cycle advances with distance travelled rather than with
        // time, so a mech standing still does not march on the spot and a fast
        // one does not glide.
        let speed = self.sim.player.speed();
        let rate = if self.sim.player.grounded {
            speed * 0.22
        } else {
            0.0
        };
        self.gait = (self.gait + rate * dt) % (std::f32::consts::PI * 2.0);

        if self.sim.mission.finished() {
            self.phase = Phase::Results;
            self.capture_mouse(false);
        }
        self.sync_hint();
    }

    /// Edge-detect the one-shot bindings.
    fn read_bindings(&mut self) {
        let now = input::edges();
        self.pending_jump = now.jump && !self.prev.jump;
        self.pending_boost = now.boost && !self.prev.boost;
        self.pending_missile = now.missile && !self.prev.missile;
        self.pending_blade = now.blade && !self.prev.blade;
        self.pending_reload = now.reload && !self.prev.reload;
        self.pending_repair = now.repair && !self.prev.repair;
        self.pending_lock = now.lock && !self.prev.lock;
        self.prev = now;
    }

    fn control_state(&mut self) -> ControlState {
        let mut c = input::sample();
        c.jump_pressed = self.pending_jump;
        c.quick_boost_pressed = self.pending_boost;
        c.missile_pressed = self.pending_missile;
        c.blade_pressed = self.pending_blade;
        c.reload_pressed = self.pending_reload;
        c.repair_pressed = self.pending_repair;
        c.toggle_lock_pressed = self.pending_lock;
        c.cycle_dir = self.wheel.take();
        if self.autoplay {
            self.scripted(&mut c);
        }
        c
    }

    /// A canned demonstration, for the screenshot harness.
    ///
    /// It exists so a capture shows the game doing something. A still frame of a
    /// mech standing in a yard proves the renderer starts; it does not prove a
    /// shot connects, that an enemy reacts, or that a lock bracket lands on a
    /// target. This drives the same control state a person would, through the
    /// same code path, so what the harness captures is what a player would see.
    fn scripted(&mut self, c: &mut ControlState) {
        let t = self.sim.time;
        // Face the yard, then sweep, which is what a player does while looking
        // for the screen.
        self.camera.set_look(
            std::f32::consts::PI + (t * 0.6).sin() * 0.5,
            -0.05 + (t * 0.4).sin() * 0.06,
        );

        if t < 3.6 {
            return;
        }
        c.move_z = if t < 7.0 { 1.0 } else { 0.0 };
        c.move_x = if t < 7.0 { 0.0 } else { (t * 0.8).sin() };
        c.move_mag = (c.move_x * c.move_x + c.move_z * c.move_z).sqrt().min(1.0);
        c.fire_primary = t > 6.0;
        c.toggle_lock_pressed = (t - 5.6).abs() < 0.01;
        c.missile_pressed = (t - 6.2).abs() < 0.01;
        c.quick_boost_pressed = (t - 4.2).abs() < 0.01;
        c.assault_held = (12.0..14.0).contains(&t);
        c.jump_pressed = (t - 9.6).abs() < 0.01;
        c.jump_held = (9.6..11.4).contains(&t);
    }

    /// Let the demonstration finish the mission.
    ///
    /// The script above is a bad player: it flies the yard firing at whatever
    /// it happens to be pointing at, and by twenty seconds it is losing. That
    /// is fine for a combat capture and useless for checking the results
    /// screen, which is the one screen nobody sees until the end.
    ///
    /// So after twenty seconds the demonstration starts hitting, one unit per
    /// step, so the fight ends the way a fight ends rather than in a single
    /// frame of everything dying at once. The damage goes through the
    /// simulation, which is what registers the kill: a harness that killed
    /// units by writing to them directly would reach the results screen with
    /// nothing on it, and would be evidence of nothing.
    fn demo_win_tick(&mut self) {
        if !self.demo_win || self.sim.time <= 20.0 {
            return;
        }
        let Some(index) = self.sim.roster.list.iter().position(|enemy| enemy.alive) else {
            return;
        };
        let lethal = self.sim.roster.list[index].health + 1.0;
        let listener = self.sim.player.pos;
        let effects = self.effects.as_mut().expect("effects are built in ready");
        let audio = self.audio.as_mut().expect("audio is built in ready");
        let mut sink = Sink {
            effects,
            audio,
            listener,
            log: &mut self.events,
        };
        self.sim.damage_enemy(index, lethal, &mut sink);
    }

    /// A fixed three-quarter view of the player, for looking at the rig itself
    /// rather than at what the game camera happens to frame.
    fn debug_camera(&mut self) {
        let p = self.sim.player.pos;
        let at = to_godot(ashframe_sim::types::Vec3::new(p.x, p.y + 3.0, p.z));
        if let Some(node) = self.camera_node.as_mut() {
            node.set_position(at + Vector3::new(6.0, 3.0, 9.0));
            node.look_at(at);
            node.set_fov(50.0);
        }
    }

    fn step_camera(&mut self, dt: f32) {
        if self.debug_cam {
            self.debug_camera();
            return;
        }
        let (dx, dy) = self.look.take();
        if !input::free_look() {
            self.camera.look(dx, dy);
        }

        let assist = self.sim.camera_assist(dt);
        let target = self.sim.camera_target();
        let locked = self.sim.targeting.locked_id().is_some();
        let pose = self
            .camera
            .update(dt, target, &self.sim.collision, locked, assist);
        self.camera.write_view(&pose, &mut self.sim.view);

        if let Some(node) = self.camera_node.as_mut() {
            node.set_position(to_godot(pose.position));
            node.look_at(to_godot(pose.look_at));
            if (node.get_fov() - pose.fov).abs() > 0.05 {
                node.set_fov(pose.fov);
            }
        }
    }

    /// The briefing camera: a slow orbit of the yard from above.
    fn place_camera_idle(&mut self, dt: f32) {
        self.idle_orbit += dt * 0.09;
        let radius = 96.0;
        let at = Vector3::new(
            self.idle_orbit.sin() * radius,
            36.0,
            self.idle_orbit.cos() * radius,
        );
        if let Some(node) = self.camera_node.as_mut() {
            node.set_position(at);
            node.look_at(Vector3::new(0.0, 6.0, 0.0));
        }
    }

    /// Turn this frame's events into anything that outlives the frame.
    fn react_to_events(&mut self) {
        let player = to_godot(self.sim.player.pos);
        let mut shake = 0.0f32;
        for event in &self.events {
            match event {
                SimEvent::Objective { text } => self.objective = GString::from(text.as_str()),
                SimEvent::PlayerStagger => shake += 0.6,
                SimEvent::PlayerDamage { amount, .. } => shake += 0.18 + amount * 0.003,
                SimEvent::Fire { .. } => shake += 0.045,
                SimEvent::BladeHit { .. } => shake += 0.2,
                SimEvent::Land { speed, .. } => {
                    shake += (speed / 70.0).clamp(0.0, 0.45);
                }
                SimEvent::Explosion { at, .. } => {
                    let near = (to_godot(*at) - player).length();
                    shake += (1.0 - near / 45.0).clamp(0.0, 1.0) * 0.8;
                }
                _ => {}
            }
        }
        if shake > 0.0 {
            self.camera.add_shake(shake.min(1.2));
        }
    }

    /// Raise a one-time hint the first time its condition holds.
    fn sync_hint(&mut self) {
        if self.hint_left > 0.0 {
            return;
        }
        let wanted = if self.sim.time < 7.0 && self.sim.player.speed() < 1.5 {
            Some(("move", "W A S D to move.  SHIFT is a quick boost."))
        } else if self.sim.player.energy < 22.0 && self.sim.player.grounded {
            Some((
                "energy",
                "Energy runs the boosts and the flight. Land and it comes back.",
            ))
        } else if self.sim.roster.active_count() > 0 && self.sim.mission.stats.shots_fired == 0 {
            Some((
                "fire",
                "LEFT MOUSE for the autocannon.  Q locks a target; RMB launches.",
            ))
        } else {
            None
        };
        if let Some((key, text)) = wanted {
            if self.sim.mission.claim_hint(key) {
                self.hint = GString::from(text);
                self.hint_left = 5.5;
            }
        }
    }

    /// Push simulation state into the scene tree.
    fn present(&mut self, dt: f32) {
        let pose = self.player_pose();
        if let Some(rig) = self.rigs.first_mut() {
            rig.set_visible(self.sim.player.alive);
            rig.pose(&pose);
        }

        self.sync_rigs();
        let count = self.sim.roster.list.len();
        for i in 0..count {
            let enemy = &self.sim.roster.list[i];
            let pose = enemy_pose(enemy);
            let alive = enemy.alive;
            if let Some(&rig_index) = self.enemy_rigs.get(i) {
                if let Some(rig) = self.rigs.get_mut(rig_index) {
                    rig.set_visible(alive);
                    if alive {
                        rig.pose(&pose);
                    }
                }
            }
        }
        let _ = dt;
    }

    /// Give every roster entry a rig, building one the first time it is needed.
    ///
    /// The roster only ever grows — a dead unit is reused rather than removed —
    /// so pairing by index is stable for the life of a mission.
    fn sync_rigs(&mut self) {
        while self.enemy_rigs.len() < self.sim.roster.list.len() {
            let index = self.enemy_rigs.len();
            let kind = self.sim.roster.list[index].kind;
            let mut parent = self.base().clone().upcast::<Node3D>();
            let rig_index = self.rigs.len();
            self.rigs
                .push(MechRig::new(&mut parent, Frame::Hostile(kind)));
            self.enemy_rigs.push(rig_index);
        }
    }

    fn player_pose(&self) -> MechPose {
        let p = &self.sim.player;
        MechPose {
            at: p.pos,
            yaw: p.body_yaw,
            torso_yaw: p.yaw - p.body_yaw,
            lean_x: p.lean_x,
            lean_z: p.lean_z,
            gait: self.gait,
            airborne: !p.grounded,
            thrust: thrust_level(p),
            stagger: (p.stagger_timer / player_cfg::STAGGER_DURATION).clamp(0.0, 1.0),
            recoil: p.recoil,
            blade: blade_extension(p.blade_phase, p.blade_timer_left()),
            hit_flash: 0.0,
        }
    }

    fn rebuild_rigs(&mut self) {
        for mut rig in self.rigs.drain(..) {
            rig.root.queue_free();
        }
        self.enemy_rigs.clear();
        let mut parent = self.base().clone().upcast::<Node3D>();
        self.rigs.push(MechRig::new(&mut parent, Frame::Player));
    }

    fn build_world(&mut self) {
        let mut this = self.base().clone();

        // ── sky and air ─────────────────────────────────────────────────────
        let mut sky_material = ProceduralSkyMaterial::new_gd();
        sky_material.set_sky_top_color(Color::from_rgba(0.15, 0.26, 0.44, 1.0));
        sky_material.set_sky_horizon_color(Color::from_rgba(0.62, 0.60, 0.53, 1.0));
        sky_material.set_ground_bottom_color(Color::from_rgba(0.09, 0.09, 0.09, 1.0));
        sky_material.set_ground_horizon_color(Color::from_rgba(0.34, 0.32, 0.28, 1.0));
        sky_material.set_sun_angle_max(18.0);
        sky_material.set_sun_curve(0.18);
        sky_material.set_sky_energy_multiplier(0.9);
        let mut sky = Sky::new_gd();
        sky.set_material(&sky_material);

        let mut env = Environment::new_gd();
        env.set_background(godot::classes::environment::BgMode::SKY);
        env.set_sky(Some(&sky));
        env.set_ambient_source(godot::classes::environment::AmbientSource::SKY);
        // Sky fill is what keeps the shadowed faces from going black, but it is
        // also what flattens a scene into one tone if it is turned up. A third
        // of full is enough to read shape without erasing the sun.
        env.set_ambient_light_energy(0.30);
        env.set_ambient_light_sky_contribution(1.0);
        env.set_tonemapper(godot::classes::environment::ToneMapper::ACES);
        // White point at 1.0 rather than the default 6: the six-stop version is
        // for scenes lit far brighter than this one, and it lifted every
        // midtone in the yard until the concrete read as fog.
        env.set_tonemap_white(1.0);
        env.set_tonemap_exposure(0.92);
        // Dust in the air is what gives the yard depth. Without it the far
        // structures are as crisp as the near ones and the scale reads wrong.
        env.set_fog_enabled(true);
        env.set_fog_light_color(Color::from_rgba(0.52, 0.50, 0.45, 1.0));
        env.set_fog_density(0.0016);
        env.set_fog_sky_affect(0.15);
        env.set_fog_aerial_perspective(0.4);
        // Ambient occlusion, turned well down. At a large radius on a flat
        // floor the kernel samples along the surface and finds the surface
        // itself, which paints a black wash over the ground in front of the
        // camera. A small radius with a light influence keeps the creases
        // between crates and drops the wash.
        env.set_ssao_enabled(true);
        env.set_ssao_radius(0.85);
        env.set_ssao_intensity(0.7);
        env.set("ssao_power", &1.8f32.to_variant());
        env.set("ssao_detail", &0.6f32.to_variant());
        env.set("ssao_light_affect", &0.12f32.to_variant());
        env.set("ssao_ao_channel_affect", &0.0f32.to_variant());
        env.set("ssao_sharpness", &0.98f32.to_variant());
        env.set_glow_enabled(true);
        env.set_glow_intensity(0.5);
        env.set_glow_bloom(0.1);
        env.set_adjustment_enabled(true);
        env.set_adjustment_contrast(1.14);
        env.set_adjustment_saturation(1.12);

        let mut world_env = WorldEnvironment::new_alloc();
        world_env.set_environment(&env);
        this.add_child(&world_env);

        // ── sun ─────────────────────────────────────────────────────────────
        let mut sun = DirectionalLight3D::new_alloc();
        sun.set_rotation(Vector3::new(-0.95, 0.65, 0.0));
        sun.set("light_energy", &1.05f32.to_variant());
        sun.set(
            "light_color",
            &Color::from_rgba(1.0, 0.93, 0.80, 1.0).to_variant(),
        );
        sun.set("shadow_enabled", &true.to_variant());
        sun.set("directional_shadow_max_distance", &240.0f32.to_variant());
        this.add_child(&sun);

        let mut fill = DirectionalLight3D::new_alloc();
        fill.set_rotation(Vector3::new(-0.35, -2.2, 0.0));
        fill.set("light_energy", &0.22f32.to_variant());
        fill.set(
            "light_color",
            &Color::from_rgba(0.48, 0.62, 0.86, 1.0).to_variant(),
        );
        fill.set("shadow_enabled", &false.to_variant());
        this.add_child(&fill);

        // ── camera ──────────────────────────────────────────────────────────
        let mut camera = Camera3D::new_alloc();
        camera.set_fov(ashframe_sim::config::camera::FOV);
        camera.set_far(900.0);
        camera.set_near(0.25);
        this.add_child(&camera);
        self.camera_node = Some(camera);

        // ── the yard ────────────────────────────────────────────────────────
        // Built from the simulation's own description of itself, so there is no
        // second copy of the arena to drift away from the collision.
        let arena = ArenaView::build(&mut this, &self.sim.arena);
        self.arena = Some(arena);
        self.effects = Some(Effects::build(&mut this));
        self.audio = Some(Audio::build(&mut this));
        self.rebuild_rigs();
    }

    fn read_command_line(&mut self) {
        let args = godot::classes::Os::singleton().get_cmdline_user_args();
        let list: Vec<String> = args.as_slice().iter().map(|s| s.to_string()).collect();
        let mut i = 0;
        while i < list.len() {
            match list[i].as_str() {
                "--shot" if i + 1 < list.len() => {
                    self.shot_path = Some(list[i + 1].clone());
                    i += 2;
                }
                "--shot-frame" if i + 1 < list.len() => {
                    self.shot_frame = list[i + 1].parse().unwrap_or(90);
                    i += 2;
                }
                "--quit-after-shot" => {
                    self.shot_quit = true;
                    i += 1;
                }
                "--play" => {
                    self.deploy();
                    i += 1;
                }
                "--debug-cam" => {
                    self.debug_cam = true;
                    i += 1;
                }
                // Each of the harness flags starts the game as well as
                // configuring it. They read as "capture the game doing
                // something", and a flag that quietly captured the briefing
                // instead is worse than one that does nothing: the picture
                // looks plausible and shows none of what it was asked for.
                //
                // `deploy` returns early unless the game is at the briefing or
                // the results, so naming both `--play` and `--autoplay` is
                // harmless.
                "--autoplay" => {
                    self.autoplay = true;
                    self.deploy();
                    i += 1;
                }
                "--demo-win" => {
                    self.autoplay = true;
                    self.demo_win = true;
                    self.deploy();
                    i += 1;
                }
                "--trace" => {
                    self.trace = true;
                    i += 1;
                }
                _ => i += 1,
            }
        }
    }

    fn write_screenshot(&mut self, path: &str) {
        let Some(viewport) = self.base().get_viewport() else {
            godot::global::godot_error!("no viewport to capture");
            return;
        };
        let Some(texture) = viewport.get_texture() else {
            godot::global::godot_error!("viewport had no texture; is rendering on?");
            return;
        };
        let Some(image) = texture.get_image() else {
            godot::global::godot_error!("viewport texture had no image; is rendering on?");
            return;
        };
        let err = image.save_png(path);
        if err != godot::global::Error::OK {
            godot::global::godot_error!("could not write screenshot: {err:?}");
        } else {
            godot::global::godot_print!("screenshot: {path}");
        }
    }
}

fn thrust_level(p: &ashframe_sim::player::Player) -> f32 {
    if p.assaulting {
        1.0
    } else if p.flying {
        0.75
    } else if p.boosting {
        0.5
    } else {
        0.0
    }
}

/// How far the blade is extended: it grows through windup, is out for the whole
/// active window, and retracts through recovery.
fn blade_extension(phase: BladePhase, timer_left: f32) -> f32 {
    match phase {
        BladePhase::Idle => 0.0,
        BladePhase::Windup => (1.0 - timer_left / blade_cfg::WINDUP).clamp(0.0, 1.0),
        BladePhase::Active => 1.0,
        BladePhase::Recovery => (timer_left / blade_cfg::RECOVERY).clamp(0.0, 1.0),
    }
}

fn enemy_pose(enemy: &Enemy) -> MechPose {
    MechPose {
        at: enemy.pos,
        yaw: enemy.yaw,
        torso_yaw: enemy.torso_yaw,
        lean_x: 0.0,
        lean_z: 0.0,
        gait: enemy.gait,
        airborne: false,
        thrust: if enemy.dashing { 0.7 } else { 0.0 },
        stagger: (enemy.stagger_timer / 1.6).clamp(0.0, 1.0),
        recoil: if enemy.burst_left > 0 { 0.5 } else { 0.0 },
        blade: 0.0,
        hit_flash: enemy.hit_flash.clamp(0.0, 1.0),
    }
}
