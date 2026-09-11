//! The arena, drawn.
//!
//! Every solid in the yard is already described by [`ashframe_sim::arena`] as a
//! box with a surface tag, so the renderer builds exactly what the simulation
//! collides against — one list, two uses. That is the whole reason the
//! simulation carries props at all: a renderer that invented its own geometry
//! would drift from the collision, and a mech would eventually walk through
//! something that looked solid or stop against something that looked like air.

use godot::classes::{Node3D, StandardMaterial3D};
use godot::prelude::*;

use ashframe_sim::arena::{Arena, Prop};

use crate::palette::{
    box_node, from_hex, grid_material, ground_plane, material, surface_color, to_godot,
};

/// The built arena, kept so it can be cleared and rebuilt on a restart.
pub struct ArenaView {
    solids: Gd<Node3D>,
    markings: Gd<Node3D>,
}

impl ArenaView {
    /// Build the yard into `parent`.
    pub fn build(parent: &mut Gd<Node3D>, arena: &Arena) -> Self {
        let mut root = Node3D::new_alloc();
        root.set_name("Arena");

        let mut ground = Node3D::new_alloc();
        ground.set_name("Ground");
        root.add_child(&ground);

        // The apron is bigger than the playable half-extent so the horizon is
        // floor rather than void, and darker than the yard so the boundary is
        // legible without a wall to mark it.
        let apron = grid_material(surface_color::STEEL_DARK, 0.0, 0.95);
        let mut apron_node = ground_plane(ashframe_sim::config::world::HALF * 6.0, &apron);
        apron_node.set_position(Vector3::new(0.0, -0.05, 0.0));
        ground.add_child(&apron_node);

        // The deck is the concrete apron inside the boundary. It sits a
        // handspan above the outer ground so the edge of the playable area is
        // legible from the ground alone, without a wall to mark it.
        let deck = grid_material(surface_color::CONCRETE, 0.0, 0.92);
        let mut deck_node = ground_plane(ashframe_sim::config::world::HALF * 2.0, &deck);
        deck_node.set_position(Vector3::new(0.0, 0.02, 0.0));
        ground.add_child(&deck_node);

        let mut solids = Node3D::new_alloc();
        solids.set_name("Solids");
        root.add_child(&solids);

        // One material per surface, made once. A material per box would be
        // hundreds of them, and Godot would batch nothing.
        let mut cache: Vec<(&'static str, Gd<StandardMaterial3D>)> = Vec::new();
        for prop in &arena.props {
            let mat = match cache.iter().find(|(tag, _)| *tag == prop.tag) {
                Some((_, mat)) => mat.clone(),
                None => {
                    // Glass is the one surface that is not a plate: a grid on a
                    // window would read as a window with a grid painted on it.
                    let made = if prop.tag == "glass" {
                        material(from_hex(surface_color::GLASS), 0.9, 0.12)
                    } else {
                        grid_material(
                            surface_color::for_tag(prop.tag),
                            0.15,
                            if prop.tag == "hazard" { 0.55 } else { 0.72 },
                        )
                    };
                    cache.push((prop.tag, made.clone()));
                    made
                }
            };
            add_prop(&mut solids, prop, &mat);
        }

        let mut markings = Node3D::new_alloc();
        markings.set_name("Markings");
        root.add_child(&markings);
        for prop in &arena.markings {
            // Markings are painted stripes, so they get no grid: a grid on a
            // painted line reads as a tiled line.
            let mat = material(from_hex(surface_color::for_tag(prop.tag)), 0.0, 0.75);
            add_prop(&mut markings, prop, &mat);
        }

        parent.add_child(&root);
        Self { solids, markings }
    }

    /// How many boxes the yard is drawn from, for the diagnostics overlay.
    pub fn solid_count(&self) -> usize {
        self.solids.get_child_count() as usize + self.markings.get_child_count() as usize
    }
}

fn add_prop(parent: &mut Gd<Node3D>, prop: &Prop, mat: &Gd<StandardMaterial3D>) {
    let mut node = box_node(to_godot(prop.size), mat);
    node.set_position(to_godot(prop.at));
    if prop.rotation != 0.0 {
        node.set_rotation(Vector3::new(0.0, prop.rotation, 0.0));
    }
    // Godot culls by geometry, so a shadow-casting flag is honoured by simply
    // not letting the markings cast one.
    node.set_cast_shadows_setting(if prop.casts_shadow {
        godot::classes::geometry_instance_3d::ShadowCastingSetting::ON
    } else {
        godot::classes::geometry_instance_3d::ShadowCastingSetting::OFF
    });
    parent.add_child(&node);
}
