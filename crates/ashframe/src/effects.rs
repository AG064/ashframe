//! Combat effects: muzzle flash, tracers, impacts, explosions, dust, debris,
//! telegraph warnings, and the flare that says a mech has just been hit.
//!
//! ## What this is allowed to say
//!
//! Every effect here is drawn from a simulation event and from nothing else.
//! The renderer never reads simulation state, so the only way one of these can
//! lie is by drawing more than the event it came from: a tracer travels at the
//! speed the weapon's own config gives its round, a fireball never grows past
//! the blast radius the event carries, and debris falls under the simulation's
//! gravity to the simulation's floor. Where the simulation has nothing to say —
//! the dust a walking mech's foot ought to raise between steps, the path of a
//! missile after it leaves the tube — there is no effect rather than an
//! invented one. A footstep puff would be a claim about a step the simulation
//! never reported.
//!
//! ## Pooling
//!
//! Every node, mesh, material and curve in this file is built once, when the
//! game loads, and then recycled for the rest of the match. Nothing here
//! allocates while the game is running: a shot writes a transform and a handful
//! of colour properties on resources that already exist, because allocating a
//! node per round would hand Godot's allocator and its scene tree a few
//! thousand edits a second during a firefight and the frame time would show it.
//! That constraint is why each slot owns the materials it animates rather than
//! cloning one at fire time, and why the particle emitters are fixed at their
//! maximum particle count with a per-shot `amount_ratio` instead of a per-shot
//! `amount`, which would reallocate the particle buffer on every shot.
//!
//! Every pool is bounded, and a full pool drops the new effect rather than
//! growing: a missing spark during the busiest second of a fight costs nothing,
//! and a pool that grew under load would put its frame-time cliff exactly
//! there. The one thing that is not first-come-first-served is the light pool,
//! which is small and prioritised — an explosion may take a light off a muzzle
//! flash, because the explosion is what the player is looking at.
//!
//! ## Shapes
//!
//! There is no model file and no texture: the meshes are generated as vertex
//! and colour arrays at load time. A shape's falloff, taper and fade are baked
//! into its vertex colours, which is what lets one material serve a tracer, a
//! flare and a smoke puff — and lets the arithmetic that produces them be
//! tested without an engine running.

use godot::builtin::{
    Aabb, Basis, PackedColorArray, PackedInt32Array, PackedVector3Array, Variant,
};
use godot::classes::base_material_3d::{
    BillboardMode, BlendMode, CullMode, DepthDrawMode, Flags, Transparency,
};
use godot::classes::geometry_instance_3d::ShadowCastingSetting;
use godot::classes::mesh::{ArrayType, PrimitiveType};
use godot::classes::{
    ArrayMesh, BoxMesh, Curve, CurveTexture, GpuParticles3D, Gradient, GradientTexture1D, Mesh,
    MeshInstance3D, Node3D, OmniLight3D, ParticleProcessMaterial, StandardMaterial3D,
};
use godot::prelude::*;

use ashframe_sim::config::rifle;
use ashframe_sim::config::world::{FLOOR_Y, GRAVITY};
use ashframe_sim::types::Vec3;

use crate::palette::{material, to_godot, unlit};

/// How many rounds can be in the air at once. The rifle is slow — a little over
/// seven a second — so this is generous even with a yard full of skirmishers.
const TRACER_POOL: usize = 48;
/// Muzzle flashes, which are a star rather than a round glow.
const FLASH_POOL: usize = 16;
/// Round glows: impact light, hit feedback, the middle of a fireball.
const GLOW_POOL: usize = 24;
/// Spark bursts, for anything hard being hit.
const SPARK_POOL: usize = 24;
/// Dust puffs: soft impacts, feet meeting the ground, and the ground under a
/// blast.
const DUST_POOL: usize = 24;
/// Smoke columns, for things that have just burned.
const SMOKE_POOL: usize = 12;
/// Fireballs.
const BLAST_POOL: usize = 16;
/// Flat ground waves: the ring under a blast, the crack of a heavy landing.
const RING_POOL: usize = 16;
/// Telegraph markers, in time with the simulation's own warnings.
const MARKER_POOL: usize = 12;
/// Pieces of what a blast threw.
const DEBRIS_POOL: usize = 40;
/// Dynamic lights. Deliberately few: every one of these is a light the forward
/// renderer has to consider, and more than a handful at once costs more than
/// the illumination is worth.
const LIGHT_POOL: usize = 8;

/// How long a tracer is drawn for. A round leaves the barrel at three hundred
/// metres a second, so this is a hundred and thirty metres of flight, which
/// covers most of the yard; a round that meets something is cut short by the
/// impact rather than left to fly through a wall.
const TRAIL_LIFE: f32 = 0.45;
/// How close to a tracer's line an impact has to be to be its impact.
const CUT_RADIUS: f32 = 2.6;
/// How far past a tracer's head an impact may be and still be its impact. A
/// round arrives and reports within a step or two of each other, so a little
/// slop costs a streak that ends a metre short and buys never ending one a
/// metre late.
const CUT_LEAD: f32 = 3.0;
/// Below this much of its life left, a tracer starts to fade out.
const TRAIL_FADE: f32 = 0.45;
/// Tracer brightness. Above white, so the glow pass picks a tracer up and it
/// reads as a burning round rather than as an orange stick.
const TRAIL_BOOST: f32 = 2.3;
/// The muzzle flare is brighter than the tonemapper's white, so it blooms.
const FLARE_BOOST: f32 = 2.4;
/// The middle of a fireball. Lower than the muzzle flash on purpose: pushed
/// harder than this the whole fireball clips to white and a machine burning to
/// death reads as a puff of steam.
const CORE_BOOST: f32 = 1.8;

/// Priorities for the light pool. Higher wins when every light is busy.
const LIGHT_MUZZLE: u8 = 1;
const LIGHT_HIT: u8 = 1;
const LIGHT_BLAST: u8 = 3;

const TAU: f32 = std::f32::consts::TAU;

/// The seed for per-shot variation. Fixed rather than taken from the clock: a
/// capture of frame six hundred has to look the same every time it is taken, or
/// a screenshot stops being evidence of anything.
const RNG_SEED: u32 = 0x5eed_1a7e;

// ─── lifetimes ──────────────────────────────────────────────────────────────

/// How much of an effect's life is left.
///
/// Kept apart from the node it belongs to, and kept as arithmetic rather than
/// as engine state, because this is the part of the effect system worth
/// testing: whether a thing expires once, whether a pool hands out a slot that
/// is still in use, whether an effect that is late by a whole frame ages past
/// its end or wraps round to the beginning.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Life {
    left: f32,
    total: f32,
}

impl Life {
    /// A slot that is not in use. A dead life has no duration to divide by, so
    /// `total` is one and every query about it is guarded by `is_dead` first.
    const DEAD: Self = Self {
        left: 0.0,
        total: 1.0,
    };

    fn new(total: f32) -> Self {
        Self {
            left: total.max(0.0),
            total: total.max(1e-4),
        }
    }

    fn is_dead(self) -> bool {
        self.left <= 0.0
    }

    /// How far through its life the effect is: zero at the instant it fires,
    /// one as it goes out.
    fn age(self) -> f32 {
        (1.0 - self.left / self.total).clamp(0.0, 1.0)
    }

    /// What is left, as a fraction: one at the instant it fires.
    fn remaining(self) -> f32 {
        (self.left / self.total).clamp(0.0, 1.0)
    }

    /// Advances by `dt`, and says whether *this* is the step that ran it out.
    /// An effect that is already dead stays dead, so a slot cannot be switched
    /// off twice or brought back by a late frame.
    fn step(&mut self, dt: f32) -> bool {
        if self.is_dead() {
            return false;
        }
        self.left -= dt;
        if self.left <= 0.0 {
            self.left = 0.0;
            return true;
        }
        false
    }
}

/// Something a pool can switch off the moment its life runs out.
trait Visual {
    fn hide(&mut self);
}

/// A fixed set of nodes, with a lifetime on each.
///
/// The pool owns nothing but the slots: what a slot holds is the caller's
/// business, which is what keeps the handing-out rules testable without an
/// engine to build nodes in.
struct Pool<T> {
    slots: Vec<Slot<T>>,
}

struct Slot<T> {
    item: T,
    life: Life,
    /// What this effect is worth when something more important wants the slot.
    priority: u8,
}

impl<T> Pool<T> {
    /// Takes the items already built. The pool never makes one of its own and
    /// never makes another: it is the whole reason there is no allocation in
    /// the hot path.
    fn new(items: Vec<T>) -> Self {
        Self {
            slots: items
                .into_iter()
                .map(|item| Slot {
                    item,
                    life: Life::DEAD,
                    priority: 0,
                })
                .collect(),
        }
    }

    /// How many slots the pool has. It never changes.
    #[cfg(test)]
    fn capacity(&self) -> usize {
        self.slots.len()
    }

    fn live_count(&self) -> usize {
        self.slots.iter().filter(|s| !s.life.is_dead()).count()
    }

    /// Starts the first free slot's life and hands it over.
    ///
    /// `None` means every slot is busy and the effect is dropped. Dropping the
    /// new one is the right way round: the effects already running are the ones
    /// the player has already seen, and cutting one of those short to make room
    /// for a fresher spark would make a burst flicker.
    fn claim(&mut self, total: f32) -> Option<&mut Slot<T>> {
        self.claim_with(total, 0)
    }

    /// Starts a slot's life, displacing a live effect that matters less if
    /// there is one.
    ///
    /// Only the lights use this, and they use it because a light is the one
    /// effect that changes what the *rest* of the frame looks like: an
    /// explosion that arrives while a muzzle flash holds the last slot should
    /// take it, not go unlit.
    fn claim_with(&mut self, total: f32, priority: u8) -> Option<&mut Slot<T>> {
        let index = match self.slots.iter().position(|s| s.life.is_dead()) {
            Some(free) => free,
            None => self
                .slots
                .iter()
                .enumerate()
                .filter(|(_, s)| s.priority < priority)
                .min_by(|(_, a), (_, b)| a.life.left.total_cmp(&b.life.left))
                .map(|(index, _)| index)?,
        };
        let slot = &mut self.slots[index];
        slot.life = Life::new(total);
        slot.priority = priority;
        Some(slot)
    }
}

impl<T: Visual> Pool<T> {
    /// Ages every slot and switches off whatever ran out.
    fn step(&mut self, dt: f32) {
        for slot in &mut self.slots {
            if slot.life.step(dt) {
                slot.item.hide();
            }
        }
    }

    /// Every slot still in use, with its remaining life. Handed out in slot
    /// order and never allocated: this is what the per-frame animation walks.
    fn live(&mut self) -> impl Iterator<Item = (&mut T, Life)> {
        self.slots
            .iter_mut()
            .filter(|s| !s.life.is_dead())
            .map(|s| (&mut s.item, s.life))
    }
}

// ─── per-shot variation ─────────────────────────────────────────────────────

/// A small deterministic generator, for the variation that stops a burst
/// looking like one frame repeated.
///
/// Every visible difference between two shots in a burst comes from here: the
/// size of the flash, how long it lasts, which way its points are turned, how
/// hot its tint is. It is driven by a counter rather than by the clock so that
/// the same burst looks the same in every capture.
struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Self {
        Self(if seed == 0 { 0x2545_f491 } else { seed })
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// A number in `[-1, 1)`.
    fn signed(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    }

    /// A number in `[0, 1)`.
    fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    /// A multiplier within `spread` of one, so a shot can be given a size or a
    /// duration that is *nearly* the same as the last one.
    fn around(&mut self, spread: f32) -> f32 {
        1.0 + self.signed() * spread
    }

    /// A number in `[low, high)`.
    fn range(&mut self, low: f32, high: f32) -> f32 {
        low + self.unit() * (high - low)
    }
}

// ─── the arithmetic the effects are made of ─────────────────────────────────

