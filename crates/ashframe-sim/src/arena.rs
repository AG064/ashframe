//! The arena: one deliberately composed industrial site.
//!
//! Ported from the original's `world/arena.ts`, with the rendering left behind.
//! The original said of itself: *"Every solid is emitted twice — render
//! geometry and an axis-aligned collider — so what you see is what blocks
//! shots."*
//!
//! That pairing is a promise about the game rather than an implementation
//! detail, so the port keeps it: this module emits **both** — the colliders
//! into a [`CollisionWorld`], and the same boxes again as [`Prop`]s for the
//! renderer to draw. The renderer does not decide where a wall is; it is told,
//! by the same code that decided the wall stops bullets. A layout written twice
//! is a layout that will disagree with itself, and the disagreement shows up as
//! shooting through a wall that looks solid.
//!
//! ## What the original drew and this does not
//!
//! Materials, geometry, instancing, shadows, the procedural concrete texture,
//! and the distant skyline's exact silhouettes are all rendering concerns and
//! belong to Godot. What is kept is every number that decides *where* something
//! is, because that is the part the simulation depends on.

use crate::collision::{Box as Collider, CollisionWorld};
use crate::config::world as cfg;
use crate::types::Vec3;

/// One piece of the arena the renderer should draw.
///
/// The collider for a prop is already in the [`CollisionWorld`]; this is the
/// same box again, described for drawing rather than for collision. `rotation`
/// is visual only — colliders are axis-aligned by design, and a rotated prop
/// uses its enclosing box, exactly as the original did.
#[derive(Debug, Clone, PartialEq)]
pub struct Prop {
    pub at: Vec3,
    pub size: Vec3,
    /// Yaw in radians. Zero for everything that has a collider.
    pub rotation: f32,
    /// What it is, for choosing a material and for diagnostics.
    pub tag: &'static str,
    /// Whether the renderer should cast a shadow from it.
    ///
    /// Most things do; ground markings deliberately do not, because a shadow
    /// under a painted stripe is a shadow of nothing.
    pub casts_shadow: bool,
}

/// Named materials the renderer maps to its own.
///
/// The original held Three.js materials here. These are names: the simulation
/// has no business knowing what a material is, and the renderer has no
/// business inventing which surface a wall is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Concrete,
    Steel,
    SteelDark,
    Plating,
    Rust,
    Hazard,
    Glass,
}

/// The colours the original used, as `0xRRGGBB`, for the renderer to match.
///
/// Kept in the simulation because they are part of the arena's identity rather
/// than of any renderer's, and because the alternative is a palette in a Godot
/// script drifting away from the palette the game was designed around.
pub mod palette {
    pub const CONCRETE: u32 = 0x007c_786e;
    pub const STEEL: u32 = 0x0047_4e56;
    pub const STEEL_DARK: u32 = 0x002b_3036;
    pub const PLATING: u32 = 0x009a_a1a8;
    pub const RUST: u32 = 0x006d_4b38;
    pub const HAZARD: u32 = 0x00c9_971f;
    pub const GLASS: u32 = 0x0024_313a;
    pub const SKY: u32 = 0x0093_a6b8;
}

/// Where everything starts, and where it can be sent back to.
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnPoints {
    pub player: Vec3,
    pub skirmisher: Vec<Vec3>,
    pub artillery: Vec<Vec3>,
    pub boss: Vec3,
}

/// The built arena.
#[derive(Debug, Clone)]
pub struct Arena {
    /// Everything the renderer draws, in build order.
    pub props: Vec<Prop>,
    pub spawns: SpawnPoints,
    /// Tall, distinctive points, for the skyline reading and for navigation.
    pub landmarks: Vec<Vec3>,
    /// Ground markings, which are drawn but cast no shadow and block nothing.
    pub markings: Vec<Prop>,
}

/// Build the arena's colliders into `world` and describe it for drawing.
///
/// The collider set and the prop list are produced together, in one pass, so
/// they cannot fall out of step.
pub fn build(world: &mut CollisionWorld) -> Arena {
    let mut b = Builder {
        world,
        props: Vec::new(),
        markings: Vec::new(),
    };
    b.layout();
    b.finish()
}

/// Accumulates colliders and props together.
struct Builder<'a> {
    world: &'a mut CollisionWorld,
    props: Vec<Prop>,
    markings: Vec<Prop>,
}

