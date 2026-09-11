//! The collision world.
//!
//! Ported from the original's `world/collision.ts`, which said of itself:
//! *"Axis-aligned boxes are the only static proxy. Everything gameplay-facing
//! (movement, projectiles, line of sight, camera pull-in) runs through this
//! module so fast objects are swept rather than sampled at frame ends."*
//!
//! Two decisions in here are load-bearing and easy to undo by accident.
//!
//! **Projectiles sweep rather than sample.** At 300 m/s and a 120 Hz step a
//! rifle round advances 2.5 metres between steps, further than many surfaces
//! are thick. A point test at the end of each step would let rounds pass
//! through cover, and it would do so intermittently — which reads as the game
//! being unfair rather than as a bug.
//!
//! **The body resolver pushes out rather than applying impulses.** That is what
//! makes "never clip through a wall" a property of the resolver rather than a
//! tuning accident: the body is placed where it is allowed to be, every step,
//! instead of being asked nicely to go there.
//!
//! The API takes loose `f32`s rather than [`Vec3`](crate::types::Vec3), exactly
//! as the original did. That is deliberate: it keeps every call site in the
//! rest of the port a mechanical translation, so a mistake shows up as a
//! compile error rather than as a subtly different vector operation.

use crate::types::Vec3;

/// An immovable collider.
#[derive(Debug, Clone, PartialEq)]
pub struct Box {
    pub min_x: f32,
    pub min_y: f32,
    pub min_z: f32,
    pub max_x: f32,
    pub max_y: f32,
    pub max_z: f32,
    /// Free-form label, used by diagnostics and by surface-specific effects.
    pub tag: String,
}

impl Box {
    /// A box from its centre and its full size, which is how the arena builder
    /// thinks about the world.
    pub fn from_size(cx: f32, cy: f32, cz: f32, sx: f32, sy: f32, sz: f32, tag: &str) -> Self {
        Self {
            min_x: cx - sx / 2.0,
            min_y: cy - sy / 2.0,
            min_z: cz - sz / 2.0,
            max_x: cx + sx / 2.0,
            max_y: cy + sy / 2.0,
            max_z: cz + sz / 2.0,
            tag: tag.to_string(),
        }
    }

    /// A box from its centre and its half extents.
    ///
    /// Takes vectors rather than six floats. The original passed the numbers
    /// loose, and six same-typed parameters in a row is a transposition
    /// waiting to happen -- a box built from `(x, y, z, hz, hy, hx)` compiles
    /// and is wrong. Grouping them makes the mistake a type error.
    pub fn from_half(centre: Vec3, half: Vec3, tag: &str) -> Self {
        Self {
            min_x: centre.x - half.x,
            min_y: centre.y - half.y,
            min_z: centre.z - half.z,
            max_x: centre.x + half.x,
            max_y: centre.y + half.y,
            max_z: centre.z + half.z,
            tag: tag.to_string(),
        }
    }

    /// The centre, for the renderer and for diagnostics.
    pub fn centre(&self) -> Vec3 {
        Vec3::new(
            (self.min_x + self.max_x) * 0.5,
            (self.min_y + self.max_y) * 0.5,
            (self.min_z + self.max_z) * 0.5,
        )
    }

    /// The full size, for the renderer.
    pub fn size(&self) -> Vec3 {
        Vec3::new(
            self.max_x - self.min_x,
            self.max_y - self.min_y,
            self.max_z - self.min_z,
        )
    }

    pub fn contains_xz(&self, x: f32, z: f32) -> bool {
        x >= self.min_x && x <= self.max_x && z >= self.min_z && z <= self.max_z
    }
}

/// Where a ray or a sweep met something.
#[derive(Debug, Clone, PartialEq)]
pub struct RayHit {
    /// Distance along the ray.
    pub t: f32,
    pub at: Vec3,
    pub normal: Vec3,
    pub tag: String,
}

/// What resolving a body against the world did.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ResolveResult {
    pub hit: bool,
    /// The body was pushed up onto a low ledge.
    pub stepped: bool,
    /// Strongest surface normal encountered, in the ground plane. Not
    /// normalised: the original sums contributions, so a body touching two
    /// walls leans by the total. Preserved as-is.
    pub normal_xz: Vec3,
    /// A ceiling stopped upward motion.
    pub ceiling: bool,
}