/// Where a round is, `age` seconds into its flight.
///
/// The streak is drawn at the weapon's own muzzle velocity and along the
/// direction the simulation reported, so a tracer sits where the bullet is
/// rather than being a mark left at the barrel.
fn tracer_position(from: Vector3, direction: Vector3, age: f32, speed: f32) -> Vector3 {
    from + direction * (speed * age.max(0.0))
}

/// Whether a round's path passes through a point, given how far it has got.
///
/// This is how a tracer learns where it stops. The impact arrives as its own
/// event with its own position, and ending the streak there is what makes a
/// burst read as fire *at* something rather than as fire passing through it. A
/// point behind the muzzle, or off to one side, must not cut a round that is
/// flying somewhere else entirely.
fn tracer_passes(from: Vector3, direction: Vector3, travelled: f32, at: Vector3) -> bool {
    let rel = at - from;
    let along = rel.dot(direction);
    if along < 0.0 || along > travelled + CUT_LEAD {
        return false;
    }
    (rel - direction * along).length() <= CUT_RADIUS
}

/// A basis whose forward — Godot's -Z — points along `direction`, with +X and
/// +Y square to it.
///
/// Built here rather than with `look_at`, which needs the node to be in the
/// tree, refuses to work when the direction is straight up, and would put the
/// roll of every tracer at the mercy of a helper this file does not own.
fn facing(direction: Vector3) -> Basis {
    let forward = direction.normalized();
    // The node's +Z runs back down the line of flight: the streak is built with
    // its head at the origin and its tail along +Z.
    let back = -forward;
    // A round fired straight up has no horizontal component to build the rest
    // of the basis from, and a NaN in a transform deletes the node rather than
    // drawing it at a strange angle, so the reference axis changes instead.
    let reference = if forward.y.abs() > 0.999 {
        Vector3::BACK
    } else {
        Vector3::UP
    };
    let x = reference.cross(back).normalized();
    let y = back.cross(x);
    Basis::from_cols(x, y, back)
}

/// One step of a chunk's flight: gravity, motion, and the floor.
///
/// Debris is integrated here rather than handed to Godot's physics because it
/// is decoration — nothing reacts to it and it has no collider — and because a
/// piece of a wall that arcs and bounces is most of what makes a blast read as
/// having happened *somewhere* rather than as a glow pasted on the screen.
fn ballistics(position: &mut Vector3, velocity: &mut Vector3, dt: f32, floor: f32) {
    velocity.y += GRAVITY * dt;
    *position += *velocity * dt;
    if position.y > floor {
        return;
    }
    position.y = floor;
    if velocity.y >= 0.0 {
        return;
    }
    // One bounce, losing most of its energy: a chunk that bounced like a
    // tennis ball would look like a tennis ball.
    velocity.y = -velocity.y * 0.32;
    velocity.x *= 0.6;
    velocity.z *= 0.6;
    if velocity.y < 1.5 {
        velocity.y = 0.0;
        velocity.x *= 0.35;
        velocity.z *= 0.35;
    }
}

/// A fireball's size part of the way through its life, as a fraction of the
/// blast radius the simulation reported.
///
/// It never exceeds one. That radius is the sphere the splash damage was
/// computed in, and a fireball visibly bigger than it would be the renderer
/// promising damage that was never dealt.
fn fireball_scale(age: f32) -> f32 {
    let t = age.clamp(0.0, 1.0);
    let ease = 1.0 - (1.0 - t) * (1.0 - t);
    0.42 + 0.58 * ease
}

/// A fireball's colour and opacity over its life.
///
/// Three stages, because a fireball does not fade out — it cools: white-hot
/// while the explosive is burning, orange as it turns to soot, and a dull red
/// ember that is nearly gone by the time the smoke arrives.
fn fireball_ramp(age: f32) -> Color {
    let t = age.clamp(0.0, 1.0);
    let (r, g, b) = if t < 0.12 {
        let k = t / 0.12;
        (1.0, 0.94 - 0.3 * k, 0.72 - 0.5 * k)
    } else if t < 0.5 {
        let k = (t - 0.12) / 0.38;
        (1.0, 0.64 - 0.32 * k, 0.22 - 0.14 * k)
    } else {
        let k = (t - 0.5) / 0.5;
        (0.98 - 0.66 * k, 0.32 - 0.26 * k, 0.08 - 0.06 * k)
    };
    Color::from_rgba(r, g, b, (1.0 - t).powf(1.15))
}

/// A light's energy part of the way through its life: full at the instant it
/// fires, then squared away.
///
/// A light that faded *in* would read as a lamp warming up rather than as a
/// discharge, which is why there is no attack here.
fn light_energy(peak: f32, life: Life) -> f32 {
    let spent = life.age();
    peak * (1.0 - spent) * (1.0 - spent)
}

/// How much of a telegraphed marker has filled in.
///
/// The fill is the warning: it reaches the ring exactly as the strike lands, so
/// a player who has learned to read it knows how long they have without
/// counting. Slightly ahead of linear, because the last third of a warning is
/// the part anybody actually acts on.
fn telegraph_fill(elapsed: f32, duration: f32) -> f32 {
    if duration <= 0.0 {
        return 1.0;
    }
    (elapsed / duration).clamp(0.0, 1.0).powf(0.85)
}

/// How hard a telegraph marker is flashing, from the fraction of the warning
/// left. Quiet for most of it, urgent at the end.
fn telegraph_pulse(remaining: f32) -> f32 {
    let urgency = (1.0 - remaining.clamp(0.0, 1.0) / 0.3).clamp(0.0, 1.0);
    0.5 + urgency * 1.6
}

/// What a surface does to a round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImpactLook {
    /// Sparks and a hard ring.
    Hard,
    /// A puff of dust and a dull thud.
    Soft,
}

/// The simulation tags every prop with what it is made of, and the renderer
/// decides what that means.
///
/// This is deliberately the same split the audio uses, with `steel-dark` added
/// because it is steel: a hit that rings in the ear and puffs dust on screen is
/// the sort of thing that stops a player trusting what they are shown.
fn impact_look(surface: &str) -> ImpactLook {
    match surface {
        "steel" | "steel-dark" | "plating" | "armor" | "player" | "tank" | "container"
        | "glass" => ImpactLook::Hard,
        // Concrete, rust, hazard paint, and any surface this renderer has never
        // heard of: a new prop that sparked would be claiming to be metal.
        _ => ImpactLook::Soft,
    }
}

/// How big a machine's death is, from the label the simulation reports.
///
/// Scaled from the archetype's own height in the simulation's config, so a
/// skirmisher does not come apart like the Patriarch. The fallback is the
/// player: a `Destroy` for a label that is not an enemy archetype is the player
/// being killed, those being the only other machines that can die.
fn death_radius(kind: &str) -> f32 {
    use ashframe_sim::config::{artillery, boss, player, skirmisher};
    use ashframe_sim::types::EnemyKind;
    match EnemyKind::from_label(kind) {
        Some(EnemyKind::Skirmisher) => skirmisher::HEIGHT * 0.62,
        Some(EnemyKind::Artillery) => artillery::HEIGHT * 0.68,
        Some(EnemyKind::Boss) => boss::HEIGHT * 0.72,
        None => player::HEIGHT * 0.6,
    }
}

/// The tint of a muzzle flash: white-hot, with enough spread that a burst does
/// not strobe one colour.
fn muzzle_tint(rng: &mut Rng) -> Color {
    let heat = rng.unit();
    Color::from_rgba(1.0, 0.82 + heat * 0.14, 0.52 + heat * 0.32, 1.0)
}

// ─── the pooled things themselves ───────────────────────────────────────────

/// A round in flight.
struct Tracer {
    node: Gd<MeshInstance3D>,
    /// Its own material, because each round's tint and fade are its own, and a
    /// shared one would make a burst one colour and make it blink.
    material: Gd<StandardMaterial3D>,
    from: Vector3,
    direction: Vector3,
    /// Metres a second, from the weapon's config.
    speed: f32,
    tint: Color,
}

impl Visual for Tracer {
    fn hide(&mut self) {
        self.node.set_visible(false);
    }
}

/// A short-lived glowing shape: muzzle flash, impact glow, hit feedback, the
/// middle of a fireball.
struct Flare {
    node: Gd<MeshInstance3D>,
    material: Gd<StandardMaterial3D>,
    size: f32,
    /// How much bigger it gets by the time it goes out.
    growth: f32,
    tint: Color,
}

impl Visual for Flare {
    fn hide(&mut self) {
        self.node.set_visible(false);
    }
}

/// One recycled particle emitter.
struct Burst {
    node: Gd<GpuParticles3D>,
    material: Gd<ParticleProcessMaterial>,
}

impl Visual for Burst {
    fn hide(&mut self) {
        self.node.set_emitting(false);
        self.node.set_visible(false);
    }
}

/// A fireball: a hot core with a cooling shell around it.
struct Blast {
    core: Gd<MeshInstance3D>,
    core_material: Gd<StandardMaterial3D>,
    halo: Gd<MeshInstance3D>,
    halo_material: Gd<StandardMaterial3D>,
    radius: f32,
}

impl Visual for Blast {
    fn hide(&mut self) {
        self.core.set_visible(false);
        self.halo.set_visible(false);
    }
}

/// A flat wave travelling out across the floor.
struct Ring {
    node: Gd<MeshInstance3D>,
    material: Gd<StandardMaterial3D>,
    tint: Color,
    radius: f32,
    /// How far it travels, as a multiple of `radius`.
    reach: f32,
}

impl Visual for Ring {
    fn hide(&mut self) {
        self.node.set_visible(false);
    }
}

/// A telegraphed strike's warning: a ring that fills towards the moment it
/// lands.
struct Marker {
    ring: Gd<MeshInstance3D>,
    ring_material: Gd<StandardMaterial3D>,
    fill: Gd<MeshInstance3D>,
    fill_material: Gd<StandardMaterial3D>,
    curtain: Gd<MeshInstance3D>,
    curtain_material: Gd<StandardMaterial3D>,
    radius: f32,
    duration: f32,
}

impl Visual for Marker {
    fn hide(&mut self) {
        self.ring.set_visible(false);
        self.fill.set_visible(false);
        self.curtain.set_visible(false);
    }
}

/// A piece of what a blast threw.
struct Chunk {
    node: Gd<MeshInstance3D>,
    position: Vector3,
    velocity: Vector3,
    /// Radians a second about each axis.
    spin: Vector3,
    angle: Vector3,
    size: f32,
}

impl Chunk {
    fn fly(&mut self, dt: f32) {
        ballistics(&mut self.position, &mut self.velocity, dt, FLOOR_Y);
        self.angle += self.spin * dt;
        self.node.set_position(self.position);
        self.node.set_rotation(self.angle);
    }

    /// Chunks do not fade — they are lit boxes, and a transparent material per
    /// chunk would be a material per chunk. They sink into the last of their
    /// life instead, which at this size reads as settling into the dust.
    fn shrink(&mut self, life: Life) {
        let t = (life.remaining() / 0.25).min(1.0);
        self.node.set_scale(Vector3::splat(self.size * t));
    }
}

impl Visual for Chunk {
    fn hide(&mut self) {
        self.node.set_visible(false);
    }
}

/// A dynamic light.
struct Light {
    node: Gd<OmniLight3D>,
    peak: f32,
}

impl Visual for Light {
    fn hide(&mut self) {
        self.node.set("light_energy", &0.0f32.to_variant());
        self.node.set_visible(false);
    }
}

/// Everything one burst of particles needs that varies from shot to shot.
///
/// Plain data, so the shape of a burst is something that can be reasoned about
/// — and tested — without a scene tree to hang it in. What is *not* here is the
/// particle count and the lifetime, which are fixed when the emitter is built
/// because changing either of them reallocates the particle buffer.
#[derive(Debug, Clone, Copy, PartialEq)]
struct BurstShot {
    direction: Vector3,
    spread: f32,
    speed_min: f32,
    speed_max: f32,
    scale_min: f32,
    scale_max: f32,
    gravity: Vector3,
    damping: f32,
    /// The fraction of the emitter's particles this burst uses.
    count: f32,
    lifetime_randomness: f64,
    /// The radius of the ball the particles are born in.
    emission_radius: f32,
}