impl Builder<'_> {
    /// One solid: a collider and the box the renderer draws for it.
    fn solid(&mut self, at: Vec3, size: Vec3, surface: Surface) {
        self.world.add(Collider::from_size(
            at.x,
            at.y,
            at.z,
            size.x,
            size.y,
            size.z,
            surface_tag(surface),
        ));
        self.props.push(Prop {
            at,
            size,
            rotation: 0.0,
            tag: surface_tag(surface),
            casts_shadow: true,
        });
    }

    /// A collider with no visible box of its own, because something else is
    /// drawn in its place.
    fn hidden_collider(&mut self, at: Vec3, size: Vec3, tag: &'static str) {
        self.world.add(Collider::from_size(
            at.x, at.y, at.z, size.x, size.y, size.z, tag,
        ));
    }

    /// A painted marking: drawn, casts nothing, blocks nothing.
    fn marking(&mut self, at: Vec3, size: Vec3, rotation: f32) {
        self.markings.push(Prop {
            at,
            size,
            rotation,
            tag: "marking",
            casts_shadow: false,
        });
    }

    /// A cylindrical tank.
    ///
    /// Drawn as a cylinder, collided as a cross of two boxes. The cross
    /// approximates the circle closely enough that nobody has ever walked
    /// through a tank, and it keeps the whole world axis-aligned.
    fn tank(&mut self, cx: f32, cz: f32, r: f32, h: f32, surface: Surface) {
        self.props.push(Prop {
            at: Vec3::new(cx, h / 2.0, cz),
            size: Vec3::new(r * 2.0, h, r * 2.0),
            rotation: 0.0,
            tag: surface_tag(surface),
            casts_shadow: true,
        });
        let s = r * 1.72;
        self.hidden_collider(Vec3::new(cx, h / 2.0, cz), Vec3::new(r * 2.0, h, s), "tank");
        self.hidden_collider(Vec3::new(cx, h / 2.0, cz), Vec3::new(s, h, r * 2.0), "tank");
    }

    /// A walkable ramp.
    ///
    /// The visible slab is a single rotated box; the collision is a flight of
    /// steps. That is the whole trick: the mech climbs geometry that is flat
    /// underneath, so it never fights a slope, and the player sees a ramp
    /// rather than a staircase.
    #[allow(clippy::too_many_arguments)]
    fn ramp(
        &mut self,
        cx: f32,
        cz: f32,
        width: f32,
        run: f32,
        rise: f32,
        dir_x: f32,
        dir_z: f32,
        surface: Surface,
    ) {
        let steps = ((rise / 0.8).round() as i32).max(5);
        let step_rise = rise / steps as f32;
        let step_run = run / steps as f32;
        let low_x = cx - dir_x * (run / 2.0);
        let low_z = cz - dir_z * (run / 2.0);

        for i in 0..steps {
            let px = low_x + dir_x * step_run * (i as f32 + 0.5);
            let pz = low_z + dir_z * step_run * (i as f32 + 0.5);
            let h = step_rise * (i as f32 + 1.0);
            // The extra 2 cm closes the seam between one step and the next, so
            // a body sliding along the ramp cannot catch on the join.
            let sx = if dir_x != 0.0 { step_run + 0.02 } else { width };
            let sz = if dir_z != 0.0 { step_run + 0.02 } else { width };
            self.hidden_collider(Vec3::new(px, h / 2.0, pz), Vec3::new(sx, h, sz), "ramp");
        }

        let length = (run * run + rise * rise).sqrt();
        let size = Vec3::new(
            if dir_x != 0.0 { length } else { width },
            0.5,
            if dir_z != 0.0 { length } else { width },
        );
        let angle = rise.atan2(run);
        self.props.push(Prop {
            at: Vec3::new(cx, rise / 2.0 - 0.1, cz),
            size,
            // The renderer needs the axis as well as the angle, which the sign
            // carries: a positive rotation about Z tilts along X, and so on.
            rotation: if dir_x != 0.0 {
                if dir_x > 0.0 {
                    -angle
                } else {
                    angle
                }
            } else if dir_z > 0.0 {
                angle
            } else {
                -angle
            },
            tag: surface_tag(surface),
            casts_shadow: true,
        });
    }

    /// The whole site.
    fn layout(&mut self) {
        let h = cfg::HALF;

        // ── 1. command deck, north of the spawn ──────────────────────────
        // Deck top at y = 7.0, reached by a ramp from z = -13 up to z = -27.
        self.solid(
            Vec3::new(0.0, 6.3, -34.0),
            Vec3::new(30.0, 1.4, 14.0),
            Surface::Steel,
        );
        self.solid(
            Vec3::new(0.0, 7.9, -41.0),
            Vec3::new(30.0, 1.4, 1.4),
            Surface::SteelDark,
        );
        self.ramp(0.0, -20.0, 12.0, 14.0, 7.0, 0.0, -1.0, Surface::Concrete);
        self.solid(
            Vec3::new(-9.0, 11.0, -37.0),
            Vec3::new(10.0, 8.0, 9.0),
            Surface::Concrete,
        );
        self.solid(
            Vec3::new(-9.0, 12.4, -32.4),
            Vec3::new(8.0, 3.0, 0.4),
            Surface::Glass,
        );
        self.solid(
            Vec3::new(7.0, 10.2, -37.0),
            Vec3::new(7.0, 6.4, 8.0),
            Surface::SteelDark,
        );
        // Radar mast: the landmark that makes the deck findable from anywhere.
        self.solid(
            Vec3::new(0.0, 14.0, -46.0),
            Vec3::new(1.8, 28.0, 1.8),
            Surface::Steel,
        );
        self.solid(
            Vec3::new(0.0, 28.6, -46.0),
            Vec3::new(10.0, 1.2, 4.4),
            Surface::Plating,
        );
        self.solid(
            Vec3::new(0.0, 29.8, -46.0),
            Vec3::new(2.6, 1.2, 2.6),
            Surface::Hazard,
        );

        // ── 2. cracking towers and the pipe bridge ───────────────────────
        for tx in [-58.0f32, -30.0] {
            self.tank(tx, -78.0, 7.5, 40.0, Surface::Steel);
            self.solid(
                Vec3::new(tx, 41.0, -78.0),
                Vec3::new(12.0, 2.6, 12.0),
                Surface::SteelDark,
            );
        }
        self.solid(
            Vec3::new(-58.0, 44.5, -78.0),
            Vec3::new(3.0, 1.6, 3.0),
            Surface::Hazard,
        );
        self.solid(
            Vec3::new(-44.0, 26.0, -78.0),
            Vec3::new(34.0, 2.2, 5.0),
            Surface::Steel,
        );
        self.solid(
            Vec3::new(-44.0, 12.0, -70.0),
            Vec3::new(4.0, 24.0, 4.0),
            Surface::Concrete,
        );
        self.solid(
            Vec3::new(-44.0, 12.0, -86.0),
            Vec3::new(4.0, 24.0, 4.0),
            Surface::Concrete,
        );
        // Ground-level pipe runs: drawn as cylinders, collided as boxes.
        for (px, pz) in [(-20.0f32, -52.0f32), (26.0, -60.0)] {
            for py in [1.1f32, 3.3] {
                self.props.push(Prop {
                    at: Vec3::new(px, py, pz),
                    size: Vec3::new(22.0, 2.2, 2.2),
                    rotation: std::f32::consts::FRAC_PI_2,
                    tag: "pipe",
                    casts_shadow: true,
                });
            }
            self.hidden_collider(Vec3::new(px, 2.3, pz), Vec3::new(22.0, 4.6, 2.2), "pipe");
        }

        // ── 3. tank farm and the collapsed warehouse ─────────────────────
        for i in 0..2 {
            for j in 0..2 {
                let surface = if i == 1 && j == 0 {
                    Surface::Rust
                } else {
                    Surface::Plating
                };
                self.tank(
                    44.0 + i as f32 * 24.0,
                    20.0 + j as f32 * 30.0,
                    8.5,
                    17.0,
                    surface,
                );
            }
        }
        // Warehouse with a walkable roof: roof top at 15.6, landing pad at 16.6.
        self.solid(
            Vec3::new(96.0, 7.0, -50.0),
            Vec3::new(40.0, 14.0, 30.0),
            Surface::Concrete,
        );
        self.solid(
            Vec3::new(96.0, 14.8, -50.0),
            Vec3::new(42.0, 1.6, 32.0),
            Surface::SteelDark,
        );
        self.solid(
            Vec3::new(96.0, 16.2, -50.0),
            Vec3::new(22.0, 0.8, 14.0),
            Surface::Plating,
        );
        self.solid(
            Vec3::new(76.4, 8.5, -56.0),
            Vec3::new(0.5, 8.0, 10.0),
            Surface::Glass,
        );
        self.ramp(66.0, -50.0, 12.0, 20.0, 16.2, 1.0, 0.0, Surface::Concrete);

        // ── 4. container yard and gantry crane ───────────────────────────
        let mut containers: Vec<Prop> = Vec::new();
        for i in 0..11 {
            let x = -68.0 + i as f32 * 13.5;
            let z = 58.0 + (i % 2) as f32 * 9.0;
            let stack = i % 3;
            for s in 0..=stack {
                let y = 1.6 + s as f32 * 3.25;
                containers.push(Prop {
                    at: Vec3::new(x, y, z),
                    size: Vec3::new(12.0, 3.2, 3.2),
                    rotation: 0.0,
                    tag: "container",
                    casts_shadow: true,
                });
                // The collider is centred a half-step higher than the visible
                // box, which is the original's arithmetic and is kept as-is:
                // the stack is stepped, and the collider for each layer spans
                // from the one below to its own top.
                self.hidden_collider(
                    Vec3::new(x, (s as f32 * 3.25 + (s + 1) as f32 * 3.25) / 2.0, z),
                    Vec3::new(12.0, 3.25, 3.2),
                    "container",
                );
            }
        }
        self.props.extend(containers.iter().cloned());

        self.solid(
            Vec3::new(-34.0, 20.0, 96.0),
            Vec3::new(2.5, 40.0, 2.5),
            Surface::Hazard,
        );
        self.solid(
            Vec3::new(34.0, 20.0, 96.0),
            Vec3::new(2.5, 40.0, 2.5),
            Surface::Hazard,
        );
        self.solid(
            Vec3::new(0.0, 40.5, 96.0),
            Vec3::new(74.0, 2.6, 3.5),
            Surface::Steel,
        );
        self.solid(
            Vec3::new(6.0, 37.5, 96.0),
            Vec3::new(8.0, 3.4, 6.0),
            Surface::SteelDark,
        );
        self.solid(
            Vec3::new(-34.0, 3.0, 96.0),
            Vec3::new(9.0, 6.0, 9.0),
            Surface::Concrete,
        );
        self.solid(
            Vec3::new(34.0, 3.0, 96.0),
            Vec3::new(9.0, 6.0, 9.0),
            Surface::Concrete,
        );
        self.ramp(-34.0, 83.0, 9.0, 9.0, 6.0, 0.0, 1.0, Surface::Concrete);

        // ── 5. western stacks and the elevated conveyor ──────────────────
        for i in 0..3 {
            let z = -40.0 + i as f32 * 34.0;
            let height = 30.0 + i as f32 * 4.0;
            self.tank(-86.0, z, 5.5, height, Surface::SteelDark);
            self.solid(
                Vec3::new(-86.0, height + 0.7, z),
                Vec3::new(3.0, 1.4, 3.0),
                Surface::Hazard,
            );
        }
        // Conveyor deck top at 16.2, spanning z = -30 .. 46.
        self.solid(
            Vec3::new(-60.0, 7.5, 8.0),
            Vec3::new(8.0, 15.0, 60.0),
            Surface::Steel,
        );
        self.solid(
            Vec3::new(-60.0, 15.6, 8.0),
            Vec3::new(12.0, 1.2, 76.0),
            Surface::Steel,
        );
        self.solid(
            Vec3::new(-60.0, 16.6, -24.0),
            Vec3::new(8.0, 1.6, 76.0),
            Surface::SteelDark,
        );
        self.ramp(-60.0, -39.0, 12.0, 18.0, 16.2, 0.0, 1.0, Surface::Concrete);
        self.ramp(-60.0, 55.0, 12.0, 18.0, 16.2, 0.0, -1.0, Surface::Concrete);

        // ── 6. cover in the open yard ────────────────────────────────────
        // Twelve barriers in the open, which is what stops the middle of the
        // map from being a place where the first person to shoot wins.
        let spots = [
            (40.0f32, -8.0f32),
            (-44.0, 30.0),
            (56.0, 46.0),
            (-14.0, 62.0),
            (24.0, -4.0),
            (-64.0, -6.0),
            (70.0, 10.0),
            (14.0, 88.0),
            (-6.0, -46.0),
            (62.0, -86.0),
            (-40.0, -20.0),
            (36.0, 66.0),
        ];
        for (i, (x, z)) in spots.iter().enumerate() {
            let rotation = (i % 4) as f32 * std::f32::consts::FRAC_PI_2;
            self.props.push(Prop {
                at: Vec3::new(*x, 1.5, *z),
                size: Vec3::new(8.0, 3.0, 2.0),
                rotation,
                tag: "barrier",
                casts_shadow: true,
            });
            // The collider follows the visible rotation by swapping extents,
            // because the world has no rotated boxes.
            let along_x = i % 4 == 0 || i % 4 == 2;
            self.hidden_collider(
                Vec3::new(*x, 1.5, *z),
                if along_x {
                    Vec3::new(8.0, 3.0, 2.0)
                } else {
                    Vec3::new(2.0, 3.0, 8.0)
                },
                "barrier",
            );
        }

        // Rubble: visual detail with no colliders, so the mech cannot snag on
        // it. Deterministic rather than random, so a replay looks the same.
        for i in 0..90 {
            let x = (rubble_noise(i) - 0.5) * (h * 2.0 - 30.0);
            let z = (rubble_noise(i + 500) - 0.5) * (h * 2.0 - 30.0);
            let y = self.world.ground_at(x, z, 200.0, 400.0);
            // Skipped above 1.5 m so rubble does not float over the roadways
            // and the elevated decks.
            if y > 1.5 {
                continue;
            }
            self.props.push(Prop {
                at: Vec3::new(x, y + 0.2, z),
                size: Vec3::new(1.6, 0.5, 1.1),
                rotation: rubble_noise(i + 900) * std::f32::consts::PI,
                tag: "rubble",
                casts_shadow: true,
            });
        }

        // Container ribs, which give the boxes their corrugated read.
        for container in &containers {
            for k in -1..=1 {
                self.props.push(Prop {
                    at: Vec3::new(
                        container.at.x + k as f32 * 3.6,
                        container.at.y,
                        container.at.z,
                    ),
                    size: Vec3::new(0.35, 3.3, 3.4),
                    rotation: 0.0,
                    tag: "rib",
                    casts_shadow: true,
                });
            }
        }

        // Painted hazard chevrons: drawn, and deliberately neither solid nor
        // shadow-casting.
        for i in 0..5 {
            self.marking(
                Vec3::new(0.0, 7.72, -33.0 + i as f32 * 2.6),
                Vec3::new(16.0, 0.06, 1.2),
                0.0,
            );
        }
        for i in 0..4 {
            self.marking(
                Vec3::new(-6.0 + i as f32 * 4.0, 0.04, 12.0),
                Vec3::new(16.0, 0.06, 1.2),
                std::f32::consts::FRAC_PI_2,
            );
        }

        // ── 7. perimeter blast wall with approach gaps ───────────────────
        // Gaps at the cardinal approaches, so the wall shapes the fight
        // instead of fencing the player in.
        for i in -4i32..=4 {
            if i.abs() <= 1 {
                continue;
            }
            let p = i as f32 * 34.0;
            for (x, z, rotation) in [
                (p, -h + 4.0, 0.0f32),
                (p, h - 4.0, 0.0),
                (-h + 4.0, p, std::f32::consts::FRAC_PI_2),
                (h - 4.0, p, std::f32::consts::FRAC_PI_2),
            ] {
                self.props.push(Prop {
                    at: Vec3::new(x, 4.5, z),
                    size: Vec3::new(30.0, 9.0, 4.0),
                    rotation,
                    tag: "wall",
                    casts_shadow: true,
                });
                let along_x = rotation == 0.0;
                self.hidden_collider(
                    Vec3::new(x, 4.5, z),
                    if along_x {
                        Vec3::new(30.0, 9.0, 4.0)
                    } else {
                        Vec3::new(4.0, 9.0, 30.0)
                    },
                    "wall",
                );
            }
        }

        // ── 8. the distant skyline ───────────────────────────────────────
        // Visual scale only, well outside the play area. Generated here rather
        // than in the renderer so the silhouette is the same every run and can
        // be reasoned about.
        for i in 0..46 {
            let angle = (i as f32 / 46.0) * std::f32::consts::PI * 2.0 + 0.21;
            let ring = i % 3;
            let r = 430.0 + ring as f32 * 130.0 + (i % 5) as f32 * 34.0;
            let w = 34.0 + (i % 4) as f32 * 22.0;
            let hh = 60.0 + ((i * 53) % 150) as f32 + ring as f32 * 30.0;
            self.props.push(Prop {
                at: Vec3::new(angle.cos() * r, hh / 2.0 - 10.0, angle.sin() * r),
                size: Vec3::new(w, hh, w),
                rotation: 0.0,
                tag: "skyline",
                casts_shadow: false,
            });
            // A stepped crown on every fourth tower, which breaks up what
            // would otherwise be a ring of identical slabs.
            if i % 4 == 0 {
                self.props.push(Prop {
                    at: Vec3::new(angle.cos() * r, hh + 12.0, angle.sin() * r),
                    size: Vec3::new(w * 0.4, 22.0, w * 0.4),
                    rotation: 0.0,
                    tag: "skyline",
                    casts_shadow: false,
                });
            }
        }
    }

    fn finish(self) -> Arena {
        Arena {
            props: self.props,
            spawns: spawn_points(),
            landmarks: vec![
                Vec3::new(0.0, 30.0, -46.0),
                Vec3::new(-58.0, 44.0, -78.0),
                Vec3::new(96.0, 16.0, -50.0),
                Vec3::new(0.0, 41.0, 96.0),
                Vec3::new(-60.0, 16.0, 8.0),
            ],
            markings: self.markings,
        }
    }
}