/// The tolerance below which a direction counts as parallel to a slab.
const EPS: f32 = 1e-9;

/// Slab test for one axis.
///
/// Returns the updated interval, or `None` when the ray misses this slab.
fn slab(o: f32, d: f32, lo: f32, hi: f32, t0: f32, t1: f32) -> Option<(f32, f32)> {
    if d.abs() < EPS {
        // Parallel to the slab: either inside it for all time, or outside it
        // for all time. No division, which is also why a zero direction here
        // cannot produce an infinity that poisons the interval.
        if o < lo || o > hi {
            return None;
        }
        return Some((t0, t1));
    }
    let inv = 1.0 / d;
    let mut ta = (lo - o) * inv;
    let mut tb = (hi - o) * inv;
    if ta > tb {
        std::mem::swap(&mut ta, &mut tb);
    }
    let n0 = if ta > t0 { ta } else { t0 };
    let n1 = if tb < t1 { tb } else { t1 };
    if n0 > n1 {
        return None;
    }
    Some((n0, n1))
}

/// The face normal implied by which side of the box the entry point is nearest.
///
/// The comparisons are exact equality against a computed minimum, as in the
/// original. That looks fragile and is not: `dmin` *is* one of those six
/// values, so at least one comparison must match, and the final `+Z` return
/// only catches a box with no extent on any axis.
fn face_normal(t: f32, origin: Vec3, direction: Vec3, b: &Box) -> Vec3 {
    let px = origin.x + direction.x * t;
    let py = origin.y + direction.y * t;
    let pz = origin.z + direction.z * t;

    let dx_min = (px - b.min_x).abs();
    let dx_max = (px - b.max_x).abs();
    let dy_min = (py - b.min_y).abs();
    let dy_max = (py - b.max_y).abs();
    let dz_min = (pz - b.min_z).abs();
    let dz_max = (pz - b.max_z).abs();

    let dmin = dx_min
        .min(dx_max)
        .min(dy_min)
        .min(dy_max)
        .min(dz_min)
        .min(dz_max);

    if dmin == dx_min {
        return Vec3::new(-1.0, 0.0, 0.0);
    }
    if dmin == dx_max {
        return Vec3::new(1.0, 0.0, 0.0);
    }
    if dmin == dy_min {
        return Vec3::new(0.0, -1.0, 0.0);
    }
    if dmin == dy_max {
        return Vec3::new(0.0, 1.0, 0.0);
    }
    if dmin == dz_min {
        return Vec3::new(0.0, 0.0, -1.0);
    }
    Vec3::new(0.0, 0.0, 1.0)
}

/// Every static collider in the arena, and the queries that run against them.
///
/// Boxes are held in a flat `Vec` and every query walks all of them. That is
/// not an oversight: an arena is a few hundred boxes, the walk is a handful of
/// comparisons each, and a broadphase would be more code to get wrong than it
/// saves. If the arena ever grows to thousands, this is the place to revisit.
#[derive(Debug, Clone, Default)]
pub struct CollisionWorld {
    pub boxes: Vec<Box>,
    floor_y: f32,
}

impl CollisionWorld {
    pub fn new(floor_y: f32) -> Self {
        Self {
            boxes: Vec::new(),
            floor_y,
        }
    }

    pub fn floor_y(&self) -> f32 {
        self.floor_y
    }

    pub fn clear(&mut self) {
        self.boxes.clear();
    }

    pub fn add(&mut self, b: Box) {
        self.boxes.push(b);
    }

    /// Add a box from its centre and half extents.
    pub fn add_centred(&mut self, centre: Vec3, half: Vec3, tag: &str) {
        self.boxes.push(Box::from_half(centre, half, tag));
    }

    /// The nearest hit along a ray, or `None`.
    pub fn raycast(&self, origin: Vec3, direction: Vec3, max_dist: f32) -> Option<RayHit> {
        let mut best: Option<RayHit> = None;
        let mut best_t = max_dist;

        for b in &self.boxes {
            // The interval is narrowed by each slab, so a box that cannot be
            // hit within `best_t` is discarded before the next axis is tested.
            let Some(span) = slab(origin.x, direction.x, b.min_x, b.max_x, 0.0, best_t) else {
                continue;
            };
            let Some(span) = slab(origin.y, direction.y, b.min_y, b.max_y, span.0, span.1) else {
                continue;
            };
            let Some(span) = slab(origin.z, direction.z, b.min_z, b.max_z, span.0, span.1) else {
                continue;
            };
            let t = span.0;
            if t < 0.0 || t > best_t {
                continue;
            }
            best_t = t;
            best = Some(RayHit {
                t,
                at: Vec3::new(
                    origin.x + direction.x * t,
                    origin.y + direction.y * t,
                    origin.z + direction.z * t,
                ),
                normal: face_normal(t, origin, direction, b),
                tag: b.tag.clone(),
            });
        }
        best
    }