impl BurstShot {
    /// Hot metal coming off something hard: fast, tight, and over quickly.
    fn sparks(direction: Vector3, speed: f32, count: f32) -> Self {
        Self {
            direction,
            spread: 44.0,
            speed_min: speed * 0.5,
            speed_max: speed * 1.5,
            scale_min: 0.7,
            scale_max: 2.0,
            gravity: Vector3::new(0.0, -16.0, 0.0),
            damping: 1.8,
            count,
            lifetime_randomness: 0.6,
            emission_radius: 0.12,
        }
    }

    /// Ground thrown up: slower, wider, and it hangs about.
    fn dust(direction: Vector3, speed: f32, count: f32) -> Self {
        Self {
            direction,
            spread: 74.0,
            speed_min: speed * 0.4,
            speed_max: speed * 1.3,
            // Small and overlapping rather than few and large: a handful of big
            // billboards reads as a string of balls, which is the one thing
            // dust never looks like.
            scale_min: 0.4,
            scale_max: 1.0,
            gravity: Vector3::new(0.0, -1.6, 0.0),
            damping: 2.4,
            count,
            lifetime_randomness: 0.5,
            emission_radius: 0.7,
        }
    }

    /// A column off something that has just burned: it rises, and it lasts.
    fn smoke(direction: Vector3, speed: f32, count: f32) -> Self {
        Self {
            direction,
            spread: 38.0,
            speed_min: speed * 0.45,
            speed_max: speed * 1.4,
            scale_min: 0.8,
            scale_max: 1.9,
            // Upwards: smoke off a burning machine goes up, and a small
            // positive gravity is the cheapest way to say so.
            gravity: Vector3::new(0.0, 0.55, 0.0),
            damping: 1.3,
            count,
            lifetime_randomness: 0.45,
            emission_radius: 0.8,
        }
    }

    /// Whether the ranges this shot carries are the right way round. A minimum
    /// above its maximum is clamped silently by the particle system, which
    /// turns a shaped burst into a uniform one and gives nobody a clue why.
    #[cfg(test)]
    fn sane(&self) -> bool {
        self.speed_min <= self.speed_max
            && self.scale_min <= self.scale_max
            && self.count > 0.0
            && self.count <= 1.0
            && (0.0..=180.0).contains(&self.spread)
            && (self.direction.length() - 1.0).abs() < 1e-3
    }
}

// ─── procedural shapes ──────────────────────────────────────────────────────

/// Vertex data for a mesh, as plain arrays.
///
/// The arithmetic comes first and the engine call comes last, so the shape of a
/// streak or a ring can be checked — does the tail taper, is the ring hollow,
/// does the glow fade from the middle out — in a test that never starts Godot.
#[derive(Default)]
struct Shape {
    vertices: Vec<Vector3>,
    colors: Vec<Color>,
    indices: Vec<i32>,
}

impl Shape {
    fn vertex(&mut self, at: Vector3, colour: Color) -> i32 {
        self.vertices.push(at);
        self.colors.push(colour);
        (self.vertices.len() - 1) as i32
    }

    fn triangle(&mut self, a: i32, b: i32, c: i32) {
        self.indices.extend_from_slice(&[a, b, c]);
    }

    fn quad(&mut self, corners: [i32; 4]) {
        self.triangle(corners[0], corners[1], corners[2]);
        self.triangle(corners[0], corners[2], corners[3]);
    }

    /// Uploads the shape. Called while the game loads and never again.
    fn upload(&self) -> Gd<ArrayMesh> {
        let mut arrays: Array<Variant> = Array::new();
        arrays.resize(ArrayType::MAX.ord() as usize, &Variant::nil());
        arrays.set(
            ArrayType::VERTEX.ord() as usize,
            &PackedVector3Array::from(self.vertices.as_slice()).to_variant(),
        );
        arrays.set(
            ArrayType::COLOR.ord() as usize,
            &PackedColorArray::from(self.colors.as_slice()).to_variant(),
        );
        arrays.set(
            ArrayType::INDEX.ord() as usize,
            &PackedInt32Array::from(self.indices.as_slice()).to_variant(),
        );

        let mut mesh = ArrayMesh::new_gd();
        mesh.add_surface_from_arrays(PrimitiveType::TRIANGLES, &arrays);
        mesh
    }

    /// The radius and alpha of every vertex, for the tests that check a shape's
    /// falloff rather than its exact vertices.
    #[cfg(test)]
    fn profile(&self) -> Vec<(f32, f32)> {
        self.vertices
            .iter()
            .zip(&self.colors)
            .map(|(v, c)| ((v.x * v.x + v.y * v.y).sqrt(), c.a))
            .collect()
    }
}

/// Concentric bands of a flat disc, in the XY plane and facing +Z.
///
/// `bands` are `(radius, alpha)` from the middle outwards and each is filled to
/// the next; a band at radius zero is a fan rather than a ring. Colours are
/// white and carry only alpha, so the tint lives on the material and one mesh
/// serves every use — a glow, a ring, a wavefront.
fn bands_shape(segments: usize, bands: &[(f32, f32)]) -> Shape {
    let mut shape = Shape::default();
    let white = |alpha: f32| Color::from_rgba(1.0, 1.0, 1.0, alpha);

    let mut rings: Vec<Vec<i32>> = Vec::with_capacity(bands.len());
    for (radius, alpha) in bands {
        if *radius <= 1e-4 {
            // One vertex at the middle, shared by every triangle that reaches
            // it: a fan, not a ring of degenerate quads.
            let centre = shape.vertex(Vector3::ZERO, white(*alpha));
            rings.push(vec![centre; segments]);
            continue;
        }
        let mut ring = Vec::with_capacity(segments);
        for i in 0..segments {
            let angle = TAU * i as f32 / segments as f32;
            let at = Vector3::new(angle.cos() * radius, angle.sin() * radius, 0.0);
            ring.push(shape.vertex(at, white(*alpha)));
        }
        rings.push(ring);
    }

    for pair in rings.windows(2) {
        let (inner, outer) = (&pair[0], &pair[1]);
        for i in 0..segments {
            let j = (i + 1) % segments;
            shape.quad([inner[i], inner[j], outer[j], outer[i]]);
        }
    }
    shape
}

/// A soft round glow: brightest in the middle, nothing at the rim.
///
/// One mesh serves every flare in the game — the muzzle flash, the glow on an
/// impact, the core of a fireball, a puff of dust — because they differ only in
/// size, tint and how long they last, and all three of those are node and
/// material properties.
fn glow_shape(segments: usize, radius: f32) -> Shape {
    bands_shape(
        segments,
        &[
            (0.0, 1.0),
            (0.3 * radius, 0.95),
            (0.6 * radius, 0.55),
            (0.85 * radius, 0.18),
            (radius, 0.0),
        ],
    )
}

/// A muzzle flash: a filled, soft-edged core with arms reaching out of it.
///
/// A fan with alternating radii and nothing in the middle — the obvious way to
/// draw a star — reads as four lines meeting at a point. What sells a flash is
/// the bright centre and the arms coming off it, so this builds both.
fn flash_shape(segments: usize, spikes: usize) -> Shape {
    let mut shape = bands_shape(
        segments,
        &[(0.0, 1.0), (0.24, 0.92), (0.44, 0.4), (0.58, 0.0)],
    );
    let centre = shape.vertex(Vector3::ZERO, Color::from_rgba(1.0, 1.0, 1.0, 0.9));
    for spike in 0..spikes {
        let angle = TAU * spike as f32 / spikes as f32;
        let width = TAU / (spikes as f32 * 4.0);
        let mut tip = |at: f32| {
            let point = Vector3::new(at.cos(), at.sin(), 0.0);
            shape.vertex(point, Color::from_rgba(1.0, 1.0, 1.0, 0.0))
        };
        let left = tip(angle - width);
        let right = tip(angle + width);
        shape.triangle(centre, left, right);
    }
    shape
}

/// A tracer: three crossed blades, hot and wide at the head, tapering to
/// nothing at the tail.
///
/// The head sits at the node's origin and the tail runs out along +Z, because
/// Godot's forward is -Z: a node placed at the round's position and turned down
/// the line of flight then has its trail behind it for free. One unit of tail
/// length and one unit of half-width, so a per-shot scale is metres.
fn streak_shape() -> Shape {
    /// `(distance down the tail, half-width, alpha)`. The taper is what stops a
    /// tracer reading as a bar, and the alpha is what gives it a hot core with
    /// a fading tail rather than one flat colour.
    const SECTIONS: [(f32, f32, f32); 3] = [(0.0, 1.0, 1.0), (0.42, 0.55, 0.3), (1.0, 0.18, 0.0)];
    const BLADES: usize = 3;

    let mut shape = Shape::default();
    for blade in 0..BLADES {
        let angle = std::f32::consts::PI * blade as f32 / BLADES as f32;
        // The width direction of this blade, square to the line of flight.
        let across = Vector3::new(angle.cos(), angle.sin(), 0.0);

        let mut sections: Vec<[i32; 3]> = Vec::with_capacity(SECTIONS.len());
        for (z, width, alpha) in SECTIONS {
            let core = shape.vertex(
                Vector3::new(0.0, 0.0, z),
                Color::from_rgba(1.0, 1.0, 1.0, alpha),
            );
            let mut edge = |side: f32| {
                let at = across * (width * side) + Vector3::new(0.0, 0.0, z);
                // The edges are transparent, so the streak has no silhouette to
                // give away that it is a strip of triangles.
                shape.vertex(at, Color::from_rgba(1.0, 1.0, 1.0, 0.0))
            };
            let left = edge(-0.5);
            let right = edge(0.5);
            sections.push([left, core, right]);
        }

        for pair in sections.windows(2) {
            let (head, tail) = (pair[0], pair[1]);
            shape.quad([head[0], tail[0], tail[1], head[1]]);
            shape.quad([head[1], tail[1], tail[2], head[2]]);
        }
    }
    shape
}

/// One spark: a sliver, brightest at its leading end. The particle system
/// stretches it along the spark's own velocity, so the mesh only has to be the
/// right shape at rest.
fn spark_shape(width: f32, length: f32) -> Shape {
    let mut shape = Shape::default();
    let cold = Color::from_rgba(1.0, 0.75, 0.4, 0.0);
    let hot = Color::from_rgba(1.0, 1.0, 0.85, 1.0);
    let tail_l = shape.vertex(Vector3::new(-width, -length, 0.0), cold);
    let tail_r = shape.vertex(Vector3::new(width, -length, 0.0), cold);
    let head_r = shape.vertex(Vector3::new(width, length, 0.0), hot);
    let head_l = shape.vertex(Vector3::new(-width, length, 0.0), hot);
    shape.quad([tail_l, tail_r, head_r, head_l]);
    shape
}

/// A soft puff for dust and smoke.
///
/// Softer than a glow, and fainter in the middle than at its edge band: a puff
/// is one billboard out of a cloud, and the eye reads a hard edge on a
/// billboard as exactly what it is. Overlapping faint puffs make a cloud;
/// overlapping bright ones make a string of balls.
fn puff_shape(segments: usize, radius: f32) -> Shape {
    bands_shape(
        segments,
        &[
            (0.0, 0.5),
            (0.35 * radius, 0.46),
            (0.68 * radius, 0.2),
            (0.88 * radius, 0.05),
            (radius, 0.0),
        ],
    )
}

/// The wall of light that stands on a telegraph ring.
///
/// A ring painted flat on the yard is legible from above and nearly invisible
/// from the third-person camera, which looks *along* the ground rather than
/// down at it: at ten metres a flat ring is one bright line across the screen
/// and the fill inside it cannot be seen at all. A low curtain on the same
/// circle carries the whole marker at that angle, for one more node.
fn curtain_shape(segments: usize, height: f32) -> Shape {
    let mut shape = Shape::default();
    let mut base = Vec::with_capacity(segments);
    let mut top = Vec::with_capacity(segments);
    for i in 0..segments {
        let angle = TAU * i as f32 / segments as f32;
        let (x, z) = (angle.cos(), angle.sin());
        base.push(shape.vertex(
            Vector3::new(x, 0.0, z),
            Color::from_rgba(1.0, 1.0, 1.0, 0.8),
        ));
        top.push(shape.vertex(
            Vector3::new(x, height, z),
            Color::from_rgba(1.0, 1.0, 1.0, 0.0),
        ));
    }
    for i in 0..segments {
        let j = (i + 1) % segments;
        shape.quad([base[i], base[j], top[j], top[i]]);
    }
    shape
}

