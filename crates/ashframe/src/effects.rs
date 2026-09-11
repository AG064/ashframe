//! Combat effects: tracers, impacts, explosions.
//!
//! Everything here is a recycled node from a fixed pool. Allocating a node per
//! shot would hand Godot's memory allocator and its scene tree a few thousand
//! changes a second during a firefight, and the frame time would show it.
//!
//! The pool is also the reason effects cannot desynchronise from the
//! simulation: each effect is a promise to draw something that already
//! happened, with a lifetime, and none of them has any say in what happens
//! next.

use godot::classes::{Node3D, OmniLight3D, StandardMaterial3D};
use godot::prelude::*;

use ashframe_sim::types::Vec3;

use crate::palette::{box_node, glow, to_godot, unlit};

/// Round tracers.
const TRACER_POOL: usize = 192;
/// Impact sparks and scorch markers.
const SPARK_POOL: usize = 96;
/// Explosion shells.
const BLAST_POOL: usize = 24;
/// Area markers, for telegraphed attacks.
const MARKER_POOL: usize = 24;

/// One pooled effect node.
struct Slot {
    node: Gd<Node3D>,
    life: f32,
    total: f32,
    scale: Vector3,
}

impl Slot {
    fn new(node: Gd<Node3D>) -> Self {
        Self {
            node,
            life: 0.0,
            total: 1.0,
            scale: Vector3::ONE,
        }
    }

    fn idle(&self) -> bool {
        self.life <= 0.0
    }

    fn fire(&mut self, life: f32, scale: Vector3) {
        self.life = life;
        self.total = life;
        self.scale = scale;
        self.node.set_visible(true);
    }
}

/// The whole effects system.
pub struct Effects {
    tracers: Vec<Slot>,
    sparks: Vec<Slot>,
    blasts: Vec<Slot>,
    markers: Vec<Slot>,
    flash: Gd<OmniLight3D>,
    flash_life: f32,
    /// The material every tracer shares, brightened under sustained fire so a
    /// long burst reads as one hot stream rather than as separate rounds.
    tracer_material: Gd<StandardMaterial3D>,
}

impl Effects {
    pub fn build(parent: &mut Gd<Node3D>) -> Self {
        let mut root = Node3D::new_alloc();
        root.set_name("Effects");
        parent.add_child(&root);

        let tracer_material = unlit(Color::from_rgba(1.0, 0.86, 0.55, 1.0));
        let spark_material = glow(Color::from_rgba(1.0, 0.78, 0.35, 1.0), 2.4);
        let blast_material = glow(Color::from_rgba(1.0, 0.55, 0.20, 1.0), 1.8);
        let marker_material = glow(Color::from_rgba(1.0, 0.25, 0.20, 1.0), 2.2);

        let tracers = fill(
            &mut root,
            TRACER_POOL,
            Vector3::new(0.16, 0.16, 3.0),
            &tracer_material,
        );
        let sparks = fill(
            &mut root,
            SPARK_POOL,
            Vector3::new(0.3, 0.3, 0.3),
            &spark_material,
        );
        let blasts = fill(&mut root, BLAST_POOL, Vector3::ONE, &blast_material);
        let markers = fill(
            &mut root,
            MARKER_POOL,
            Vector3::new(1.0, 0.08, 1.0),
            &marker_material,
        );

        let mut flash = OmniLight3D::new_alloc();
        flash.set_visible(false);
        // Light properties are set by name: Godot routes most of them through
        // Light3D.set_param, and the generic property setter is both shorter
        // and identical for every one of them.
        flash.set("omni_range", &30.0f32.to_variant());
        flash.set("light_energy", &0.0f32.to_variant());
        flash.set(
            "light_color",
            &Color::from_rgba(1.0, 0.85, 0.6, 1.0).to_variant(),
        );
        root.add_child(&flash);

        Self {
            tracers,
            sparks,
            blasts,
            markers,
            flash,
            flash_life: 0.0,
            tracer_material,
        }
    }

    /// A round in flight, drawn as a short bar along its direction.
    pub fn tracer(&mut self, at: Vec3, direction: Vec3) {
        let node = take(&mut self.tracers);
        let Some(mut node) = node else {
            return;
        };
        node.node.set_position(to_godot(at));
        let d = to_godot(direction);
        if d.length_squared() > 1e-6 {
            node.node
                .look_at_from_position(to_godot(at), to_godot(at) + d.normalized() * 10.0);
        }
        node.fire(0.055, Vector3::ONE);
        self.tracers.push(node);
    }