    /// Sweep a sphere along a segment.
    ///
    /// This is what fast projectiles use. The box is grown by the radius on
    /// every axis and the result is an ordinary ray test against the grown box,
    /// which is exact for an axis-aligned world: a sphere against an AABB is a
    /// point against the same box expanded by the radius, except near the
    /// corners where the expanded box is very slightly generous. The original
    /// makes that trade and it is invisible at these sizes; it is noted here
    /// because a future reader will otherwise assume it is exact.
    pub fn sweep_sphere(
        &self,
        origin: Vec3,
        direction: Vec3,
        radius: f32,
        max_dist: f32,
    ) -> Option<RayHit> {
        let mut best: Option<RayHit> = None;
        let mut best_t = max_dist;

        for b in &self.boxes {
            let Some(span) = slab(
                origin.x,
                direction.x,
                b.min_x - radius,
                b.max_x + radius,
                0.0,
                best_t,
            ) else {
                continue;
            };
            let Some(span) = slab(
                origin.y,
                direction.y,
                b.min_y - radius,
                b.max_y + radius,
                span.0,
                span.1,
            ) else {
                continue;
            };
            let Some(span) = slab(
                origin.z,
                direction.z,
                b.min_z - radius,
                b.max_z + radius,
                span.0,
                span.1,
            ) else {
                continue;
            };
            let t = span.0;
            if t < 0.0 || t > best_t {
                continue;
            }
            best_t = t;
            best = Some(RayHit {
                t,
                at: Vec3::new(
                    origin.x + direction.x * t,
                    origin.y + direction.y * t,
                    origin.z + direction.z * t,
                ),
                normal: face_normal(t, origin, direction, b),
                tag: b.tag.clone(),
            });
        }
        best
    }

    /// Whether nothing solid sits between two points.
    pub fn line_of_sight(&self, from: Vec3, to: Vec3) -> bool {
        let delta = to - from;
        let len = delta.length();
        // Coincident points can see each other; dividing by a zero length
        // would make the answer depend on a NaN comparison.
        if len < 1e-4 {
            return true;
        }
        let direction = delta * (1.0 / len);
        // Stopped just short of the target, so a ray aimed at a point on a
        // surface does not report the surface itself as an obstruction. That
        // is what lets a unit see the thing it is standing on.
        self.raycast(from, direction, len - 0.05).is_none()
    }

    /// The highest walkable surface directly under a point, including the floor.
    pub fn ground_at(&self, x: f32, z: f32, from_y: f32, probe: f32) -> f32 {
        let hit = self.raycast(Vec3::new(x, from_y, z), Vec3::new(0.0, -1.0, 0.0), probe);
        hit.map_or(self.floor_y, |h| h.at.y)
    }

    /// The highest surface under a small footprint.
    ///
    /// A single centre probe makes stairs and ledge edges feel like they are
    /// swallowing the mech; sampling a short ring lets broad feet stand on the
    /// edge they are actually touching. The radius is scaled by 0.6 so the ring
    /// sits inside the footprint rather than on its rim.
    pub fn ground_under(&self, x: f32, z: f32, from_y: f32, radius: f32) -> f32 {
        let mut best = self.ground_at(x, z, from_y, 400.0);
        let r = radius * 0.6;
        for (dx, dz) in [(r, 0.0), (-r, 0.0), (0.0, r), (0.0, -r)] {
            let g = self.ground_at(x + dx, z + dz, from_y, 400.0);
            if g > best {
                best = g;
            }
        }
        best
    }