/// A flat ring: a band of light with nothing inside it.
fn ring_shape(segments: usize, radius: f32, inner: f32) -> Shape {
    bands_shape(
        segments,
        &[
            (radius * inner, 0.0),
            (radius * (inner + (1.0 - inner) * 0.5), 1.0),
            (radius, 0.0),
        ],
    )
}

/// The disc that fills a telegraph ring: nearly clear in the middle, with a
/// bright wavefront at its edge, so that scaling it up reads as a fill arriving
/// rather than as a circle appearing.
fn fill_shape(segments: usize, radius: f32) -> Shape {
    bands_shape(
        segments,
        &[
            (0.0, 0.12),
            (0.55 * radius, 0.16),
            (0.88 * radius, 0.3),
            (0.96 * radius, 0.85),
            (radius, 0.0),
        ],
    )
}

/// The boxes debris is cut from: a box, but with some proportion to it, because
/// a cube tumbling through the air reads as a bug.
fn chunk_meshes() -> Vec<Gd<Mesh>> {
    let sizes = [
        Vector3::new(0.34, 0.2, 0.22),
        Vector3::new(0.24, 0.34, 0.26),
        Vector3::new(0.44, 0.16, 0.3),
    ];
    sizes
        .iter()
        .map(|size| {
            let mut mesh = BoxMesh::new_gd();
            mesh.set_size(*size);
            mesh.upcast::<Mesh>()
        })
        .collect()
}

// ─── materials ──────────────────────────────────────────────────────────────

/// The common ground of every effect material: unlit, transparent, and taking
/// its colour from the mesh's vertex colours.
///
/// Unlit because these are light sources rather than surfaces — a shaded tracer
/// would dim as it turned away from the sun, which is exactly the wrong cue for
/// something crossing the yard at three hundred metres a second — and
/// vertex-coloured because that is where each shape's falloff, taper and fade
/// are baked.
fn effect_material(tint: Color, boost: f32, blend: BlendMode) -> Gd<StandardMaterial3D> {
    let mut mat = unlit(bright(tint, boost));
    // The palette's unlit material glows, and a constant glow is exactly what
    // these shapes cannot have: vertex colour multiplies the albedo but not the
    // emission, so leaving it on would lay a flat white term over every ramp
    // and give a tracer a white-hot tail. These glow by being bright, not by
    // being emissive.
    mat.set("emission_enabled", &false.to_variant());
    mat.set_transparency(Transparency::ALPHA);
    mat.set_blend_mode(blend);
    // Nothing is culled: these shapes are seen from both sides — a ring lying
    // on the ground from above and below, a tracer from either flank — and a
    // back face that vanishes is a tracer that disappears when it turns.
    mat.set_cull_mode(CullMode::DISABLED);
    mat.set_depth_draw_mode(DepthDrawMode::OPAQUE_ONLY);
    mat.set_flag(Flags::ALBEDO_FROM_VERTEX_COLOR, true);
    mat.set_albedo(bright(tint, boost));
    mat
}

/// A colour pushed above white, so the glow pass picks it up and a flare reads
/// as light rather than as a white sticker on the wall behind it.
fn bright(tint: Color, boost: f32) -> Color {
    Color::from_rgba(tint.r * boost, tint.g * boost, tint.b * boost, tint.a)
}

/// A glow that always faces the camera: muzzle flashes, impact glows, the
/// middle of a fireball.
fn flare_material(tint: Color, boost: f32) -> Gd<StandardMaterial3D> {
    let mut mat = effect_material(tint, boost, BlendMode::ADD);
    mat.set_billboard_mode(BillboardMode::ENABLED);
    mat
}

/// A glow that keeps its own orientation: a tracer streak, a wavefront, a ring
/// lying flat on the ground.
fn wave_material(tint: Color, boost: f32, blend: BlendMode) -> Gd<StandardMaterial3D> {
    effect_material(tint, boost, blend)
}

/// A soft puff, facing the camera: dust and smoke.
///
/// Lit rather than unlit, which is the difference between smoke and cotton
/// wool: an unlit puff is the same flat brightness whatever it is in front of,
/// so it reads as a hole in the picture, while a lit one takes the sun, the
/// blast light and the fog and sits in the yard with everything else.
fn puff_material() -> Gd<StandardMaterial3D> {
    let mut mat = material(Color::WHITE, 0.0, 1.0);
    mat.set_transparency(Transparency::ALPHA);
    mat.set_cull_mode(CullMode::DISABLED);
    mat.set_depth_draw_mode(DepthDrawMode::OPAQUE_ONLY);
    mat.set_billboard_mode(BillboardMode::ENABLED);
    mat.set_flag(Flags::ALBEDO_FROM_VERTEX_COLOR, true);
    // A billboard has no depth of its own, so a shadow cast onto it lands as a
    // band across the puff rather than as a shadow on a volume.
    mat.set_flag(Flags::DONT_RECEIVE_SHADOWS, true);
    mat
}

/// A gradient, as a texture: the colour and fade of a particle over its life.
fn ramp_texture(stops: &[(f32, f32, f32, f32, f32)]) -> Gd<GradientTexture1D> {
    let mut gradient = Gradient::new_gd();
    for (offset, r, g, b, a) in stops {
        gradient.add_point(*offset, Color::from_rgba(*r, *g, *b, *a));
    }
    let mut texture = GradientTexture1D::new_gd();
    texture.set_gradient(&gradient);
    texture
}

/// A curve, as a texture, for the properties that are graphed over a particle's
/// life rather than set once.
fn curve_texture(points: &[(f32, f32)]) -> Gd<CurveTexture> {
    let mut curve = Curve::new_gd();
    for (at, value) in points {
        curve.add_point(Vector2::new(*at, *value));
    }
    let mut texture = CurveTexture::new_gd();
    texture.set_curve(&curve);
    texture
}

// ─── the system ─────────────────────────────────────────────────────────────

/// The fixed half of a class of particle burst: what it is drawn with, and how
/// many particles it may ever use. None of this can change while the game is
/// running, which is what keeps a burst off the allocator.
struct BurstLook<'a> {
    mesh: &'a Gd<ArrayMesh>,
    /// The material the particles are drawn with. It lives on the node rather
    /// than on the mesh, so that sparks can add light while dust and smoke mix
    /// with what is behind them.
    material: Gd<StandardMaterial3D>,
    ramp: &'a Gd<GradientTexture1D>,
    grow: &'a Gd<CurveTexture>,
    particles: i32,
    lifetime: f64,
    /// Sparks are stretched along their own velocity; dust and smoke are round
    /// puffs that face the camera.
    stretched: bool,
    /// Whether the particles are stirred as they rise, for smoke.
    curls: bool,
}

/// The whole effects system.
pub struct Effects {
    tracers: Pool<Tracer>,
    flares: Pool<Flare>,
    glows: Pool<Flare>,
    sparks: Pool<Burst>,
    dust: Pool<Burst>,
    smoke: Pool<Burst>,
    blasts: Pool<Blast>,
    rings: Pool<Ring>,
    markers: Pool<Marker>,
    chunks: Pool<Chunk>,
    lights: Pool<Light>,
    /// The boxes and paints the debris is cut from, kept for the per-shot pick.
    chunk_meshes: Vec<Gd<Mesh>>,
    chunk_materials: Vec<Gd<StandardMaterial3D>>,
    rng: Rng,
}

impl Effects {
    pub fn build(parent: &mut Gd<Node3D>) -> Self {
        let mut root = Node3D::new_alloc();
        root.set_name("Effects");
        parent.add_child(&root);

        // Every shape and shared resource below is built here, once. From this
        // point on the node count of the effects system is fixed for the match.
        let glow = glow_shape(20, 1.0).upload();
        let puff = puff_shape(16, 0.5).upload();
        let star = flash_shape(24, 6).upload();
        let streak = streak_shape().upload();
        let spark = spark_shape(0.05, 0.32).upload();
        let ring = ring_shape(64, 1.0, 0.87).upload();
        let fill = fill_shape(48, 1.0).upload();
        let curtain = curtain_shape(48, 1.0).upload();

        let sparks_ramp = ramp_texture(&[
            (0.0, 1.0, 0.97, 0.85, 1.0),
            (0.22, 1.0, 0.72, 0.28, 1.0),
            (0.65, 0.86, 0.3, 0.08, 0.5),
            (1.0, 0.4, 0.1, 0.05, 0.0),
        ]);
        let dust_ramp = ramp_texture(&[
            (0.0, 0.84, 0.79, 0.7, 0.0),
            (0.14, 0.86, 0.81, 0.72, 0.5),
            (0.55, 0.8, 0.75, 0.66, 0.34),
            (1.0, 0.72, 0.68, 0.6, 0.0),
        ]);
        let smoke_ramp = ramp_texture(&[
            (0.0, 0.36, 0.32, 0.3, 0.0),
            (0.12, 0.44, 0.4, 0.37, 0.72),
            (0.5, 0.5, 0.47, 0.44, 0.45),
            (1.0, 0.54, 0.52, 0.5, 0.0),
        ]);
        // Puffs grow as they age: a dust cloud that stayed the size it was born
        // reads as a bag of spheres rather than as air moving. Sparks do not.
        let grow = curve_texture(&[(0.0, 0.45), (0.45, 1.0), (1.0, 1.9)]);
        let steady = curve_texture(&[(0.0, 1.0), (1.0, 1.0)]);

        let tracers: Vec<Tracer> = (0..TRACER_POOL)
            .map(|_| {
                let material = wave_material(Color::WHITE, TRAIL_BOOST, BlendMode::ADD);
                let node = mesh_node(&mut root, &streak, &material);
                Tracer {
                    node,
                    material,
                    from: Vector3::ZERO,
                    direction: Vector3::FORWARD,
                    speed: rifle::SPEED,
                    tint: Color::WHITE,
                }
            })
            .collect();

        let flares: Vec<Flare> = (0..FLASH_POOL)
            .map(|_| {
                let material = flare_material(Color::WHITE, FLARE_BOOST);
                let node = mesh_node(&mut root, &star, &material);
                Flare {
                    node,
                    material,
                    size: 1.0,
                    growth: 0.5,
                    tint: Color::WHITE,
                }
            })
            .collect();

        // Round glows are a separate pool from the flash: an impact or a hit
        // is a bloom of light, and a four-pointed star on a mech's shoulder
        // reads as a sticker.
        let glows: Vec<Flare> = (0..GLOW_POOL)
            .map(|_| {
                let material = flare_material(Color::WHITE, FLARE_BOOST);
                let node = mesh_node(&mut root, &glow, &material);
                Flare {
                    node,
                    material,
                    size: 1.0,
                    growth: 0.5,
                    tint: Color::WHITE,
                }
            })
            .collect();

        let sparks = burst_pool(
            &mut root,
            SPARK_POOL,
            &BurstLook {
                mesh: &spark,
                material: wave_material(Color::WHITE, 2.0, BlendMode::ADD),
                ramp: &sparks_ramp,
                grow: &steady,
                particles: 32,
                lifetime: 0.34,
                stretched: true,
                curls: false,
            },
        );
        let dust = burst_pool(
            &mut root,
            DUST_POOL,
            &BurstLook {
                mesh: &puff,
                material: puff_material(),
                ramp: &dust_ramp,
                grow: &grow,
                particles: 28,
                lifetime: 0.85,
                stretched: false,
                curls: false,
            },
        );
        let smoke = burst_pool(
            &mut root,
            SMOKE_POOL,
            &BurstLook {
                mesh: &puff,
                material: puff_material(),
                ramp: &smoke_ramp,
                grow: &grow,
                particles: 20,
                lifetime: 2.0,
                stretched: false,
                curls: true,
            },
        );

        let blasts: Vec<Blast> = (0..BLAST_POOL)
            .map(|_| {
                let core_material = flare_material(Color::WHITE, CORE_BOOST);
                let halo_material = flare_material(Color::WHITE, 1.0);
                let core = mesh_node(&mut root, &glow, &core_material);
                let halo = mesh_node(&mut root, &glow, &halo_material);
                Blast {
                    core,
                    core_material,
                    halo,
                    halo_material,
                    radius: 1.0,
                }
            })
            .collect();

        let rings: Vec<Ring> = (0..RING_POOL)
            .map(|_| {
                let material = wave_material(Color::WHITE, 1.5, BlendMode::ADD);
                let mut node = mesh_node(&mut root, &ring, &material);
                node.set_rotation(flat());
                Ring {
                    node,
                    material,
                    tint: Color::WHITE,
                    radius: 1.0,
                    reach: 1.4,
                }
            })
            .collect();

        let markers: Vec<Marker> = (0..MARKER_POOL)
            .map(|_| {
                // The ring is painted light and the fill is added light: the
                // edge has to stay legible on pale concrete in daylight, and
                // the fill has to glow inside it.
                let ring_material = wave_material(Color::WHITE, 1.3, BlendMode::MIX);
                let fill_material = wave_material(Color::WHITE, 1.2, BlendMode::ADD);
                let curtain_material = wave_material(Color::WHITE, 1.1, BlendMode::ADD);
                let mut ring = mesh_node(&mut root, &ring, &ring_material);
                let mut fill = mesh_node(&mut root, &fill, &fill_material);
                let curtain = mesh_node(&mut root, &curtain, &curtain_material);
                ring.set_rotation(flat());
                fill.set_rotation(flat());
                Marker {
                    ring,
                    ring_material,
                    fill,
                    fill_material,
                    curtain,
                    curtain_material,
                    radius: 1.0,
                    duration: 1.0,
                }
            })
            .collect();

        let chunk_meshes = chunk_meshes();
        let chunk_materials = vec![
            material(Color::from_rgba(0.22, 0.2, 0.19, 1.0), 0.1, 0.85),
            material(Color::from_rgba(0.42, 0.3, 0.24, 1.0), 0.2, 0.7),
        ];
        let chunks: Vec<Chunk> = (0..DEBRIS_POOL)
            .map(|_| {
                let mut node = MeshInstance3D::new_alloc();
                node.set_mesh(&chunk_meshes[0]);
                node.set_material_override(&chunk_materials[0]);
                node.set_visible(false);
                root.add_child(&node);
                Chunk {
                    node,
                    position: Vector3::ZERO,
                    velocity: Vector3::ZERO,
                    spin: Vector3::ZERO,
                    angle: Vector3::ZERO,
                    size: 1.0,
                }
            })
            .collect();

        let lights: Vec<Light> = (0..LIGHT_POOL)
            .map(|_| {
                let mut node = OmniLight3D::new_alloc();
                // Light properties are set by name: Godot routes most of them
                // through Light3D.set_param, and the generic property setter is
                // both shorter and identical for every one of them.
                node.set("light_energy", &0.0f32.to_variant());
                node.set("shadow_enabled", &false.to_variant());
                node.set_visible(false);
                root.add_child(&node);
                Light { node, peak: 0.0 }
            })
            .collect();

        Self {
            tracers: Pool::new(tracers),
            flares: Pool::new(flares),
            glows: Pool::new(glows),
            sparks,
            dust,
            smoke,
            blasts: Pool::new(blasts),
            rings: Pool::new(rings),
            markers: Pool::new(markers),
            chunks: Pool::new(chunks),
            lights: Pool::new(lights),
            chunk_meshes,
            chunk_materials,
            rng: Rng::new(RNG_SEED),
        }
    }

