//! The ashframe extension.
//!
//! Add classes here and register them the way `aurum-godot` does.

use godot::init::{gdextension, ExtensionLibrary, InitLevel};
use godot::prelude::*;

struct Extension;

#[gdextension]
unsafe impl ExtensionLibrary for Extension {
    fn min_level() -> InitLevel {
        InitLevel::Scene
    }
}