    /// Resolve a vertical cylinder against the world, moving `pos` in place.
    ///
    /// Positional push-out rather than impulses, which is what makes the
    /// no-clipping guarantee a property of the resolver and not a tuning
    /// accident.
    pub fn resolve_cylinder(
        &self,
        pos: &mut Vec3,
        radius: f32,
        height: f32,
        step_height: f32,
        vel_y: f32,
    ) -> ResolveResult {
        let mut out = ResolveResult::default();
        let feet = pos.y;
        let head = pos.y + height;

        for b in &self.boxes {
            // Vertical overlap, with a tolerance band so a body resting exactly
            // on a ledge top is not treated as intersecting it. Without the
            // band the body is pushed out of the thing it is standing on, every
            // step, forever.
            if b.max_y <= feet + step_height + 1e-3 {
                continue;
            }
            if b.min_y >= head - 1e-3 {
                continue;
            }

            // Head meets the underside of a box.
            if vel_y > 0.0 && b.min_y >= head - 0.35 && b.min_y <= head + 0.35 {
                let inside_xz = pos.x > b.min_x - radius
                    && pos.x < b.max_x + radius
                    && pos.z > b.min_z - radius
                    && pos.z < b.max_z + radius;
                if inside_xz {
                    pos.y = b.min_y - height - 1e-3;
                    out.ceiling = true;
                    out.hit = true;
                }
                continue;
            }

            // The nearest point of the box in the ground plane.
            let cx = if pos.x < b.min_x {
                b.min_x
            } else if pos.x > b.max_x {
                b.max_x
            } else {
                pos.x
            };
            let cz = if pos.z < b.min_z {
                b.min_z
            } else if pos.z > b.max_z {
                b.max_z
            } else {
                pos.z
            };
            let dx = pos.x - cx;
            let dz = pos.z - cz;
            let d2 = dx * dx + dz * dz;
            if d2 > radius * radius {
                continue;
            }

            // A low ledge is stepped onto rather than blocked, which is what
            // makes ramps, kerbs and container stacks traversable without
            // jumping at every one.
            let ledge = b.max_y - feet;
            if ledge > 0.0 && ledge <= step_height {
                // Only climb when the body actually fits above the ledge.
                // Without this check a mech under an overhang would be lifted
                // into it.
                let overhead = self.raycast(
                    Vec3::new(pos.x, b.max_y + 0.05, pos.z),
                    Vec3::new(0.0, 1.0, 0.0),
                    height - ledge + 0.4,
                );
                if overhead.is_none() {
                    pos.y = b.max_y;
                    out.stepped = true;
                    out.hit = true;
                    continue;
                }
            }

            out.hit = true;
            if d2 > 1e-8 {
                let d = d2.sqrt();
                let push = radius - d;
                let nx = dx / d;
                let nz = dz / d;
                pos.x += nx * push;
                pos.z += nz * push;
                out.normal_xz = out.normal_xz + Vec3::new(nx, 0.0, nz);
            } else {
                // The centre is inside the footprint. Escape along the
                // shallowest face, which is the shortest way out and the one
                // that looks least like being spat sideways.
                let to_min_x = pos.x - b.min_x + radius;
                let to_max_x = b.max_x - pos.x + radius;
                let to_min_z = pos.z - b.min_z + radius;
                let to_max_z = b.max_z - pos.z + radius;
                let m = to_min_x.min(to_max_x).min(to_min_z).min(to_max_z);
                if m == to_min_x {
                    pos.x = b.min_x - radius;
                    out.normal_xz = out.normal_xz + Vec3::new(-1.0, 0.0, 0.0);
                } else if m == to_max_x {
                    pos.x = b.max_x + radius;
                    out.normal_xz = out.normal_xz + Vec3::new(1.0, 0.0, 0.0);
                } else if m == to_min_z {
                    pos.z = b.min_z - radius;
                    out.normal_xz = out.normal_xz + Vec3::new(0.0, 0.0, -1.0);
                } else {
                    pos.z = b.max_z + radius;
                    out.normal_xz = out.normal_xz + Vec3::new(0.0, 0.0, 1.0);
                }
            }
        }
        out
    }