    /// A round in flight: a hot streak travelling the round's own path.
    pub fn tracer(&mut self, at: Vec3, direction: Vec3) {
        let from = to_godot(at);
        let aim = to_godot(direction);
        if aim.length_squared() < 1e-6 {
            return;
        }
        let direction = aim.normalized();

        let tint = muzzle_tint(&mut self.rng);
        // The trail is a few metres of the round's flight, not all of it: it is
        // the streak the eye catches, and the round itself has already gone.
        let trail = self.rng.range(5.5, 7.5);
        let width = self.rng.range(0.075, 0.11);

        let Some(slot) = self.tracers.claim(TRAIL_LIFE) else {
            return;
        };
        let tracer = &mut slot.item;
        tracer.from = from;
        tracer.direction = direction;
        tracer.tint = tint;
        tracer.node.set_transform(Transform3D::new(
            facing(direction),
            tracer_position(from, direction, 0.0, tracer.speed),
        ));
        tracer.node.set_scale(Vector3::new(width, width, trail));
        tracer.node.set_visible(true);
        tracer.material.set_albedo(tint * TRAIL_BOOST);
    }

    /// Ends the tracers whose round has met something.
    fn cut_tracers(&mut self, at: Vector3) {
        for slot in &mut self.tracers.slots {
            if slot.life.is_dead() {
                continue;
            }
            let travelled = slot.life.age() * TRAIL_LIFE * slot.item.speed;
            if tracer_passes(slot.item.from, slot.item.direction, travelled, at) {
                slot.item.hide();
                // Hiding switches the node off; killing the life as well is
                // what returns the slot to the pool on this step rather than at
                // the end of a flight that is no longer happening.
                slot.life = Life::DEAD;
            }
        }
    }

    /// A flare at the muzzle and a light on it, for the moment the barrel is
    /// the brightest thing in the yard.
    ///
    /// Size, duration, roll and tint all vary a little per shot, because a
    /// burst that flashes the identical frame seven times a second reads as a
    /// stutter rather than as fire.
    pub fn muzzle_flash(&mut self, at: Vec3, energy: f32) {
        let at = to_godot(at);
        let size = 0.95 * energy * self.rng.around(0.3);
        let life = 0.06 * self.rng.around(0.4);
        let tint = muzzle_tint(&mut self.rng);
        let roll = self.rng.unit() * TAU;

        self.flare(FlareShot {
            at,
            size,
            life,
            tint,
            growth: 0.8,
            roll,
        });
        self.light(at, 4.0 * energy, 12.0 * energy, life, LIGHT_MUZZLE, tint);
    }

    /// A round meeting scenery. What it hit decides what it looks like, and the
    /// surface tag comes from the simulation rather than from a guess here.
    pub fn impact(&mut self, at: Vec3, surface: &str, normal: Vec3) {
        let at = to_godot(at);
        let normal = to_godot(normal);
        let normal = if normal.length_squared() > 1e-6 {
            normal.normalized()
        } else {
            Vector3::UP
        };

        match impact_look(surface) {
            ImpactLook::Hard => {
                // Sparks come off along the surface normal, which is the one
                // piece of geometry the impact event carries.
                self.spark_burst(at, BurstShot::sparks(normal, 10.0, 0.9), 0.55);
                let tint = muzzle_tint(&mut self.rng);
                let roll = self.rng.unit() * TAU;
                self.glow(FlareShot {
                    at,
                    size: 0.8,
                    life: 0.14,
                    tint,
                    growth: 0.8,
                    roll,
                });
            }
            ImpactLook::Soft => {
                // A round into concrete does not spark; it takes a bite out of
                // it and leaves a puff hanging where the hole is.
                self.dust_burst(at, BurstShot::dust(normal, 4.0, 0.6), 0.95);
                self.glow(FlareShot {
                    at,
                    size: 0.4,
                    life: 0.08,
                    tint: Color::from_rgba(1.0, 0.72, 0.42, 1.0),
                    growth: 0.4,
                    roll: 0.0,
                });
            }
        }
        self.cut_tracers(at);
    }

    /// Damage landing on a mech: a flare where it landed, and sparks off the
    /// plate.
    pub fn mech_hit(&mut self, at: Vec3) {
        let at = to_godot(at);
        self.spark_burst(at, BurstShot::sparks(Vector3::UP, 10.0, 0.7), 0.45);
        let roll = self.rng.unit() * TAU;
        self.glow(FlareShot {
            at,
            size: 1.3,
            life: 0.16,
            tint: Color::from_rgba(1.0, 0.9, 0.68, 1.0),
            growth: 1.1,
            roll,
        });
        self.light(
            at,
            2.2,
            7.0,
            0.1,
            LIGHT_HIT,
            Color::from_rgba(1.0, 0.86, 0.62, 1.0),
        );
        // A round that hits a mech stops there, so a tracer aimed through it
        // ends here rather than carrying on out of the far side.
        self.cut_tracers(at);
    }

    /// The blade meeting something. Heavier than a bullet: the arc blade is
    /// most of a mech's mass arriving at once.
    pub fn blade_hit(&mut self, at: Vec3) {
        let at = to_godot(at);
        self.spark_burst(at, BurstShot::sparks(Vector3::UP, 17.0, 1.0), 0.55);
        let roll = self.rng.unit() * TAU;
        self.glow(FlareShot {
            at,
            size: 1.9,
            life: 0.22,
            tint: Color::from_rgba(1.0, 0.8, 0.5, 1.0),
            growth: 1.3,
            roll,
        });
        self.light(
            at,
            3.4,
            9.0,
            0.14,
            LIGHT_HIT,
            Color::from_rgba(1.0, 0.8, 0.55, 1.0),
        );
        self.cut_tracers(at);
    }

    /// A warhead going off: fireball, shell, ground wave, smoke, debris, and a
    /// light that lifts the yard for as long as the flash lasts.
    ///
    /// Every part is a separate pool, and any one of them may drop if its pool
    /// is full — which is a thinner explosion, never a broken one.
    pub fn explosion(&mut self, at: Vec3, radius: f32) {
        let centre = to_godot(at);
        let radius = radius.max(0.5);
        // The ground wave only exists when there is ground under the blast: a
        // flat ring hanging at head height is worse than no ring at all.
        let on_the_floor = centre.y <= FLOOR_Y + radius * 0.4;

        if let Some(slot) = self.blasts.claim(0.45 + radius * 0.03) {
            let blast = &mut slot.item;
            blast.radius = radius;
            let lifted = centre + Vector3::UP * (radius * 0.15);
            blast.core.set_position(lifted);
            blast.halo.set_position(lifted);
        }

        if on_the_floor {
            if let Some(slot) = self.rings.claim(0.5) {
                let ring = &mut slot.item;
                ring.radius = radius;
                ring.reach = 1.35;
                ring.tint = Color::from_rgba(1.0, 0.62, 0.3, 1.0);
                ring.node
                    .set_position(Vector3::new(centre.x, FLOOR_Y + 0.14, centre.z));
            }
        }

        self.smoke_burst(
            centre + Vector3::UP * (radius * 0.3),
            BurstShot::smoke(Vector3::UP, 2.6 + radius * 0.25, 0.9),
            2.2,
        );
        if on_the_floor {
            self.dust_burst(
                Vector3::new(centre.x, FLOOR_Y + 0.2, centre.z),
                BurstShot::dust(Vector3::UP, 4.0 + radius * 0.5, 0.8),
                1.1,
            );
        }

        // Debris scales with the blast, and the pool drops the rest rather than
        // growing: a bigger explosion gets more chunks, not more nodes.
        let count = (3.0 + radius * 0.7) as usize;
        self.debris(centre, radius, count);

        self.light(
            centre + Vector3::UP * (radius * 0.3),
            4.5 + radius * 0.9,
            6.0 + radius * 3.2,
            0.4,
            LIGHT_BLAST,
            Color::from_rgba(1.0, 0.66, 0.32, 1.0),
        );
    }

    /// A machine coming apart. Sized from the archetype, because a skirmisher
    /// and the Patriarch are not the same amount of explosion.
    pub fn destroy(&mut self, at: Vec3, kind: &str) {
        let radius = death_radius(kind);
        self.explosion(at, radius);
        // A second helping of debris and a longer column of smoke, so a death
        // is unmistakably bigger than a missile landing next to a machine.
        let centre = to_godot(at);
        self.debris(centre + Vector3::UP * 1.2, radius, 6);
        self.smoke_burst(
            centre + Vector3::UP * (radius * 0.5),
            BurstShot::smoke(Vector3::UP, 3.4, 1.0),
            2.2,
        );
    }