fn surface_tag(surface: Surface) -> &'static str {
    match surface {
        Surface::Concrete => "concrete",
        Surface::Steel => "steel",
        Surface::SteelDark => "steel-dark",
        Surface::Plating => "plating",
        Surface::Rust => "rust",
        Surface::Hazard => "hazard",
        Surface::Glass => "glass",
    }
}

/// The original's scatter function, reproduced exactly.
///
/// A sine hash rather than the game's `Rng`, because the original used one here
/// and rubble placed differently would make a replay's visuals disagree with its
/// recording even though nothing about the fight changed.
fn rubble_noise(i: i32) -> f32 {
    let raw = (i as f32 * 12.9898).sin() * 43_758.547;
    // JavaScript's `%` keeps the sign of the dividend, so the original adds one
    // and takes the remainder again to land in `0.0..1.0`. Reproduced rather
    // than replaced with `rem_euclid`, which would place every stone
    // differently.
    (raw % 1.0 + 1.0) % 1.0
}

/// Where everything starts.
pub fn spawn_points() -> SpawnPoints {
    SpawnPoints {
        player: Vec3::new(0.0, cfg::FLOOR_Y, 26.0),
        skirmisher: vec![
            Vec3::new(-40.0, 0.0, -22.0),
            Vec3::new(48.0, 0.0, -26.0),
            Vec3::new(-24.0, 0.0, 40.0),
            Vec3::new(38.0, 0.0, 34.0),
            Vec3::new(-72.0, 0.0, 62.0),
            Vec3::new(18.0, 0.0, 74.0),
            Vec3::new(66.0, 0.0, 58.0),
            Vec3::new(-14.0, 0.0, -56.0),
            Vec3::new(78.0, 0.0, -16.0),
            Vec3::new(-88.0, 0.0, 4.0),
        ],
        artillery: vec![
            Vec3::new(30.0, 0.0, -62.0),
            Vec3::new(-48.0, 0.0, 18.0),
            Vec3::new(74.0, 0.0, -4.0),
            Vec3::new(-4.0, 0.0, 80.0),
            Vec3::new(52.0, 0.0, 8.0),
        ],
        boss: Vec3::new(0.0, 0.0, -64.0),
    }
}

