//! Shared visual vocabulary.
//!
//! Colours, materials and the small amount of maths the Godot layer needs that
//! the simulation deliberately does not carry. The simulation owns geometry and
//! tags; this decides what a `concrete` box looks like, and nothing here feeds
//! anything back into the game state.

use godot::builtin::{Array, PackedInt32Array, PackedVector2Array, PackedVector3Array, Variant};
use godot::classes::mesh::PrimitiveType;
use godot::classes::{ArrayMesh, BoxMesh, MeshInstance3D, StandardMaterial3D};
use godot::prelude::*;

/// The arena palette, as `0xRRGGBB`, matching the simulation's table.
///
/// Duplicated here rather than read from the simulation because the simulation
/// only carries a surface *name* on a prop; the renderer is what knows a name
/// has a colour. The two are pinned together by a test.
pub mod surface_color {
    pub const CONCRETE: u32 = 0x007c_786e;
    pub const STEEL: u32 = 0x0047_4e56;
    pub const STEEL_DARK: u32 = 0x002b_3036;
    pub const PLATING: u32 = 0x009a_a1a8;
    pub const RUST: u32 = 0x006d_4b38;
    pub const HAZARD: u32 = 0x00c9_971f;
    pub const GLASS: u32 = 0x0024_313a;

    /// The colour for a prop's tag, or the concrete grey for a tag the
    /// renderer has never heard of.
    ///
    /// A fallback rather than a panic: an arena that gains a surface should
    /// look unfinished, not refuse to start.
    pub fn for_tag(tag: &str) -> u32 {
        match tag {
            "concrete" => CONCRETE,
            "steel" => STEEL,
            "steel-dark" => STEEL_DARK,
            "plating" => PLATING,
            "rust" => RUST,
            "hazard" => HAZARD,
            "glass" => GLASS,
            _ => CONCRETE,
        }
    }
}

// The mech palette and the procedural proportion table used to live here. They
// are in `blender-mcp/parts/mechs.py` now: a model carries its own materials,
// and a colour in one repository that has to agree with a mesh in another is a
// colour that will one day stop agreeing with it.

/// A `0xRRGGBB` value as a Godot colour.
pub fn from_hex(rgb: u32) -> Color {
    Color::from_rgba8(
        ((rgb >> 16) & 0xff) as u8,
        ((rgb >> 8) & 0xff) as u8,
        (rgb & 0xff) as u8,
        255,
    )
}

/// A lit, non-emissive material.
pub fn material(color: Color, metallic: f32, roughness: f32) -> Gd<StandardMaterial3D> {
    let mut mat = StandardMaterial3D::new_gd();
    mat.set_albedo(color);
    mat.set_metallic(metallic);
    mat.set_roughness(roughness);
    mat
}

/// Metres per texture tile on world-mapped surfaces.
///
/// A four-metre cell reads at mech scale: the player is six metres tall, so a
/// cell is a little under the height of the machine, which is what the eye uses
/// to judge distance across the yard.
pub const TILE_METRES: f32 = 4.0;