    /// The warning before a telegraphed strike: a ring that fills towards the
    /// moment it lands, and gets urgent as it does.
    pub fn telegraph(&mut self, at: Vec3, radius: f32, duration: f32) {
        let at = to_godot(at);
        let duration = duration.max(0.15);
        let radius = radius.max(0.5);
        let Some(slot) = self.markers.claim(duration) else {
            return;
        };
        let marker = &mut slot.item;
        marker.radius = radius;
        marker.duration = duration;
        // A hand's width above the ground: enough that the ring is not fighting
        // the floor for the same depth, near enough that it reads as *on* it.
        let on_the_ground = Vector3::new(at.x, at.y + 0.09, at.z);
        marker.ring.set_position(on_the_ground);
        marker.fill.set_position(on_the_ground + Vector3::UP * 0.01);
        marker.curtain.set_position(on_the_ground);
        marker.ring.set_scale(Vector3::new(radius, 1.0, radius));
        marker.curtain.set_scale(Vector3::new(radius, 1.0, radius));
        marker
            .fill
            .set_scale(Vector3::new(radius * 0.02, 1.0, radius * 0.02));
        marker.ring.set_visible(true);
        marker.fill.set_visible(true);
        marker.curtain.set_visible(true);
    }

    /// Feet meeting the ground. The simulation reports the speed of the
    /// landing, so a step off a kerb does not raise a cloud.
    pub fn landing(&mut self, at: Vec3, speed: f32) {
        let at = to_godot(at);
        let weight = (speed / 34.0).clamp(0.18, 1.0);
        let shot = BurstShot::dust(Vector3::UP, 3.0 + 6.0 * weight, 0.35 + 0.6 * weight);
        self.dust_burst(Vector3::new(at.x, at.y + 0.2, at.z), shot, 1.0);
        if speed > 22.0 {
            // A hard landing cracks the ground it lands on.
            if let Some(slot) = self.rings.claim(0.45) {
                let ring = &mut slot.item;
                ring.radius = 2.0 + weight * 2.5;
                ring.reach = 1.5;
                ring.tint = Color::from_rgba(0.9, 0.85, 0.72, 1.0);
                ring.node
                    .set_position(Vector3::new(at.x, FLOOR_Y + 0.12, at.z));
            }
        }
    }

    /// Thrusters lighting: the blast of a jump throws dust out from under the
    /// machine.
    pub fn jump_dust(&mut self, at: Vec3) {
        let at = to_godot(at);
        self.dust_burst(
            Vector3::new(at.x, at.y + 0.15, at.z),
            BurstShot::dust(Vector3::UP, 5.0, 0.55),
            1.0,
        );
    }

    /// A quick boost: dust thrown backwards, the way the machine did not go.
    pub fn boost_dust(&mut self, at: Vec3, direction: Vec3) {
        let at = to_godot(at);
        let push = to_godot(direction);
        if push.length_squared() < 1e-6 {
            return;
        }
        // The exhaust went one way and the machine went the other, so the dust
        // goes with the exhaust.
        self.dust_burst(
            Vector3::new(at.x, at.y + 0.2, at.z),
            BurstShot::dust(-push.normalized(), 7.0, 0.7),
            1.0,
        );
    }

    /// A missile leaving the tube: a flash and a plume that hangs where the
    /// launch was. The missile's own flight is not drawn, because the renderer
    /// is never told where it goes.
    pub fn missile_launch(&mut self, at: Vec3, direction: Vec3) {
        self.muzzle_flash(at, 1.4);
        let at = to_godot(at);
        let push = to_godot(direction);
        let push = if push.length_squared() > 1e-6 {
            push.normalized()
        } else {
            Vector3::UP
        };
        self.smoke_burst(at, BurstShot::smoke(push, 4.0, 0.8), 2.2);
    }

    /// A flare from the pool, placed and sized. Dropped when the pool is full,
    /// which is the pool's business rather than this one's.
    fn flare(&mut self, shot: FlareShot) {
        let Some(slot) = self.flares.claim(shot.life) else {
            return;
        };
        place_flare(&mut slot.item, shot);
    }

    /// The same, from the round glows: an impact, a hit, a blade landing.
    fn glow(&mut self, shot: FlareShot) {
        let Some(slot) = self.glows.claim(shot.life) else {
            return;
        };
        place_flare(&mut slot.item, shot);
    }

    fn spark_burst(&mut self, at: Vector3, shot: BurstShot, life: f32) {
        let Some(slot) = self.sparks.claim(life) else {
            return;
        };
        start_burst(&mut slot.item, at, &shot);
    }

    fn dust_burst(&mut self, at: Vector3, shot: BurstShot, life: f32) {
        let Some(slot) = self.dust.claim(life) else {
            return;
        };
        start_burst(&mut slot.item, at, &shot);
    }

    fn smoke_burst(&mut self, at: Vector3, shot: BurstShot, life: f32) {
        let Some(slot) = self.smoke.claim(life) else {
            return;
        };
        start_burst(&mut slot.item, at, &shot);
    }

    /// Throws chunks of whatever the blast was standing on.
    fn debris(&mut self, at: Vector3, radius: f32, count: usize) {
        for _ in 0..count {
            let speed = self.rng.range(5.0, 9.0 + radius * 1.4);
            let lift = self.rng.range(0.35, 1.15);
            let spin = Vector3::new(
                self.rng.signed() * 9.0,
                self.rng.signed() * 9.0,
                self.rng.signed() * 9.0,
            );
            let size = self.rng.range(0.9, 2.0);
            let duration = self.rng.range(1.6, 3.0);
            let mesh = self.rng.next_u32() as usize % self.chunk_meshes.len();
            let paint = self.rng.next_u32() as usize % self.chunk_materials.len();
            let (throw, rise) = (
                Vector3::new(
                    self.rng.signed() * radius * 0.3,
                    radius * 0.2,
                    self.rng.signed() * radius * 0.3,
                ),
                Vector3::new(
                    self.rng.signed() * speed,
                    speed * lift,
                    self.rng.signed() * speed,
                ),
            );
            let Some(slot) = self.chunks.claim(duration) else {
                // The pool is full. The rest of the blast is already on its
                // way, so there is nothing to undo: there are simply fewer
                // pieces than there might have been.
                return;
            };
            let chunk = &mut slot.item;
            chunk.position = at + throw;
            chunk.velocity = rise;
            chunk.spin = spin;
            chunk.angle = Vector3::ZERO;
            chunk.size = size;
            chunk.node.set_mesh(&self.chunk_meshes[mesh]);
            chunk
                .node
                .set_material_override(&self.chunk_materials[paint]);
            chunk.node.set_scale(Vector3::splat(size));
            chunk.node.set_position(chunk.position);
            chunk.node.set_visible(true);
        }
    }

    /// A light, from a small pool that is first-come-first-served except that a
    /// more important flash may take a slot from a less important one.
    fn light(&mut self, at: Vector3, peak: f32, range: f32, life: f32, priority: u8, tint: Color) {
        let Some(slot) = self.lights.claim_with(life, priority) else {
            return;
        };
        let light = &mut slot.item;
        light.peak = peak;
        light.node.set_position(at);
        light.node.set("omni_range", &range.to_variant());
        light.node.set("light_color", &tint.to_variant());
        light.node.set("light_energy", &peak.to_variant());
        light.node.set_visible(true);
    }

    /// Advances every effect. This is the whole per-frame cost of the system:
    /// one pass over a few hundred pre-built slots, writing transforms and
    /// colours on resources that already exist.
    pub fn step(&mut self, dt: f32) {
        // A hitch must not throw a round a hundred metres down range or drop a
        // chunk through the floor, so the step is capped at something a frame
        // could plausibly be.
        let dt = dt.clamp(0.0, 0.1);

        self.tracers.step(dt);
        for (tracer, life) in self.tracers.live() {
            let travelled = life.age() * TRAIL_LIFE * tracer.speed;
            tracer
                .node
                .set_position(tracer.from + tracer.direction * travelled);
            let fade = (life.remaining() / TRAIL_FADE).min(1.0);
            tracer
                .material
                .set_albedo(tracer.tint * (fade * TRAIL_BOOST));
        }

        self.flares.step(dt);
        for (flare, life) in self.flares.live() {
            animate_flare(flare, life);
        }

        self.glows.step(dt);
        for (glow, life) in self.glows.live() {
            animate_flare(glow, life);
        }

        for pool in [&mut self.sparks, &mut self.dust, &mut self.smoke] {
            pool.step(dt);
        }

        self.blasts.step(dt);
        for (blast, life) in self.blasts.live() {
            let age = life.age();
            let ramp = fireball_ramp(age);
            blast
                .core
                .set_scale(Vector3::splat(blast.radius * fireball_scale(age)));
            blast.core_material.set_albedo(ramp * CORE_BOOST);
            // The shell runs a little wider and a little longer than the core,
            // so the fireball has an edge that is cooling rather than a
            // silhouette that simply shrinks.
            blast
                .halo
                .set_scale(Vector3::splat(blast.radius * (0.9 + 0.25 * age)));
            blast.halo_material.set_albedo(ramp * (0.55 * (1.0 - age)));
        }

        self.rings.step(dt);
        for (ring, life) in self.rings.live() {
            let age = life.age();
            let ease = 1.0 - (1.0 - age) * (1.0 - age);
            let reach = ring.radius * ring.reach * (0.2 + 0.8 * ease);
            ring.node.set_scale(Vector3::new(reach, 1.0, reach));
            ring.material
                .set_albedo(ring.tint * ((1.0 - age).powf(1.7) * 1.5));
        }

        self.markers.step(dt);
        for (marker, life) in self.markers.live() {
            let filled = telegraph_fill(life.age() * marker.duration, marker.duration);
            let pulse = telegraph_pulse(life.remaining());
            let reach = (marker.radius * filled).max(0.02);
            marker.fill.set_scale(Vector3::new(reach, 1.0, reach));
            // The fill is a wash of light inside the ring and the ring is the
            // edge of it, and both get louder as the strike arrives: a warning
            // that only got brighter at the end would read as a decoration for
            // most of its life, which is the opposite of a warning. All three
            // are kept near white rather than above it — this is additive, and
            // a marker the player can be standing inside has to stay a colour
            // at arm's length instead of a hole in the picture.
            marker.fill_material.set_albedo(bright(
                Color::from_rgba(1.0, 0.32, 0.22, 1.0),
                0.5 + 0.5 * pulse,
            ));
            marker.ring_material.set_albedo(bright(
                Color::from_rgba(1.0, 0.42, 0.3, 1.0),
                0.7 + 0.5 * pulse,
            ));
            // The curtain climbs as the strike arrives, so the warning grows
            // into the space the player is standing in rather than only getting
            // brighter at their feet.
            marker.curtain.set_scale(Vector3::new(
                marker.radius,
                0.5 + 1.1 * pulse,
                marker.radius,
            ));
            marker.curtain_material.set_albedo(bright(
                Color::from_rgba(1.0, 0.38, 0.26, 1.0),
                0.4 + 0.5 * pulse,
            ));
        }

        self.chunks.step(dt);
        for (chunk, life) in self.chunks.live() {
            chunk.fly(dt);
            chunk.shrink(life);
        }

        self.lights.step(dt);
        for (light, life) in self.lights.live() {
            light
                .node
                .set("light_energy", &light_energy(light.peak, life).to_variant());
        }
    }

    /// How many effects are currently live, for diagnostics.
    pub fn live_count(&self) -> usize {
        self.tracers.live_count()
            + self.flares.live_count()
            + self.glows.live_count()
            + self.sparks.live_count()
            + self.dust.live_count()
            + self.smoke.live_count()
            + self.blasts.live_count()
            + self.rings.live_count()
            + self.markers.live_count()
            + self.chunks.live_count()
            + self.lights.live_count()
    }
}

