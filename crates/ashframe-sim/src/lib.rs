//! The Ashframe simulation.
//!
//! Renderer-free, engine-free, and headless by construction. It owns
//! transforms, velocities, timers and health, and reports what happened through
//! [`types::SimEvent`]. Rendering and audio read that state; nothing here reads
//! them back.
//!
//! That split is not an invention of this port. The original was built the same
//! way and said so: `src/game/world.ts` owns state and emits events, and
//! nothing in it touches Three.js. It is the reason the same 21 gameplay
//! assertions can run here as ran there, and the reason the renderer could be
//! replaced — Three.js for Godot — without rewriting the game.

pub mod arena;
pub mod collision;
pub mod combat;
pub mod config;
pub mod math;
pub mod types;