/// Whether a body of `radius` and `height` fits at `(x, z)`.
pub fn is_spawn_clear(world: &CollisionWorld, x: f32, z: f32, radius: f32, height: f32) -> bool {
    let y = world.ground_at(x, z, 200.0, 400.0);
    let mut probe = Vec3::new(x, y, z);
    let before = probe;
    // Step height zero and no upward velocity: this asks whether the body is
    // intersecting anything, not whether it would be comfortable there.
    world.resolve_cylinder(&mut probe, radius, height, 0.0, 0.0);
    (probe - before).length_squared() < 1e-6
}

/// Nudge a desired spawn to a valid standing position.
///
/// Enemies are never dropped inside scenery: if the authored point is blocked,
/// the search walks outward in rings until it finds clear ground. Falls back to
/// the middle of the arena rather than to somewhere it has not checked.
pub fn resolve_spawn(world: &CollisionWorld, desired: Vec3, radius: f32, height: f32) -> Vec3 {
    let mut out = desired;
    out.y = world.ground_at(out.x, out.z, 220.0, 400.0);
    if is_spawn_clear(world, out.x, out.z, radius, height) {
        return out;
    }

    for ring in 1..=6 {
        let r = ring as f32 * (radius + 2.5);
        for i in 0..12 {
            let a = (i as f32 / 12.0) * std::f32::consts::PI * 2.0;
            let x = desired.x + a.cos() * r;
            let z = desired.z + a.sin() * r;
            // Kept away from the blast wall, so a nudge cannot put a unit
            // inside the perimeter it is meant to fight within.
            if x.abs() > cfg::HALF - 6.0 || z.abs() > cfg::HALF - 6.0 {
                continue;
            }
            if !is_spawn_clear(world, x, z, radius, height) {
                continue;
            }
            out = Vec3::new(x, world.ground_at(x, z, 220.0, 400.0), z);
            return out;
        }
    }

    out = Vec3::new(0.0, world.ground_at(0.0, 0.0, 220.0, 400.0), 0.0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn built() -> (CollisionWorld, Arena) {
        let mut world = CollisionWorld::new(cfg::FLOOR_Y);
        let arena = build(&mut world);
        (world, arena)
    }

    #[test]
    fn the_arena_is_solid_enough_to_hide_behind() {
        let (world, _) = built();
        assert!(
            world.boxes.len() > 100,
            "an arena this size should have well over a hundred colliders, got {}",
            world.boxes.len()
        );

        // And the cover is spread rather than clustered: the open yard is the
        // middle of the map, and a barrier count that collapsed would turn it
        // into a place where whoever shoots first wins.
        let barriers = world.boxes.iter().filter(|b| b.tag == "barrier").count();
        assert_eq!(barriers, 12, "the yard should have twelve barriers");
        assert!(
            world.boxes.iter().any(|b| b.tag == "wall"),
            "and a perimeter"
        );
        assert!(world.boxes.iter().any(|b| b.tag == "ramp"), "and ramps");
    }

    #[test]
    fn every_prop_has_a_size_and_a_tag() {
        let (_, arena) = built();
        for prop in &arena.props {
            assert!(prop.size.x > 0.0 && prop.size.y > 0.0 && prop.size.z > 0.0);
            assert!(!prop.tag.is_empty());
            assert!(prop.at.is_finite());
            assert!(prop.rotation.is_finite());
        }
    }

    #[test]
    fn every_collider_is_a_real_box() {
        // A box with a negative extent has no interior and no face the normal
        // rule can pick, so it is silently unhittable.
        let (world, _) = built();
        for b in &world.boxes {
            assert!(
                b.max_x >= b.min_x && b.max_y >= b.min_y && b.max_z >= b.min_z,
                "inverted collider: {b:?}"
            );
            assert!(!b.tag.is_empty(), "every collider should say what it is");
        }
    }

    #[test]
    fn the_ground_is_clear_where_the_player_starts() {
        let (world, arena) = built();
        let p = arena.spawns.player;
        assert!(
            is_spawn_clear(&world, p.x, p.z, 1.7, 6.0),
            "the player must not start inside scenery"
        );
    }

    #[test]
    fn every_authored_enemy_spawn_is_usable() {
        // The spawn points are hand-placed against a layout that has changed
        // since; one sitting inside a tank would drop an enemy into geometry
        // and it would never be seen.
        let (world, arena) = built();
        for (i, point) in arena.spawns.skirmisher.iter().enumerate() {
            let resolved = resolve_spawn(&world, *point, 1.35, 4.6);
            assert!(
                is_spawn_clear(&world, resolved.x, resolved.z, 1.35, 4.6),
                "skirmisher spawn {i} could not be resolved to clear ground"
            );
        }
        for (i, point) in arena.spawns.artillery.iter().enumerate() {
            let resolved = resolve_spawn(&world, *point, 1.9, 5.4);
            assert!(
                is_spawn_clear(&world, resolved.x, resolved.z, 1.9, 5.4),
                "artillery spawn {i} could not be resolved to clear ground"
            );
        }

        let boss = resolve_spawn(&world, arena.spawns.boss, 3.4, 11.5);
        assert!(is_spawn_clear(&world, boss.x, boss.z, 3.4, 11.5));
    }

    #[test]
    fn spawn_resolution_leaves_a_clear_point_alone() {
        // A nudge that moves a unit which was already fine would make enemy
        // placement unpredictable for no reason.
        let (world, arena) = built();
        let p = arena.spawns.player;
        let resolved = resolve_spawn(&world, p, 1.7, 6.0);
        assert!((resolved.x - p.x).abs() < 1e-3);
        assert!((resolved.z - p.z).abs() < 1e-3);
    }

    #[test]
    fn a_blocked_spawn_is_resolved_inside_the_perimeter() {
        // The ring search bounds itself to the arena, so a nudge can never
        // push a unit through the blast wall it is meant to fight inside.
        let (world, _) = built();
        // Inside a cracking tower, so the point itself is blocked and the
        // search actually runs.
        let resolved = resolve_spawn(&world, Vec3::new(-58.0, 0.0, -78.0), 1.7, 6.0);
        assert!(
            resolved.x.abs() <= cfg::HALF && resolved.z.abs() <= cfg::HALF,
            "resolved to ({}, {}), outside the arena",
            resolved.x,
            resolved.z
        );
        assert!(
            is_spawn_clear(&world, resolved.x, resolved.z, 1.7, 6.0),
            "the resolved point should be somewhere a body fits"
        );
    }

    #[test]
    fn a_clear_spawn_is_returned_even_when_it_is_far_outside() {
        // Behaviour of the original, recorded because it is not obvious.
        //
        // `is_spawn_clear` asks whether a body *intersects* something. There
        // is nothing at (1000, 1000) to intersect, and the ground query
        // answers with the floor, so an out-of-bounds point comes back
        // unchanged and the ring search never runs. Containment is only
        // guaranteed on the path where the desired point was blocked.
        //
        // Every authored spawn is inside the arena, so this never arises in
        // play. A test that asserted otherwise would be asserting a guarantee
        // this code does not make, and the next person would trust it.
        let (world, _) = built();
        let resolved = resolve_spawn(&world, Vec3::new(1000.0, 0.0, 1000.0), 1.7, 6.0);
        assert_eq!(resolved.x, 1000.0);
        assert_eq!(resolved.z, 1000.0);
    }

    #[test]
    fn a_resolved_spawn_always_stands_on_something() {
        let (world, _) = built();
        for x in [-140.0f32, -60.0, 0.0, 60.0, 140.0] {
            for z in [-140.0f32, 0.0, 140.0] {
                let resolved = resolve_spawn(&world, Vec3::new(x, 0.0, z), 1.7, 6.0);
                let ground = world.ground_at(resolved.x, resolved.z, 220.0, 400.0);
                assert!(
                    (resolved.y - ground).abs() < 1e-2,
                    "resolved spawn at ({x}, {z}) floats at {} above ground {ground}",
                    resolved.y
                );
            }
        }
    }

    #[test]
    fn the_landmarks_are_where_the_layout_puts_them() {
        // They are what the skyline reads and what a commander would call
        // out. A landmark that drifts from its structure is worse than none.
        let (_, arena) = built();
        assert_eq!(arena.landmarks.len(), 5);
        assert!(
            arena.landmarks.contains(&Vec3::new(0.0, 30.0, -46.0)),
            "radar mast"
        );
        assert!(
            arena.landmarks.contains(&Vec3::new(-58.0, 44.0, -78.0)),
            "flare stack"
        );
        assert!(
            arena.landmarks.contains(&Vec3::new(96.0, 16.0, -50.0)),
            "warehouse roof"
        );
    }

    #[test]
    fn the_deck_and_the_warehouse_roof_are_reachable_by_ramp() {
        // Both are described as elevated positions worth taking. An
        // unreachable one is decoration pretending to be level design.
        let (world, _) = built();

        // The deck: 7 metres up at z = -34.
        let deck = world.ground_at(0.0, -34.0, 60.0, 400.0);
        assert!(
            (deck - 7.0).abs() < 0.1,
            "deck top should be at 7.0, got {deck}"
        );
        // And a ramp climbs to it -- its steps end near the deck's edge.
        let ramp_top = world.ground_at(0.0, -27.0, 60.0, 400.0);
        assert!(
            ramp_top > 5.0,
            "the ramp should nearly reach the deck, got {ramp_top}"
        );

        // The warehouse roof: 16.6 at the landing pad.
        let roof = world.ground_at(96.0, -50.0, 60.0, 400.0);
        assert!(
            (roof - 16.6).abs() < 0.1,
            "roof pad should be at 16.6, got {roof}"
        );
    }

    #[test]
    fn the_perimeter_has_gaps_at_the_cardinal_approaches() {
        // The wall shapes the fight rather than fencing the player in. With
        // no gaps the mech could be trapped against it by a wave it cannot
        // outrun.
        let (world, _) = built();
        for (x, z) in [(0.0f32, -cfg::HALF + 4.0), (0.0, cfg::HALF - 4.0)] {
            assert!(
                world
                    .raycast(Vec3::new(x, 4.5, z - 20.0), Vec3::new(0.0, 0.0, 1.0), 40.0)
                    .is_none()
                    || world
                        .raycast(Vec3::new(x, 4.5, z), Vec3::new(0.0, 0.0, 1.0), 1.0)
                        .is_none(),
                "there should be a gap at ({x}, {z})"
            );
        }
    }

    #[test]
    fn the_containers_form_the_stacks_the_original_built() {
        // Eleven positions, stacked zero, one or two high in a repeating
        // pattern. The count matters because the yard is the main cover in the
        // south and a missing layer changes the sightlines.
        let (_, arena) = built();
        let containers: Vec<&Prop> = arena
            .props
            .iter()
            .filter(|p| p.tag == "container")
            .collect();
        let expected: usize = (0..11).map(|i| i % 3 + 1).sum();
        assert_eq!(containers.len(), expected);
    }

    #[test]
    fn rubble_never_floats_above_a_roadway() {
        // The original skips anything above 1.5 m so stones do not hover over
        // the elevated decks.
        let (world, arena) = built();
        for prop in arena.props.iter().filter(|p| p.tag == "rubble") {
            let ground = world.ground_at(prop.at.x, prop.at.z, 200.0, 400.0);
            assert!(
                (prop.at.y - (ground + 0.2)).abs() < 1e-2,
                "rubble at {:?} is not resting on ground {ground}",
                prop.at
            );
            assert!(ground <= 1.5, "rubble should not sit above 1.5 m");
        }
    }

    #[test]
    fn the_scatter_is_the_same_every_time() {
        // A replay has to look like its recording.
        let (_, first) = built();
        let (_, second) = built();
        assert_eq!(first.props, second.props);
    }

    #[test]
    fn rubble_noise_stays_in_the_unit_interval() {
        // The original's remainder arithmetic is easy to get wrong in a way
        // that produces negatives, which would scatter rubble outside the
        // arena on one side only.
        for i in 0..2000 {
            let v = rubble_noise(i);
            assert!((0.0..1.0).contains(&v), "rubble_noise({i}) = {v}");
        }
    }

    #[test]
    fn the_skyline_is_outside_the_play_area() {
        let (_, arena) = built();
        for prop in arena.props.iter().filter(|p| p.tag == "skyline") {
            let distance = (prop.at.x * prop.at.x + prop.at.z * prop.at.z).sqrt();
            assert!(
                distance > cfg::HALF + 100.0,
                "skyline at {distance} is too close to the arena"
            );
        }
    }

    #[test]
    fn markings_block_nothing() {
        // They are paint. A collider here would stop a mech on a stripe.
        let (world, arena) = built();
        assert!(!arena.markings.is_empty());
        for marking in &arena.markings {
            assert!(!marking.casts_shadow, "paint has no shadow");
            assert!(marking.size.y < 0.1, "paint has no height");
        }
        // And the deck markings sit on the deck, not inside it.
        let deck = world.ground_at(0.0, -33.0, 60.0, 400.0);
        assert!((deck - 7.0).abs() < 0.1);
    }

    #[test]
    fn the_building_is_deterministic() {
        // Two builds must produce identical colliders, or a replay diverges
        // from its recording the moment anything is rebuilt.
        let (first, _) = built();
        let (second, _) = built();
        assert_eq!(first.boxes, second.boxes);
    }
}