/// One flare to fire. What it looks like is decided where it is asked for; the
/// pool only has to place it.
#[derive(Debug, Clone, Copy)]
struct FlareShot {
    at: Vector3,
    size: f32,
    life: f32,
    tint: Color,
    growth: f32,
    /// Roll about the view axis, so two flashes in a burst do not have their
    /// points in the same place.
    roll: f32,
}

fn place_flare(flare: &mut Flare, shot: FlareShot) {
    flare.node.set_position(shot.at);
    flare.node.set_rotation(Vector3::new(0.0, 0.0, shot.roll));
    flare.size = shot.size;
    flare.growth = shot.growth;
    flare.tint = shot.tint;
    flare.node.set_visible(true);
}

fn animate_flare(flare: &mut Flare, life: Life) {
    let size = flare.size * (1.0 + flare.growth * life.age());
    flare.node.set_scale(Vector3::splat(size));
    // Held bright for the first of its life and then dropped: a flare that
    // faded linearly from its first frame reads as a dim flare.
    flare
        .material
        .set_albedo(flare.tint * (life.remaining().powf(0.7) * FLARE_BOOST));
}

/// Flat on the floor, facing up. The band shapes are built facing +Z, so a
/// quarter turn about X lays one down.
fn flat() -> Vector3 {
    Vector3::new(-std::f32::consts::FRAC_PI_2, 0.0, 0.0)
}

/// Points a pooled emitter at a shot's settings and sets it going.
///
/// Every line here writes a property on a material or a node that already
/// exists — no allocation, and the particle count is left alone.
/// `amount_ratio` is the per-shot count for exactly that reason: it changes how
/// many of the emitter's particles are used without reallocating its buffer.
fn start_burst(burst: &mut Burst, at: Vector3, shot: &BurstShot) {
    let mat = &mut burst.material;
    mat.set_direction(shot.direction);
    mat.set_spread(shot.spread);
    mat.set_gravity(shot.gravity);
    mat.set_lifetime_randomness(shot.lifetime_randomness);
    mat.set_emission_sphere_radius(shot.emission_radius);
    // The velocity, scale and damping ramps are properties rather than plain
    // fields: Godot binds them through `set_param_min`, for which gdext
    // generates no typed setter, so they go in by name.
    mat.set("initial_velocity_min", &shot.speed_min.to_variant());
    mat.set("initial_velocity_max", &shot.speed_max.to_variant());
    mat.set("scale_min", &shot.scale_min.to_variant());
    mat.set("scale_max", &shot.scale_max.to_variant());
    mat.set("damping_min", &shot.damping.to_variant());
    mat.set("damping_max", &(shot.damping * 1.6).to_variant());

    let node = &mut burst.node;
    node.set_position(at);
    node.set_amount_ratio(shot.count.clamp(0.0, 1.0));
    node.set_visible(true);
    // `restart` resets the cycle, which is what makes a one-shot emitter fire
    // again; setting `emitting` afterwards keeps it true even if the last burst
    // had already finished and switched it off.
    node.restart();
    node.set_emitting(true);
}

/// A pool of particle emitters, all built from the same look but each with its
/// own process material: what a burst looks like is decided at fire time, and
/// two bursts at once must not fight over one material.
fn burst_pool(root: &mut Gd<Node3D>, count: usize, look: &BurstLook) -> Pool<Burst> {
    let items: Vec<Burst> = (0..count)
        .map(|_| {
            let mut material = ParticleProcessMaterial::new_gd();
            // The ramp carries the colour *and* the fade over a particle's
            // life, which is why the emitters can share a mesh whose vertex
            // colours have no fade in them.
            material.set_color_ramp(look.ramp);
            material.set_color(Color::WHITE);
            // The growth curve is bound through `set_param_texture`, which has
            // no typed setter, so it goes in by name with the rest of the
            // particle parameters.
            material.set("scale_curve", &look.grow.to_variant());
            material.set_use_scale_3d(true);
            material.set_scale_3d_min(Vector3::splat(1.0));
            material.set_scale_3d_max(Vector3::splat(1.0));
            material.set_emission_shape(
                godot::classes::particle_process_material::EmissionShape::SPHERE,
            );
            if look.curls {
                // Rising smoke curls; a column of perfect spheres does not.
                material.set_turbulence_enabled(true);
                material.set("turbulence_noise_strength", &0.7f32.to_variant());
                material.set("turbulence_noise_scale", &2.6f32.to_variant());
                material.set(
                    "turbulence_noise_speed",
                    &Vector3::new(0.1, 0.5, 0.1).to_variant(),
                );
            }

            let mut node = GpuParticles3D::new_alloc();
            node.set_amount(look.particles);
            node.set_lifetime(look.lifetime);
            node.set_one_shot(true);
            node.set_explosiveness_ratio(1.0);
            node.set_randomness_ratio(0.5);
            // World space: an emitter reused across the yard must not drag its
            // last burst along with it.
            node.set_use_local_coordinates(false);
            node.set_process_material(&material);
            node.set_draw_pass_mesh(0, look.mesh);
            // The draw mesh carries no material of its own: one material on the
            // node covers every particle it draws.
            node.set_material_override(&look.material);
            if look.stretched {
                // Sparks are stretched along their own velocity, which is the
                // whole reason they are drawn as slivers rather than as dots.
                node.set_transform_align(
                    godot::classes::gpu_particles_3d::TransformAlign::Z_BILLBOARD_Y_TO_VELOCITY,
                );
            }
            // Particle systems are culled by this box, and the default one is
            // small enough that an explosion vanishes when its centre leaves
            // the screen — which is precisely when most of it is still visible.
            node.set_visibility_aabb(Aabb::new(
                Vector3::new(-16.0, -8.0, -16.0),
                Vector3::new(32.0, 24.0, 32.0),
            ));
            node.set_emitting(false);
            node.set_visible(false);
            root.add_child(&node);

            Burst { node, material }
        })
        .collect();
    Pool::new(items)
}