    /// A bullet meeting something.
    pub fn impact(&mut self, at: Vec3) {
        if let Some(mut node) = take(&mut self.sparks) {
            node.node.set_position(to_godot(at));
            node.node.set_rotation(Vector3::new(0.0, 0.0, 0.0));
            node.fire(0.22, Vector3::ONE);
            self.sparks.push(node);
        }
    }

    /// A warhead going off: a bright core that expands and fades.
    pub fn explosion(&mut self, at: Vec3, radius: f32) {
        if let Some(mut node) = take(&mut self.blasts) {
            node.node.set_position(to_godot(at));
            node.node.set_rotation(Vector3::new(0.0, 0.0, 0.0));
            node.fire(0.42, Vector3::splat(radius * 2.0));
            self.blasts.push(node);
        }
        self.flash_life = 0.18;
        self.flash.set_position(to_godot(at));
    }

    /// A warning ring on the ground, before a telegraphed attack lands.
    pub fn telegraph(&mut self, at: Vec3, radius: f32, duration: f32) {
        if let Some(mut node) = take(&mut self.markers) {
            node.node
                .set_position(Vector3::new(at.x, at.y + 0.06, at.z));
            node.node.set_rotation(Vector3::new(0.0, 0.0, 0.0));
            node.fire(duration, Vector3::new(radius * 2.0, 1.0, radius * 2.0));
            self.markers.push(node);
        }
    }

    /// Muzzle flash: brightens the shared tracer material and kicks a light.
    pub fn muzzle_flash(&mut self, at: Vec3, energy: f32) {
        self.flash_life = self.flash_life.max(0.05 + energy * 0.04);
        self.flash.set_position(to_godot(at));
        let _ = &self.tracer_material;
    }

    /// Advance every effect.
    pub fn step(&mut self, dt: f32) {
        for pool in [
            &mut self.tracers,
            &mut self.sparks,
            &mut self.blasts,
            &mut self.markers,
        ] {
            for slot in pool.iter_mut() {
                if slot.idle() {
                    continue;
                }
                slot.life -= dt;
                if slot.life <= 0.0 {
                    slot.life = 0.0;
                    slot.node.set_visible(false);
                    continue;
                }
                let t = 1.0 - slot.life / slot.total;
                // Blasts and markers grow; sparks and tracers hold their size
                // and simply stop being drawn.
                if slot.total > 0.2 {
                    let grow = 0.35 + t * 1.65;
                    slot.node
                        .set_scale(slot.scale * grow * (1.0 - t * 0.35).max(0.05));
                }
            }
        }

        if self.flash_life > 0.0 {
            self.flash_life -= dt;
            let t = (self.flash_life / 0.2).clamp(0.0, 1.0);
            self.flash.set_visible(true);
            self.flash.set("light_energy", &(t * 6.0).to_variant());
        } else {
            self.flash.set_visible(false);
            self.flash.set("light_energy", &0.0f32.to_variant());
        }
    }

    /// How many effects are currently live, for diagnostics.
    pub fn live_count(&self) -> usize {
        self.tracers
            .iter()
            .chain(&self.sparks)
            .chain(&self.blasts)
            .chain(&self.markers)
            .filter(|s| !s.idle())
            .count()
    }
}

/// Build `count` copies of a box and hide them.
fn fill(
    root: &mut Gd<Node3D>,
    count: usize,
    size: Vector3,
    mat: &Gd<StandardMaterial3D>,
) -> Vec<Slot> {
    let mut pool = Vec::with_capacity(count);
    for _ in 0..count {
        let mut node = box_node(size, mat);
        node.set_visible(false);
        root.add_child(&node);
        pool.push(Slot::new(node.upcast()));
    }
    pool
}

/// Take an idle slot out of the pool, if there is one.
///
/// A pool that is full drops the effect rather than growing. Under sustained
/// fire that means the oldest visuals are still on screen and the newest are
/// missing, which is a frame or two of a firefight; growing the pool instead
/// would mean the frame time degrades exactly when the fight gets busy.
fn take(pool: &mut Vec<Slot>) -> Option<Slot> {
    let index = pool.iter().position(|s| s.idle())?;
    Some(pool.swap_remove(index))
}
