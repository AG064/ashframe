//! Keyboard and mouse, resolved into the simulation's control state.
//!
//! Reading Godot's `Input` singleton directly rather than going through an
//! InputMap: an InputMap lives in `project.godot`, which means a rebindable
//! action set is a file that can silently disagree with the code reading it.
//! The keys are named once, here, and a missing action is a compile error
//! rather than a control that quietly does nothing.

use godot::classes::Input;
use godot::global::Key;
use godot::prelude::*;

use ashframe_sim::sim::ControlState;

/// Mouse motion accumulated since the last step.
///
/// Motion arrives as events but is consumed per frame, so it has to be banked.
/// Consuming it in the event handler instead would make look speed depend on
/// the polling rate.
#[derive(Debug, Default)]
pub struct LookState {
    pub dx: f32,
    pub dy: f32,
}

impl LookState {
    pub fn add(&mut self, dx: f32, dy: f32) {
        self.dx += dx;
        self.dy += dy;
    }

    /// Take everything banked since the last call.
    pub fn take(&mut self) -> (f32, f32) {
        let out = (self.dx, self.dy);
        self.dx = 0.0;
        self.dy = 0.0;
        out
    }
}

/// Fixed bindings. Named so the HUD and the briefing can print them.
pub mod keys {
    use godot::global::Key;

    pub const FORWARD: Key = Key::W;
    pub const BACK: Key = Key::S;
    pub const LEFT: Key = Key::A;
    pub const RIGHT: Key = Key::D;
    pub const JUMP: Key = Key::SPACE;
    pub const BOOST: Key = Key::SHIFT;
    pub const ASSAULT: Key = Key::CTRL;
    pub const LOCK: Key = Key::Q;
    pub const BLADE: Key = Key::V;
    pub const RELOAD: Key = Key::R;
    pub const REPAIR: Key = Key::F;
    pub const PAUSE: Key = Key::ESCAPE;
    pub const CONFIRM: Key = Key::ENTER;
    /// Held to look around without the mouse being captured, which is what the
    /// screenshot harness uses.
    pub const FREE_LOOK: Key = Key::ALT;
}

/// Read the current control state.
///
/// This reports what is *held*. The one-shot actions are filled in by the game
/// node, which is the only place that knows what happened last frame; a control
/// state that decided for itself what counted as a fresh press would have to
/// remember, and then there would be two places that remember.
pub fn sample() -> ControlState {
    let input = Input::singleton();
    let down = |key: Key| input.is_physical_key_pressed(key);

    let move_x = axis(down(keys::RIGHT), down(keys::LEFT));
    let move_z = axis(down(keys::FORWARD), down(keys::BACK));
    let magnitude = (move_x * move_x + move_z * move_z).sqrt().min(1.0);

    ControlState {
        move_x,
        move_z,
        move_mag: magnitude,
        jump_held: down(keys::JUMP),
        jump_pressed: false,
        quick_boost_pressed: false,
        assault_held: down(keys::ASSAULT),
        fire_primary: input.is_mouse_button_pressed(godot::global::MouseButton::LEFT),
        missile_pressed: false,
        blade_pressed: false,
        reload_pressed: false,
        repair_pressed: false,
        toggle_lock_pressed: false,
        cycle_dir: 0.0,
    }
}

/// Which one-shot bindings are down right now.
///
/// The game node compares consecutive samples to find the edges. Everything
/// that should happen once per press is in here, and everything that should
/// happen while held is in [`sample`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Edges {
    pub jump: bool,
    pub boost: bool,
    pub missile: bool,
    pub blade: bool,
    pub reload: bool,
    pub repair: bool,
    pub lock: bool,
}

/// Sample the one-shot bindings.
pub fn edges() -> Edges {
    let input = Input::singleton();
    Edges {
        jump: input.is_physical_key_pressed(keys::JUMP),
        boost: input.is_physical_key_pressed(keys::BOOST),
        missile: input.is_mouse_button_pressed(godot::global::MouseButton::RIGHT),
        blade: input.is_physical_key_pressed(keys::BLADE),
        reload: input.is_physical_key_pressed(keys::RELOAD),
        repair: input.is_physical_key_pressed(keys::REPAIR),
        lock: input.is_physical_key_pressed(keys::LOCK),
    }
}

/// Whether the mouse is free to move without turning the camera.
pub fn free_look() -> bool {
    Input::singleton().is_physical_key_pressed(keys::FREE_LOOK)
}

fn axis(positive: bool, negative: bool) -> f32 {
    match (positive, negative) {
        (true, false) => 1.0,
        (false, true) => -1.0,
        _ => 0.0,
    }
}

/// Mouse wheel notches, as -1, 0 or +1.
#[derive(Debug, Default)]
pub struct Wheel(pub f32);

impl Wheel {
    pub fn add(&mut self, notches: f32) {
        self.0 += notches;
    }

    /// Take the whole notches turned since the last call, keeping the rest.
    ///
    /// The remainder is kept rather than discarded because a trackpad or a
    /// free-spinning wheel reports fractions: a player scrolling slowly would
    /// otherwise turn the wheel forever and never change target.
    pub fn take(&mut self) -> f32 {
        let whole = self.0.trunc();
        self.0 -= whole;
        whole.clamp(-1.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opposite_keys_cancel_rather_than_adding() {
        assert_eq!(axis(true, true), 0.0);
        assert_eq!(axis(false, false), 0.0);
        assert_eq!(axis(true, false), 1.0);
        assert_eq!(axis(false, true), -1.0);
    }

    #[test]
    fn look_motion_banks_and_is_taken_once() {
        let mut look = LookState::default();
        look.add(3.0, -2.0);
        look.add(1.0, 1.0);
        assert_eq!(look.take(), (4.0, -1.0));
        assert_eq!(look.take(), (0.0, 0.0));
    }

    #[test]
    fn the_wheel_reports_one_notch_at_a_time() {
        let mut wheel = Wheel::default();
        wheel.add(0.4);
        assert_eq!(wheel.take(), 0.0, "a partial notch is not a notch");
        wheel.add(0.8);
        assert_eq!(wheel.take(), 1.0);
        wheel.add(-2.4);
        assert_eq!(wheel.take(), -1.0);
        assert_eq!(wheel.take(), 0.0);
    }
}
