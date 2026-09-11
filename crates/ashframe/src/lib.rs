//! The Ashframe extension.
//!
//! Ashframe is a Godot game whose rules live in Rust. This crate is the seam:
//! it registers the one node the scene tree talks to, and everything below it —
//! movement, weapons, enemy behaviour, mission flow — is `ashframe-sim`, which
//! knows nothing about Godot and runs headless in its own test suite.
//!
//! `aurum.toml` names this package, so `aurum build` compiles it and drops the
//! library where the `.gdextension` file expects it.

mod arena_view;
mod effects;
mod game;
mod input;
mod palette;
mod rig;

use godot::init::{gdextension, ExtensionLibrary, InitLevel};

pub use game::AshframeGame;

struct AshframeExtension;

#[gdextension]
unsafe impl ExtensionLibrary for AshframeExtension {
    fn min_level() -> InitLevel {
        InitLevel::Scene
    }
}