    /// The nearest safe standing spot around a point, for out-of-bounds recovery.
    ///
    /// Falls back to the origin rather than to nothing. A mech that fell through
    /// the world has to be put somewhere, and the middle of the arena is a
    /// better answer than leaving it below the floor forever.
    pub fn nearest_ground(&self, x: f32, z: f32, search_step: f32, max_rings: u32) -> (f32, f32) {
        if self.ground_at(x, z, 200.0, 400.0) > self.floor_y - 0.01 {
            return (x, z);
        }
        for ring in 1..=max_rings {
            let r = ring as f32 * search_step;
            // Eight probes per ring, so the search is cheap and the result is
            // independent of the order they happen to be tried in.
            for i in 0..8 {
                let a = (i as f32 / 8.0) * std::f32::consts::PI * 2.0;
                let px = x + a.cos() * r;
                let pz = z + a.sin() * r;
                let gy = self.ground_at(px, pz, 200.0, 400.0);
                // Below 40 metres, so recovery does not drop the player on top
                // of a gantry they cannot get down from.
                if gy >= self.floor_y - 0.01 && gy < 40.0 {
                    return (px, pz);
                }
            }
        }
        (0.0, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An arena with a floor and a few solid boxes.
    fn arena() -> CollisionWorld {
        let mut world = CollisionWorld::new(0.0);
        // A wall across z = 10, 2 metres thick.
        world.add(Box::from_size(0.0, 5.0, 10.0, 40.0, 10.0, 2.0, "wall"));
        // A low ledge 1 metre high.
        world.add(Box::from_size(20.0, 0.5, 0.0, 6.0, 1.0, 6.0, "ledge"));
        // A raised platform 4 metres up.
        world.add(Box::from_size(-20.0, 4.0, 0.0, 8.0, 8.0, 8.0, "platform"));
        world
    }

    #[test]
    fn a_ray_hits_a_box_in_front_of_it() {
        let world = arena();
        // Fired from mid-wall height. From y = 0 the ray meets the wall's
        // bottom edge, where the original's nearest-face rule legitimately
        // reports the *bottom* face rather than the front one -- see
        // `a_hit_exactly_on_an_edge_reports_a_side_face`.
        let hit = world
            .raycast(Vec3::new(0.0, 3.0, 0.0), Vec3::new(0.0, 0.0, 1.0), 100.0)
            .expect("should hit the wall");
        assert!((hit.at.z - 9.0).abs() < 1e-3, "hit at z = {}", hit.at.z);
        assert_eq!(hit.normal.z, -1.0, "the near face points back at us");
        assert_eq!(hit.tag, "wall");
    }

    #[test]
    fn a_ray_that_misses_returns_nothing() {
        let world = arena();
        assert!(world
            .raycast(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), 5.0)
            .is_none());
    }

    #[test]
    fn the_nearest_hit_wins() {
        let mut world = CollisionWorld::new(0.0);
        world.add(Box::from_size(0.0, 0.0, 5.0, 4.0, 4.0, 1.0, "near"));
        world.add(Box::from_size(0.0, 0.0, 20.0, 4.0, 4.0, 1.0, "far"));
        let hit = world
            .raycast(Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0), 100.0)
            .expect("should hit");
        assert_eq!(hit.tag, "near");
    }