/// A hidden mesh node, parented to the effects root. Built once, at load.
fn mesh_node(
    root: &mut Gd<Node3D>,
    mesh: &Gd<ArrayMesh>,
    material: &Gd<StandardMaterial3D>,
) -> Gd<MeshInstance3D> {
    let mut node = MeshInstance3D::new_alloc();
    node.set_mesh(mesh);
    node.set_material_override(material);
    // Effects do not cast: a transparent shape casts the shadow of a rectangle
    // nobody can see, and the debris boxes are the only solid thing here.
    node.set_cast_shadows_setting(ShadowCastingSetting::OFF);
    node.set_visible(false);
    root.add_child(&node);
    node
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for a pooled node, so the pool's rules can be tested without
    /// an engine to build one in.
    #[derive(Default)]
    struct Dummy {
        hidden: bool,
    }

    impl Visual for Dummy {
        fn hide(&mut self) {
            self.hidden = true;
        }
    }

    fn dummy_pool(size: usize) -> Pool<Dummy> {
        Pool::new((0..size).map(|_| Dummy::default()).collect())
    }

    #[test]
    fn an_effect_ages_from_nothing_to_all_of_itself() {
        let mut life = Life::new(0.4);
        assert_eq!(life.age(), 0.0);
        assert_eq!(life.remaining(), 1.0);
        life.step(0.1);
        assert!((life.age() - 0.25).abs() < 1e-6);
        life.step(0.3);
        assert_eq!(life.age(), 1.0);
        assert_eq!(life.remaining(), 0.0);
    }

    #[test]
    fn a_step_past_the_end_expires_once_and_never_wraps() {
        // A frame long enough to run an effect out twice over must expire it
        // once. An age that wrapped past its end would send a tracer back to
        // the muzzle and make an explosion start growing again.
        let mut life = Life::new(0.05);
        assert!(life.step(0.2), "the step that runs it out must say so");
        assert!(life.is_dead());
        assert_eq!(life.age(), 1.0);
        assert!(!life.step(0.2), "a dead effect cannot die again");
        assert_eq!(life.age(), 1.0);
    }

    #[test]
    fn a_full_pool_drops_the_new_effect_rather_than_growing() {
        let mut pool = dummy_pool(3);
        for _ in 0..3 {
            assert!(pool.claim(1.0).is_some());
        }
        assert!(
            pool.claim(1.0).is_none(),
            "the fourth effect must be dropped"
        );
        assert_eq!(pool.capacity(), 3, "and the pool must not have grown");
        assert_eq!(pool.live_count(), 3);
    }

    #[test]
    fn a_slot_that_runs_out_is_reused() {
        let mut pool = dummy_pool(1);
        pool.claim(0.1).expect("the pool starts empty");
        assert!(pool.claim(1.0).is_none());
        pool.step(0.1);
        assert_eq!(pool.live_count(), 0);
        assert!(
            pool.claim(1.0).is_some(),
            "the slot that expired is free again"
        );
        assert_eq!(pool.capacity(), 1);
    }

    #[test]
    fn stepping_a_pool_switches_off_what_ran_out() {
        let mut pool = dummy_pool(2);
        pool.claim(0.2);
        pool.claim(0.4);
        pool.step(0.25);
        assert_eq!(pool.live_count(), 1);
        let hidden = pool.slots.iter().filter(|s| s.item.hidden).count();
        assert_eq!(hidden, 1, "the expired node must be hidden, exactly once");
    }

    #[test]
    fn a_light_can_be_taken_from_a_flash_that_matters_less() {
        // The light pool is the one pool that does not simply drop: an
        // explosion arriving while muzzle flashes hold every slot still has to
        // be lit.
        let mut pool = dummy_pool(1);
        pool.claim_with(1.0, LIGHT_MUZZLE).expect("a free slot");
        assert!(
            pool.claim_with(1.0, LIGHT_HIT).is_none(),
            "an equally important flash must not take a light"
        );
        let slot = pool
            .claim_with(1.0, LIGHT_BLAST)
            .expect("a blast takes the light");
        assert_eq!(slot.priority, LIGHT_BLAST);
        assert_eq!(pool.capacity(), 1, "and the pool still has one light");
    }

    #[test]
    fn variation_is_deterministic_and_stays_in_range() {
        // The same seed has to give the same burst every run, or a screenshot
        // stops being evidence; and every value stays in the range it is used
        // for, because a negative spread or a size below zero is a visible bug
        // rather than an exception.
        let mut a = Rng::new(RNG_SEED);
        let mut b = Rng::new(RNG_SEED);
        for _ in 0..64 {
            let (x, y) = (a.unit(), b.unit());
            assert_eq!(x, y, "the same seed must give the same sequence");
            assert!((0.0..1.0).contains(&x));
        }
        let mut rng = Rng::new(RNG_SEED);
        for _ in 0..64 {
            assert!((-1.0..1.0).contains(&rng.signed()));
            assert!((0.7..1.3).contains(&rng.around(0.3)));
            assert!((2.0..5.0).contains(&rng.range(2.0, 5.0)));
        }
    }

    #[test]
    fn successive_shots_are_not_the_same_shot() {
        // A burst that used one value for every round would strobe: the flash
        // the same size and the same tint seven times a second reads as a
        // rendering fault rather than as fire.
        let mut rng = Rng::new(RNG_SEED);
        let sizes: Vec<f32> = (0..8).map(|_| 0.6 * rng.around(0.28)).collect();
        let biggest = sizes.iter().cloned().fold(f32::MIN, f32::max);
        let smallest = sizes.iter().cloned().fold(f32::MAX, f32::min);
        assert!(
            biggest - smallest > 0.05,
            "eight flashes were all the same size"
        );
    }

    #[test]
    fn a_muzzle_flash_is_always_hot() {
        let mut rng = Rng::new(RNG_SEED);
        for _ in 0..32 {
            let tint = muzzle_tint(&mut rng);
            assert!(
                tint.r >= tint.g && tint.g >= tint.b,
                "a flash is never blue"
            );
            assert!(tint.r >= 0.99 && tint.b <= 0.9);
            assert_eq!(tint.a, 1.0);
        }
    }

    #[test]
    fn a_tracer_travels_at_the_speed_of_the_round_it_stands_for() {
        let from = Vector3::new(1.0, 2.0, 3.0);
        let direction = Vector3::new(0.0, 0.0, -1.0);
        assert_eq!(tracer_position(from, direction, 0.0, 300.0), from);
        let after = tracer_position(from, direction, 0.1, 300.0);
        assert!((after - Vector3::new(1.0, 2.0, -27.0)).length() < 1e-3);
        // An age from before the shot is clamped rather than run backwards.
        assert_eq!(tracer_position(from, direction, -1.0, 300.0), from);
    }

    #[test]
    fn an_impact_only_cuts_the_rounds_that_went_through_it() {
        let from = Vector3::ZERO;
        let direction = Vector3::new(0.0, 0.0, -1.0);
        let travelled = 30.0;
        assert!(
            tracer_passes(from, direction, travelled, Vector3::new(0.4, 0.3, -12.0)),
            "a point on the path, within reach, is this round's impact"
        );
        assert!(
            !tracer_passes(from, direction, travelled, Vector3::new(0.0, 0.0, 9.0)),
            "a point behind the muzzle is not"
        );
        assert!(
            !tracer_passes(from, direction, travelled, Vector3::new(0.0, 0.0, -60.0)),
            "a point past where the round has got to is not"
        );
        assert!(
            !tracer_passes(from, direction, travelled, Vector3::new(6.0, 0.0, -12.0)),
            "a point across the yard from the path is not"
        );
    }

    #[test]
    fn facing_points_down_the_line_of_flight() {
        let direction = Vector3::new(0.3, 0.2, -0.9).normalized();
        let basis = facing(direction);
        let cols = basis.to_cols();
        for col in cols {
            assert!((col.length() - 1.0).abs() < 1e-4, "the basis is not square");
        }
        assert!(
            cols[0].dot(cols[1]).abs() < 1e-4,
            "the basis is not orthogonal"
        );
        // Godot's forward is -Z, and the streak is built with its tail on +Z.
        assert!((cols[2] + direction).length() < 1e-4);
    }

    #[test]
    fn facing_survives_a_shot_straight_up() {
        // A round fired at the sky has no horizontal component to build the
        // rest of the basis from, and a NaN in a transform takes the node with
        // it rather than drawing it at a strange angle.
        for direction in [Vector3::UP, Vector3::DOWN] {
            let basis = facing(direction);
            assert!(basis.is_finite());
            for col in basis.to_cols() {
                assert!((col.length() - 1.0).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn a_hard_surface_sparks_and_a_soft_one_does_not() {
        assert_eq!(impact_look("steel"), ImpactLook::Hard);
        assert_eq!(impact_look("plating"), ImpactLook::Hard);
        assert_eq!(impact_look("glass"), ImpactLook::Hard);
        assert_eq!(impact_look("steel-dark"), ImpactLook::Hard);
        assert_eq!(impact_look("concrete"), ImpactLook::Soft);
        assert_eq!(impact_look("rust"), ImpactLook::Soft);
        // A surface the renderer has never heard of is dust rather than sparks:
        // a new prop that sparked would be claiming to be metal it is not.
        assert_eq!(impact_look("sandbag"), ImpactLook::Soft);
    }

    #[test]
    fn the_telegraph_fills_exactly_as_the_strike_lands() {
        assert_eq!(telegraph_fill(0.0, 1.2), 0.0);
        assert_eq!(telegraph_fill(1.2, 1.2), 1.0);
        assert_eq!(telegraph_fill(9.0, 1.2), 1.0, "a late strike is still full");
        assert_eq!(telegraph_fill(0.5, 0.0), 1.0, "no warning is already over");
        // Monotone, and a little ahead of linear: the last third of a warning
        // is the part anybody acts on.
        let mut last = -1.0;
        for step in 0..=20 {
            let filled = telegraph_fill(1.2 * step as f32 / 20.0, 1.2);
            assert!(filled >= last);
            last = filled;
        }
        assert!(telegraph_fill(0.6, 1.2) > 0.5);
    }

    #[test]
    fn a_telegraph_gets_urgent_as_the_strike_arrives() {
        assert!(telegraph_pulse(1.0) < 0.6, "it starts calm");
        assert!(telegraph_pulse(0.0) > 1.5, "and ends loud");
        let mut last = 0.0;
        for step in 0..=20 {
            let pulse = telegraph_pulse(1.0 - step as f32 / 20.0);
            assert!(pulse >= last, "the warning never gets quieter");
            last = pulse;
        }
    }

    #[test]
    fn a_fireball_cools_and_never_outgrows_its_blast() {
        let mut last_green = f32::MAX;
        let mut last_alpha = f32::MAX;
        for step in 0..=20 {
            let age = step as f32 / 20.0;
            let colour = fireball_ramp(age);
            assert!((0.0..=1.0).contains(&colour.r));
            assert!((0.0..=1.0).contains(&colour.g));
            assert!((0.0..=1.0).contains(&colour.b));
            assert!((0.0..=1.0).contains(&colour.a));
            assert!(colour.g <= last_green, "a fireball does not reheat");
            assert!(colour.a <= last_alpha, "and it does not come back");
            last_green = colour.g;
            last_alpha = colour.a;
            // Size is capped at the event's radius: that radius is the sphere
            // the splash damage was computed in, and a fireball visibly larger
            // than it promises damage the simulation never dealt.
            let scale = fireball_scale(age);
            assert!((0.42..=1.0).contains(&scale));
        }
        assert_eq!(fireball_ramp(1.0).a, 0.0);
        assert!(fireball_ramp(0.0).b > 0.7, "the core starts white hot");
    }

    #[test]
    fn a_light_is_full_the_instant_it_fires() {
        let mut life = Life::new(0.4);
        assert!((light_energy(6.0, life) - 6.0).abs() < 1e-6);
        let mut last = f32::MAX;
        for _ in 0..8 {
            life.step(0.05);
            let energy = light_energy(6.0, life);
            assert!(energy <= last, "a discharge decays and does not flicker up");
            last = energy;
        }
        life.step(1.0);
        assert_eq!(light_energy(6.0, life), 0.0);
    }

    #[test]
    fn a_machines_death_is_sized_by_the_machine() {
        // The label on a destroy event is the archetype's own label, which is
        // what the mission scores a kill from: the renderer reads the same
        // string rather than inventing a size for the machine it just watched
        // come apart.
        let skirmisher = death_radius("skirmisher");
        let boss = death_radius("boss");
        assert!(boss > skirmisher, "the Patriarch is the bigger explosion");
        assert!(skirmisher > 1.0);
        assert!(boss < 12.0, "and not a tactical warhead");
        // The player is named rather than labelled, and the player is the only
        // other machine that can be destroyed.
        let player = death_radius("ASHFRAME");
        assert!(player > 1.0 && player.is_finite());
    }

    #[test]
    fn debris_arcs_and_then_settles_on_the_floor() {
        let mut position = Vector3::new(0.0, 4.0, 0.0);
        let mut velocity = Vector3::new(3.0, 9.0, 0.0);
        let mut peak = position.y;
        let mut landed = false;
        for _ in 0..400 {
            ballistics(&mut position, &mut velocity, 1.0 / 120.0, FLOOR_Y);
            peak = peak.max(position.y);
            if position.y <= FLOOR_Y + 1e-4 {
                landed = true;
            }
            assert!(
                position.y >= FLOOR_Y,
                "debris does not sink through the yard"
            );
        }
        assert!(peak > 4.0, "a chunk thrown upwards rises before it falls");
        assert!(landed, "and it comes down");
        assert!(
            position.x > 0.0,
            "it keeps the horizontal speed it was thrown with"
        );
        assert!(velocity.y.abs() < 0.1, "and it is at rest by the end");
    }

    #[test]
    fn every_burst_is_the_right_way_round() {
        // A minimum above its maximum is clamped rather than reported by the
        // particle system, which turns a shaped burst into a uniform one and
        // gives nobody a clue why.
        let shots = [
            BurstShot::sparks(Vector3::UP, 13.0, 0.85),
            BurstShot::sparks(Vector3::new(0.0, 1.0, -1.0).normalized(), 10.0, 0.7),
            BurstShot::dust(Vector3::UP, 6.0, 0.9),
            BurstShot::smoke(Vector3::UP, 3.2, 1.0),
        ];
        for shot in shots {
            assert!(shot.sane(), "an impossible burst: {shot:?}");
        }
    }

    #[test]
    fn a_tracer_streak_tapers_from_a_hot_head_to_a_cold_tail() {
        let shape = streak_shape();
        let section = |z: f32| -> Vec<(&Vector3, &Color)> {
            shape
                .vertices
                .iter()
                .zip(&shape.colors)
                .filter(|(v, _)| (v.z - z).abs() < 1e-4)
                .collect()
        };
        let head = section(0.0);
        let tail = section(1.0);
        let width = |set: &[(&Vector3, &Color)]| {
            set.iter()
                .map(|(v, _)| (v.x * v.x + v.y * v.y).sqrt())
                .fold(0.0f32, f32::max)
        };
        let alpha =
            |set: &[(&Vector3, &Color)]| set.iter().map(|(_, c)| c.a).fold(0.0f32, f32::max);
        assert!(!head.is_empty() && !tail.is_empty());
        assert!(
            width(&head) > width(&tail) * 2.0,
            "the streak does not taper"
        );
        assert_eq!(alpha(&head), 1.0, "the head is the hot end");
        assert_eq!(alpha(&tail), 0.0, "and the tail has gone out");
        // Nothing is drawn in front of the head: the node sits at the round.
        assert!(shape.vertices.iter().all(|v| v.z >= -1e-6));
    }

    #[test]
    fn a_glow_fades_from_the_middle_out() {
        let shape = glow_shape(16, 1.0);
        let profile = shape.profile();
        assert!(profile.iter().any(|(r, a)| *r < 1e-6 && *a > 0.9));
        assert!(profile.iter().any(|(r, a)| *r > 0.99 && *a <= 1e-6));
        // Sampled from the middle outwards, brightness never rises: a glow with
        // a bright rim would read as a ring.
        let mut sorted = profile;
        sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut last = f32::MAX;
        for (_, alpha) in sorted {
            assert!(alpha <= last + 1e-6);
            last = last.min(alpha);
        }
    }

    #[test]
    fn a_telegraph_ring_is_hollow_and_its_fill_has_a_wavefront() {
        let ring = ring_shape(24, 1.0, 0.84);
        let profile = ring.profile();
        let innermost = profile.iter().map(|(r, _)| *r).fold(f32::MAX, f32::min);
        assert!(innermost > 0.8, "the ring must not be a disc");
        assert!(
            profile.iter().all(|(r, _)| *r <= 1.001),
            "and it must not spill past its radius"
        );

        let fill = fill_shape(24, 1.0);
        let profile = fill.profile();
        let brightest = profile.iter().fold(
            (0.0, 0.0),
            |best, (r, a)| if *a > best.1 { (*r, *a) } else { best },
        );
        assert!(
            brightest.0 > 0.9,
            "the fill's brightest band is its edge, so that it reads as arriving"
        );
        assert!(
            profile.iter().any(|(r, a)| *r < 1e-6 && *a > 0.05),
            "and its middle is tinted rather than empty"
        );
    }

    #[test]
    fn a_spark_is_a_sliver_that_ends_in_a_point() {
        let shape = spark_shape(0.05, 0.55);
        assert_eq!(shape.vertices.len(), 4);
        let head = shape
            .vertices
            .iter()
            .zip(&shape.colors)
            .find(|(v, _)| v.y > 0.0)
            .expect("a leading end");
        assert_eq!(head.1.a, 1.0);
        let tail = shape
            .vertices
            .iter()
            .zip(&shape.colors)
            .find(|(v, _)| v.y < 0.0)
            .expect("a trailing end");
        assert_eq!(tail.1.a, 0.0);
        assert!(
            shape.vertices.iter().all(|v| v.x.abs() <= 0.05 + 1e-6),
            "a spark is thin"
        );
    }
}
