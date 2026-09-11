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
mod audio;
mod effects;
mod game;
mod input;
mod palette;
mod rig;
mod synth;

use godot::init::{gdextension, ExtensionLibrary, InitLevel};

pub use game::AshframeGame;

struct AshframeExtension;

// The entry symbol is named rather than left at gdext's default, because this
// project loads two libraries: the engine's and this one. Both would otherwise
// export `gdext_rust_init`, and Godot starts a library by looking that symbol
// up by name — a collision there does not fail loudly, it leaves the scene's
// `AshframeGame` node as a plain `Node` and every script that talks to it
// failing on the first frame with "nonexistent function".
#[gdextension(entry_symbol = ashframe_init)]
unsafe impl ExtensionLibrary for AshframeExtension {
    fn min_level() -> InitLevel {
        InitLevel::Scene
    }
}