/// A world-mapped grid texture.
///
/// The arena is boxes, and a field of untextured boxes has no scale at all: a
/// forty-metre wall and a four-metre crate are the same flat grey, and the
/// player has nothing to judge speed or distance against. A grid supplies the
/// missing ruler.
///
/// Generated rather than loaded: a texture file is an asset to ship, to license
/// and to keep in step with the palette, and this is arithmetic.
pub fn grid_texture(base: u32, cell_px: i32, size_px: i32) -> Gd<godot::classes::ImageTexture> {
    use godot::classes::Image;

    let colour = from_hex(base);
    // The seams are a shade of the surface rather than black, so they read as
    // the join between plates rather than as a line drawn on top of them.
    let dark = Color::from_rgba(colour.r * 0.66, colour.g * 0.66, colour.b * 0.66, 1.0);
    let light = Color::from_rgba(
        (colour.r * 1.12).min(1.0),
        (colour.g * 1.12).min(1.0),
        (colour.b * 1.12).min(1.0),
        1.0,
    );

    let mut image = Image::create(size_px, size_px, false, godot::classes::image::Format::RGB8)
        .expect("256 square is a valid image size");

    for y in 0..size_px {
        for x in 0..size_px {
            let on_seam = x % cell_px == 0 || y % cell_px == 0;
            // A hash rather than a random number generator: the speckle has to
            // be identical every run, or the yard would look different each
            // time the game started.
            // Wrapping throughout: this is a hash, and the whole point is that
            // it is allowed to overflow. Left unchecked it panics in a debug
            // build, which is a texture that crashes the game on load.
            let (ux, uy) = (x as u32, y as u32);
            let n =
                ux.wrapping_mul(73_856_093) ^ uy.wrapping_mul(19_349_663).wrapping_mul(0x2545_f491);
            let noise = ((n >> 16) & 0xff) as f32 / 255.0;
            let shade = 0.94 + noise * 0.12;
            let pixel = if on_seam {
                dark
            } else {
                Color::from_rgba(
                    (colour.r * shade).min(1.0),
                    (colour.g * shade).min(1.0),
                    (colour.b * shade).min(1.0),
                    1.0,
                )
            };
            image.set_pixel(x, y, pixel);
        }
    }

    // One cell in four is a lighter plate. Breaking the regularity is what stops
    // the eye counting tiles and lets it read the result as a floor.
    let cells = size_px / cell_px;
    for by in 0..cells {
        for bx in 0..cells {
            if (bx * 7 + by * 13) % 4 != 0 {
                continue;
            }
            for y in 1..cell_px {
                for x in 1..cell_px {
                    let (px, py) = (bx * cell_px + x, by * cell_px + y);
                    if px >= size_px || py >= size_px {
                        continue;
                    }
                    let under = image.get_pixel(px, py);
                    image.set_pixel(
                        px,
                        py,
                        Color::from_rgba(
                            under.r * 0.55 + light.r * 0.45,
                            under.g * 0.55 + light.g * 0.45,
                            under.b * 0.55 + light.b * 0.45,
                            1.0,
                        ),
                    );
                }
            }
        }
    }

    godot::classes::ImageTexture::create_from_image(&image).expect("the image was just created")
}

/// A grid-mapped material.
///
/// World-space triplanar mapping, which is the point: one material per surface
/// covers every box in the yard at the same physical tile size, however large
/// the box. Per-box materials with hand-fitted UV scales would be a material per
/// prop, and Godot would batch none of them.
pub fn grid_material(base: u32, metallic: f32, roughness: f32) -> Gd<StandardMaterial3D> {
    // White albedo, because Godot multiplies the colour by the texture: leaving
    // the surface colour in both places squares it, and a concrete deck comes
    // out nearly black.
    let mut mat = material(Color::WHITE, metallic, roughness);
    let texture = grid_texture(base, 32, 256);
    mat.set_texture(
        godot::classes::base_material_3d::TextureParam::ALBEDO,
        &texture,
    );
    // By name: gdext generates accessors for the values it knows are enums, and
    // these two are plain booleans behind a property.
    mat.set("uv1_triplanar", &true.to_variant());
    mat.set("uv1_world_triplanar", &true.to_variant());
    let scale = 1.0 / TILE_METRES;
    mat.set_uv1_scale(Vector3::new(scale, scale, scale));
    mat
}

// The `glow` helper used to live here. The effects module builds its own
// materials now -- it needs to drive emission from a vertex-colour ramp and to
// switch emission back off on the same material, which a general-purpose
// glowing material cannot express. Keeping a second, unused way to make
// something glow would only be a way for the two to drift apart.

/// An unlit material, for tracers and markers where shading would only blur the
/// shape at the distance the player actually sees them.
pub fn unlit(color: Color) -> Gd<StandardMaterial3D> {
    let mut mat = StandardMaterial3D::new_gd();
    mat.set_albedo(color);
    mat.set_shading_mode(godot::classes::base_material_3d::ShadingMode::UNSHADED);
    mat.set("emission_enabled", &true.to_variant());
    mat.set_emission(color);
    mat
}

/// A box, ready to be dropped into the tree.
pub fn box_node(size: Vector3, mat: &Gd<StandardMaterial3D>) -> Gd<MeshInstance3D> {
    let mut mesh = BoxMesh::new_gd();
    mesh.set_size(size);
    let mut node = MeshInstance3D::new_alloc();
    node.set_mesh(&mesh);
    node.set_material_override(mat);
    node
}