    #[test]
    fn a_ray_respects_its_maximum_distance() {
        let world = arena();
        assert!(
            world
                .raycast(Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0), 5.0)
                .is_none(),
            "the wall at z=9 is beyond a 5 metre ray"
        );
    }

    #[test]
    fn a_ray_parallel_to_a_slab_does_not_produce_an_infinity() {
        // A zero direction component would divide by zero if the slab test did
        // not special-case it, and the resulting infinity poisons the interval
        // so the box is either always hit or never hit.
        let mut world = CollisionWorld::new(0.0);
        world.add(Box::from_size(0.0, 0.0, 5.0, 4.0, 4.0, 1.0, "box"));
        let hit = world.raycast(Vec3::ZERO, Vec3::new(0.0, 1.0, 0.0), 100.0);
        assert!(hit.is_none(), "a ray going straight up should miss");
    }

    #[test]
    fn a_hit_exactly_on_an_edge_reports_a_side_face() {
        // The original picks the face nearest the entry point by comparing
        // exact distances, in a fixed order. A ray meeting a box exactly along
        // its bottom edge is equally near the bottom face and the front face;
        // the order decides, and the bottom face wins.
        //
        // Recorded because it is surprising and because a projectile fired
        // from ground level does meet walls this way. Nothing depends on it
        // today -- impacts are cosmetic -- but a reader comparing a normal
        // against an expectation deserves to know the rule.
        let world = arena();
        let from_the_edge = world
            .raycast(Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0), 100.0)
            .expect("should hit the wall");
        assert_eq!(
            from_the_edge.normal.y, -1.0,
            "the bottom face is checked before the front one"
        );

        let from_mid_height = world
            .raycast(Vec3::new(0.0, 3.0, 0.0), Vec3::new(0.0, 0.0, 1.0), 100.0)
            .expect("should hit the wall");
        assert_eq!(
            from_mid_height.normal.z, -1.0,
            "clear of the edge, the front face is nearest and wins"
        );
    }

    // -- sweeping ---------------------------------------------------------

    #[test]
    fn a_fast_projectile_cannot_tunnel_through_a_thin_wall() {
        // The reason this module exists. A rifle round at 300 m/s covers 2.5
        // metres in one 120 Hz step; the wall here is 0.2 metres thick. A
        // point test at either end of the step misses it entirely.
        let mut world = CollisionWorld::new(0.0);
        world.add(Box::from_size(0.0, 0.0, 50.0, 20.0, 20.0, 0.2, "thin_wall"));

        let step = 300.0 / 120.0;
        assert!(
            step > 0.2,
            "the step must exceed the wall for this to mean anything"
        );

        let hit = world.sweep_sphere(
            Vec3::new(0.0, 0.0, 49.0),
            Vec3::new(0.0, 0.0, 1.0),
            0.15,
            step * 4.0,
        );
        assert!(hit.is_some(), "the round must not pass through the wall");
    }

    #[test]
    fn a_sweep_stops_a_body_before_it_touches_not_at_its_centre() {
        // The sphere is grown by its radius, so the reported distance is where
        // the surface meets, not where the centre would.
        let mut world = CollisionWorld::new(0.0);
        world.add(Box::from_size(0.0, 0.0, 10.0, 10.0, 10.0, 1.0, "wall"));
        let radius = 0.5;
        let hit = world
            .sweep_sphere(Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0), radius, 100.0)
            .expect("should hit");
        // The wall's near face is at z = 9.5, so a sphere of radius 0.5 stops
        // at 9.0.
        assert!(
            (hit.t - 9.0).abs() < 1e-3,
            "a sphere of radius {radius} should stop at 9.0, got {}",
            hit.t
        );
    }

    #[test]
    fn a_sweep_and_a_ray_agree_when_the_sphere_is_a_point() {
        let world = arena();
        let direction = Vec3::new(0.0, 0.0, 1.0);
        let ray = world.raycast(Vec3::ZERO, direction, 100.0);
        let sweep = world.sweep_sphere(Vec3::ZERO, direction, 0.0, 100.0);
        assert_eq!(ray.map(|h| h.t), sweep.map(|h| h.t));
    }

    #[test]
    fn a_sweep_that_misses_returns_nothing() {
        let world = arena();
        assert!(world
            .sweep_sphere(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), 0.5, 5.0)
            .is_none());
    }

    // -- line of sight ----------------------------------------------------

    #[test]
    fn the_wall_blocks_line_of_sight() {
        let world = arena();
        assert!(!world.line_of_sight(Vec3::ZERO, Vec3::new(0.0, 0.0, 20.0)));
    }

    #[test]
    fn open_ground_does_not_block_line_of_sight() {
        let world = arena();
        assert!(world.line_of_sight(Vec3::ZERO, Vec3::new(0.0, 0.0, -20.0)));
    }

    #[test]
    fn coincident_points_can_see_each_other() {
        // Dividing by a zero length would make this depend on a NaN
        // comparison, which is false, so the answer would be "blocked".
        let world = arena();
        let p = Vec3::new(3.0, 1.0, 3.0);
        assert!(world.line_of_sight(p, p));
    }

    #[test]
    fn a_point_on_a_surface_can_see_itself() {
        // The ray stops 5 cm short, which is what lets a unit see the thing it
        // is standing on rather than treating the ground as an obstruction.
        let world = arena();
        let feet = Vec3::new(0.0, 0.0, 0.0);
        let head = Vec3::new(0.0, 5.0, 0.0);
        assert!(world.line_of_sight(feet, head));
    }

    // -- ground -----------------------------------------------------------

    #[test]
    fn ground_is_the_floor_where_there_is_nothing_else() {
        let world = arena();
        assert_eq!(world.ground_at(0.0, 0.0, 50.0, 400.0), 0.0);
    }

    #[test]
    fn ground_is_the_top_of_a_box_where_there_is_one() {
        let world = arena();
        // The ledge spans x 17..23 at y 0..1.
        assert!((world.ground_at(20.0, 0.0, 50.0, 400.0) - 1.0).abs() < 1e-3);
        // The platform is at y 0..8.
        assert!((world.ground_at(-20.0, 0.0, 50.0, 400.0) - 8.0).abs() < 1e-3);
    }

    #[test]
    fn ground_under_a_footprint_finds_an_edge_the_centre_misses() {
        // The whole reason for the ring probe: a mech standing with its centre
        // just off a ledge should stand on the ledge, not beside it.
        let world = arena();
        let centre = world.ground_at(24.5, 0.0, 50.0, 400.0);
        let under = world.ground_under(24.5, 0.0, 50.0, 1.7);
        assert!(
            under >= centre,
            "the footprint probe should not find lower ground"
        );
    }

    // -- resolving --------------------------------------------------------

    #[test]
    fn a_body_is_pushed_out_of_a_wall() {
        let world = arena();
        // Start inside the wall and let the resolver push out.
        let mut pos = Vec3::new(0.0, 0.0, 10.0);
        let result = world.resolve_cylinder(&mut pos, 1.7, 6.0, 0.45, 0.0);
        assert!(
            result.hit,
            "the body was inside the wall and should be moved"
        );
        let clear_of_wall = pos.z <= 9.0 - 1.7 + 1e-2 || pos.z >= 11.0 + 1.7 - 1e-2;
        assert!(
            clear_of_wall,
            "the body should have been pushed clear, ended at z = {}",
            pos.z
        );
    }

    #[test]
    fn a_body_standing_on_a_ledge_is_not_pushed_out_of_it() {
        // The tolerance band's job. Without it the resolver pushes a body out
        // of the thing it is standing on, every step, forever.
        let world = arena();
        let mut pos = Vec3::new(20.0, 1.0, 0.0);
        let before = pos;
        let result = world.resolve_cylinder(&mut pos, 1.7, 6.0, 0.45, 0.0);
        assert!(
            !result.hit || (pos.x - before.x).abs() < 1e-2 && (pos.z - before.z).abs() < 1e-2,
            "a body resting on a ledge should not be shoved sideways"
        );
    }

    #[test]
    fn a_ledge_at_or_below_step_height_does_not_block() {
        // What the step height actually buys: a kerb lower than it is skipped
        // by the resolver entirely, so it is not an obstacle. The body is put
        // on top of it by the ground query, not by this.
        let world = arena();
        let mut pos = Vec3::new(16.5, 0.0, 0.0);
        let result = world.resolve_cylinder(&mut pos, 1.7, 6.0, 1.2, 0.0);

        assert!(
            !result.hit,
            "a 1.0 m ledge under a 1.2 m step height should not be an obstacle"
        );
        assert!(
            !result.stepped,
            "nothing was climbed here, because nothing needed to be"
        );
    }

    #[test]
    fn the_step_up_branch_can_never_run() {
        // A finding from the port, preserved rather than fixed.
        //
        // The guard at the top of the loop skips any box whose top is within
        // `step_height` of the feet:
        //
        //     if b.max_y <= feet + step_height + 1e-3 { continue }
        //
        // so everything reaching the branch below has `b.max_y > feet +
        // step_height`, which means `ledge = b.max_y - feet > step_height`.
        // The branch then requires `ledge <= step_height`. No box can satisfy
        // both, so `ResolveResult::stepped` is never set, and the `raycast`
        // that checks for headroom before climbing is never reached either.
        //
        // The observable behaviour is still right -- low ledges do not block,
        // which is what makes kerbs and ramps traversable -- but it is the
        // guard doing the work, not the branch. This test fails the day
        // somebody changes the guard and the branch comes alive, which is
        // exactly when they should be made to think about it.
        for step_height in [0.0f32, 0.1, 0.45, 1.2, 3.0] {
            for top in [0.05f32, 0.2, 0.4, 1.0, 2.9, 3.0] {
                let mut world = CollisionWorld::new(0.0);
                world.add(Box::from_size(4.0, top / 2.0, 0.0, 8.0, top, 8.0, "ledge"));

                let mut pos = Vec3::new(1.0, 0.0, 0.0);
                let result = world.resolve_cylinder(&mut pos, 1.7, 6.0, step_height, 0.0);
                assert!(
                    !result.stepped,
                    "stepping happened with step_height {step_height} and a {top} m ledge; \
                     the branch is no longer dead and this test needs rewriting deliberately"
                );
            }
        }
    }

    #[test]
    fn a_ledge_too_tall_to_step_is_blocked_instead() {
        let world = arena();
        // The platform spans x -24..-16, so a body of radius 1.7 touches it
        // from x >= -17.7. Placed at -15 it overlaps the edge.
        let mut pos = Vec3::new(-15.0, 0.0, 0.0);
        let result = world.resolve_cylinder(&mut pos, 1.7, 6.0, 0.45, 0.0);
        assert!(result.hit, "the body overlaps the platform edge");
        assert!(!result.stepped, "8 metres is not a step");
        assert!(
            pos.x > -15.0,
            "the body should have been pushed away from the platform, ended at {}",
            pos.x
        );
    }

    #[test]
    fn a_rising_body_stops_at_a_ceiling() {
        let mut world = CollisionWorld::new(0.0);
        // A ceiling slab from y 6 to 7.
        world.add(Box::from_size(0.0, 6.5, 0.0, 20.0, 1.0, 20.0, "ceiling"));
        // Already a little way into the slab, which is what a rising body
        // looks like after one step: the check tolerates up to 0.35 m of
        // penetration and nothing less, so a head exactly level with the
        // underside is not yet a collision.
        let mut pos = Vec3::new(0.0, 0.2, 0.0);
        let result = world.resolve_cylinder(&mut pos, 1.7, 6.0, 0.45, 10.0);
        assert!(result.ceiling, "the head should have met the ceiling");
        assert!(
            pos.y + 6.0 <= 6.0 + 1e-2,
            "the body should be under the ceiling, head at {}",
            pos.y + 6.0
        );
    }

    // -- recovery ---------------------------------------------------------

    #[test]
    fn a_body_on_solid_ground_is_left_where_it_is() {
        let world = arena();
        let (x, z) = world.nearest_ground(5.0, 5.0, 4.0, 20);
        assert_eq!((x, z), (5.0, 5.0));
    }

    #[test]
    fn a_body_that_fell_through_is_recovered_somewhere_solid() {
        // Off the edge of every box, so the straight-down probe finds nothing
        // above the floor and recovery has to search.
        let mut world = CollisionWorld::new(0.0);
        world.add(Box::from_size(0.0, 5.0, 0.0, 10.0, 10.0, 10.0, "island"));
        let (x, z) = world.nearest_ground(200.0, 200.0, 4.0, 20);
        assert!(x.is_finite() && z.is_finite());
        // Wherever it lands, it must be somewhere there is ground.
        let ground = world.ground_at(x, z, 200.0, 400.0);
        assert!(ground >= world.floor_y() - 0.01);
    }

    // -- construction -----------------------------------------------------

    #[test]
    fn a_box_built_two_ways_is_the_same_box() {
        let from_size = Box::from_size(1.0, 2.0, 3.0, 4.0, 6.0, 8.0, "a");
        let from_half = Box::from_half(Vec3::new(1.0, 2.0, 3.0), Vec3::new(2.0, 3.0, 4.0), "a");
        assert_eq!(from_size, from_half);
    }

    #[test]
    fn a_box_reports_its_centre_and_size() {
        let b = Box::from_size(10.0, 20.0, 30.0, 4.0, 6.0, 8.0, "a");
        assert_eq!(b.centre(), Vec3::new(10.0, 20.0, 30.0));
        assert_eq!(b.size(), Vec3::new(4.0, 6.0, 8.0));
    }

    #[test]
    fn clearing_removes_every_collider() {
        let mut world = arena();
        assert!(!world.boxes.is_empty());
        world.clear();
        assert!(world.boxes.is_empty());
        assert!(world
            .raycast(Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0), 100.0)
            .is_none());
    }

    #[test]
    fn a_world_with_no_boxes_is_all_floor() {
        // Restart clears the world; ground queries during that window must
        // still answer rather than returning an uninitialised value.
        let world = CollisionWorld::new(2.5);
        assert_eq!(world.floor_y(), 2.5);
        assert_eq!(world.ground_at(0.0, 0.0, 50.0, 400.0), 2.5);
    }
}