/// A flat plane as a mesh, used for the ground and for the shadow-catching
/// apron around the arena.
pub fn ground_plane(size: f32, mat: &Gd<StandardMaterial3D>) -> Gd<MeshInstance3D> {
    let mut arrays: Array<Variant> = Array::new();
    arrays.resize(
        godot::classes::mesh::ArrayType::MAX.ord() as usize,
        &Variant::nil(),
    );

    let half = size * 0.5;
    let normal = Vector3::UP;
    let points = PackedVector3Array::from(&[
        Vector3::new(-half, 0.0, -half),
        Vector3::new(half, 0.0, -half),
        Vector3::new(half, 0.0, half),
        Vector3::new(-half, 0.0, half),
    ]);
    let normals = PackedVector3Array::from(&[normal, normal, normal, normal]);
    // Scaled to the plane so a repeated texture would tile once across it; the
    // materials here have no texture, but a UV that does not lie costs nothing.
    let uvs = PackedVector2Array::from(&[
        Vector2::new(0.0, 0.0),
        Vector2::new(size, 0.0),
        Vector2::new(size, size),
        Vector2::new(0.0, size),
    ]);
    // Clockwise seen from above, which is Godot's front-face convention. The
    // reverse winding leaves the plane facing downward, and a floor whose front
    // face points at the earth is culled: the yard appears to stand on the
    // sky's ground gradient.
    let indices = PackedInt32Array::from(&[0, 1, 2, 0, 2, 3]);

    arrays.set(
        godot::classes::mesh::ArrayType::VERTEX.ord() as usize,
        &points.to_variant(),
    );
    arrays.set(
        godot::classes::mesh::ArrayType::NORMAL.ord() as usize,
        &normals.to_variant(),
    );
    arrays.set(
        godot::classes::mesh::ArrayType::TEX_UV.ord() as usize,
        &uvs.to_variant(),
    );
    arrays.set(
        godot::classes::mesh::ArrayType::INDEX.ord() as usize,
        &indices.to_variant(),
    );

    let mut mesh = ArrayMesh::new_gd();
    mesh.add_surface_from_arrays(PrimitiveType::TRIANGLES, &arrays);

    let mut node = MeshInstance3D::new_alloc();
    node.set_mesh(&mesh);
    node.set_material_override(mat);
    // The ground does not cast. A single quad nine hundred metres across is
    // most of the shadow map, and at that texel size the surface shadows itself
    // into large soft blotches; nothing is underneath it to receive a shadow
    // anyway. It still *receives*, which is what puts the mechs' shadows on it.
    node.set_cast_shadows_setting(godot::classes::geometry_instance_3d::ShadowCastingSetting::OFF);
    node
}

/// The simulation's `Vec3` as a Godot vector.
pub fn to_godot(v: ashframe_sim::types::Vec3) -> Vector3 {
    Vector3::new(v.x, v.y, v.z)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_named_surface_has_a_colour_of_its_own() {
        // A tag that fell through to the fallback would render a hazard stripe
        // as concrete, which is the kind of thing nobody notices until the yard
        // is a uniform grey.
        let tags = [
            "concrete",
            "steel",
            "steel-dark",
            "plating",
            "rust",
            "hazard",
            "glass",
        ];
        let mut seen = Vec::new();
        for tag in tags {
            let colour = surface_color::for_tag(tag);
            assert!(
                !seen.contains(&colour),
                "{tag} shares a colour with an earlier surface"
            );
            seen.push(colour);
        }
    }

    #[test]
    fn the_renderer_palette_matches_the_simulation() {
        use ashframe_sim::arena::palette as sim;
        assert_eq!(surface_color::CONCRETE, sim::CONCRETE);
        assert_eq!(surface_color::STEEL, sim::STEEL);
        assert_eq!(surface_color::STEEL_DARK, sim::STEEL_DARK);
        assert_eq!(surface_color::PLATING, sim::PLATING);
        assert_eq!(surface_color::RUST, sim::RUST);
        assert_eq!(surface_color::HAZARD, sim::HAZARD);
        assert_eq!(surface_color::GLASS, sim::GLASS);
    }

    #[test]
    fn hex_converts_channel_by_channel() {
        let c = from_hex(0x00_12_34_56);
        assert_eq!(c.r8(), 0x12);
        assert_eq!(c.g8(), 0x34);
        assert_eq!(c.b8(), 0x56);
        assert_eq!(c.a8(), 255);
    }
}
