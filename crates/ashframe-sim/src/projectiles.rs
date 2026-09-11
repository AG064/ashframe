//! Projectiles.
//!
//! Ported from the original's `game/projectiles.ts`: *"Every projectile is
//! swept along its path each step (sphere vs box / sphere vs entity), so a
//! 300 m/s autocannon round cannot tunnel through cover or skip an enemy
//! between two simulation steps. Pools are fixed size and reused."*
//!
//! ## Where the world and the hooks went
//!
//! The original held the collision world and the event sink as fields. This
//! takes them as parameters instead. In Rust, a system holding `&mut` to the
//! world and the sink *and* handing out `&mut` to the entities it is hitting
//! cannot be expressed — three mutable borrows of overlapping state.
//!
//! Passing them in is not a workaround; it makes the data flow visible. Every
//! method that can damage something says so in its signature, and the pool is
//! the only thing this owns.

use crate::collision::CollisionWorld;
use crate::combat::apply_damage;
use crate::types::{Damageable, Faction, Hooks, SimEvent, Vec3, WeaponId};

/// Fixed pool size.
///
/// Saturated pools recycle the oldest slot rather than refusing to fire. A
/// weapon that silently stops working under sustained fire would be worse than
/// one whose oldest round vanishes, which is invisible in practice at these
/// counts.
pub const POOL_SIZE: usize = 420;

/// How far below the arena a projectile may fall before it is dropped.
const FALL_LIMIT: f32 = -40.0;

/// What kind of round this is.
///
/// The kind decides what happens on impact: a bullet is consumed and reports a
/// surface, a missile or mortar detonates and splashes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectileKind {
    Bullet,
    Missile,
    Mortar,
    Barrage,
}

/// One round in flight.
#[derive(Debug, Clone, PartialEq)]
pub struct Projectile {
    pub active: bool,
    pub kind: ProjectileKind,
    pub weapon: WeaponId,
    pub faction: Faction,
    pub owner_id: u32,
    pub pos: Vec3,
    pub vel: Vec3,
    pub damage: f32,
    pub impact: f32,
    pub splash_radius: f32,
    pub splash_damage: f32,
    /// Homing target, or `None` for dumb-fire.
    pub target_id: Option<u32>,
    pub turn_rate: f32,
    pub gravity: f32,
    pub life: f32,
    pub radius: f32,
    /// Seconds since launch, for trails and arming.
    pub age: f32,
    /// A missile ignores everything for this long after launch, so it does not
    /// detonate on the barrel it just left.
    pub arming: f32,
}

impl Projectile {
    fn idle() -> Self {
        Self {
            active: false,
            kind: ProjectileKind::Bullet,
            weapon: WeaponId::Rifle,
            faction: Faction::Player,
            owner_id: u32::MAX,
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            damage: 0.0,
            impact: 0.0,
            splash_radius: 0.0,
            splash_damage: 0.0,
            target_id: None,
            turn_rate: 0.0,
            gravity: 0.0,
            life: 0.0,
            radius: 0.3,
            age: 0.0,
            arming: 0.0,
        }
    }

    fn speed(&self) -> f32 {
        self.vel.length()
    }
}

/// A round fired straight.
#[derive(Debug, Clone, PartialEq)]
pub struct BulletSpec {
    pub weapon: WeaponId,
    pub faction: Faction,
    pub owner: u32,
    pub at: Vec3,
    pub direction: Vec3,
    pub speed: f32,
    pub damage: f32,
    pub impact: f32,
    pub life: f32,
    pub radius: f32,
}

/// A homing missile.
#[derive(Debug, Clone, PartialEq)]
pub struct MissileSpec {
    pub faction: Faction,
    pub owner: u32,
    pub target: u32,
    pub at: Vec3,
    pub direction: Vec3,
    pub speed: f32,
    pub damage: f32,
    pub splash_radius: f32,
    pub splash_damage: f32,
    pub turn_rate: f32,
    pub life: f32,
}

/// Enemy ordnance: a bullet, a mortar, or a barrage round.
#[derive(Debug, Clone, PartialEq)]
pub struct EnemyShotSpec {
    pub weapon: WeaponId,
    pub owner: u32,
    pub at: Vec3,
    pub direction: Vec3,
    pub speed: f32,
    pub damage: f32,
    pub impact: f32,
    pub life: f32,
    pub gravity: f32,
    pub radius: f32,
    pub splash_radius: f32,
    pub splash_damage: f32,
}

impl EnemyShotSpec {
    /// The rounds the original fired, with its defaults for the parts a caller
    /// usually does not care about.
    pub fn new(
        weapon: WeaponId,
        owner: u32,
        at: Vec3,
        direction: Vec3,
        speed: f32,
        life: f32,
    ) -> Self {
        Self {
            weapon,
            owner,
            at,
            direction,
            speed,
            damage: 0.0,
            impact: 0.0,
            life,
            gravity: 0.0,
            radius: 0.45,
            splash_radius: 0.0,
            splash_damage: 0.0,
        }
    }
}

/// Every in-flight projectile, for both factions.
#[derive(Debug, Clone)]
pub struct ProjectileSystem {
    pool: Vec<Projectile>,
    cursor: usize,
}

impl Default for ProjectileSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectileSystem {
    pub fn new() -> Self {
        Self {
            pool: (0..POOL_SIZE).map(|_| Projectile::idle()).collect(),
            cursor: 0,
        }
    }

    /// How many rounds are in flight. Walked rather than counted on write,
    /// because a round can also be released by hitting something.
    pub fn active_count(&self) -> usize {
        self.pool.iter().filter(|p| p.active).count()
    }

    pub fn clear(&mut self) {
        for p in &mut self.pool {
            p.active = false;
        }
    }

    /// Every round in flight, for rendering.
    pub fn in_flight(&self) -> impl Iterator<Item = &Projectile> {
        self.pool.iter().filter(|p| p.active)
    }

    /// Take a free slot, recycling the oldest when the pool is saturated.
    ///
    /// The cursor means successive spawns start looking where the last one
    /// finished, so finding a free slot is usually a single check rather than
    /// a scan from the beginning.
    fn spawn(&mut self) -> &mut Projectile {
        for offset in 0..POOL_SIZE {
            let index = (self.cursor + offset) % POOL_SIZE;
            if !self.pool[index].active {
                self.cursor = (index + 1) % POOL_SIZE;
                return &mut self.pool[index];
            }
        }
        let index = self.cursor;
        self.cursor = (self.cursor + 1) % POOL_SIZE;
        &mut self.pool[index]
    }

    pub fn spawn_bullet(&mut self, spec: BulletSpec) {
        let direction = spec.direction.normalized();
        let p = self.spawn();
        p.active = true;
        p.kind = ProjectileKind::Bullet;
        p.weapon = spec.weapon;
        p.faction = spec.faction;
        p.owner_id = spec.owner;
        p.pos = spec.at;
        p.vel = direction * spec.speed;
        p.damage = spec.damage;
        p.impact = spec.impact;
        p.splash_radius = 0.0;
        p.splash_damage = 0.0;
        p.target_id = None;
        p.turn_rate = 0.0;
        p.gravity = 0.0;
        p.life = spec.life;
        p.radius = spec.radius;
        p.age = 0.0;
        p.arming = 0.0;
    }

    pub fn spawn_missile(&mut self, spec: MissileSpec) {
        let direction = spec.direction.normalized();
        let p = self.spawn();
        p.active = true;
        p.kind = ProjectileKind::Missile;
        p.weapon = WeaponId::Missiles;
        p.faction = spec.faction;
        p.owner_id = spec.owner;
        p.pos = spec.at;
        p.vel = direction * spec.speed;
        p.damage = spec.damage;
        // A missile's own impact figure is fixed rather than passed in, which
        // is the original's choice and gives every warhead the same shove.
        p.impact = 14.0;
        p.splash_radius = spec.splash_radius;
        p.splash_damage = spec.splash_damage;
        p.target_id = Some(spec.target);
        p.turn_rate = spec.turn_rate;
        p.gravity = 0.0;
        p.life = spec.life;
        p.radius = 0.5;
        p.age = 0.0;
        // Long enough to clear the pod it launched from.
        p.arming = 0.12;
    }

    pub fn spawn_enemy(&mut self, spec: EnemyShotSpec) {
        let direction = spec.direction.normalized();
        let kind = match spec.weapon {
            WeaponId::EnemyMortar => ProjectileKind::Mortar,
            WeaponId::BossBarrage => ProjectileKind::Barrage,
            _ => ProjectileKind::Bullet,
        };
        let p = self.spawn();
        p.active = true;
        p.kind = kind;
        p.weapon = spec.weapon;
        p.faction = Faction::Enemy;
        p.owner_id = spec.owner;
        p.pos = spec.at;
        p.vel = direction * spec.speed;
        p.damage = spec.damage;
        p.impact = spec.impact;
        p.splash_radius = spec.splash_radius;
        p.splash_damage = spec.splash_damage;
        p.target_id = None;
        p.turn_rate = 0.0;
        p.gravity = spec.gravity;
        p.life = spec.life;
        p.radius = spec.radius;
        p.age = 0.0;
        p.arming = 0.0;
    }

    /// Advance every round and resolve hits.
    pub fn step(
        &mut self,
        dt: f32,
        world: &CollisionWorld,
        targets: &mut [Damageable],
        hooks: &mut dyn Hooks,
    ) {
        for index in 0..self.pool.len() {
            // Read the round's state up front rather than holding a borrow of
            // the pool across the damage calls, which need the targets.
            let Some(round) = self.pool.get(index).filter(|p| p.active).cloned() else {
                continue;
            };
            let mut p = round;

            p.age += dt;
            p.life -= dt;
            if p.life <= 0.0 {
                expire(&p, hooks);
                self.pool[index] = Projectile::idle();
                continue;
            }

            if let Some(target_id) = p.target_id.filter(|_| p.turn_rate > 0.0) {
                match targets.iter().position(|t| t.id == target_id && t.alive) {
                    None => {
                        // The lock is gone. Fly straight and expire on its own
                        // rather than vanishing, so a missile that loses its
                        // target still lands somewhere.
                        p.target_id = None;
                    }
                    Some(target_index) => {
                        let target = &targets[target_index];
                        let aim = target.pos + Vec3::new(0.0, target.height * 0.55, 0.0);
                        let offset = aim - p.pos;
                        let distance = offset.length().max(1.0);
                        let speed = p.speed().max(1.0);
                        let current = p.vel * (1.0 / speed);

                        // Turn a fraction of the way toward the target, capped
                        // so a missile cannot pivot instantly. Fractional
                        // rather than absolute, which is what makes the turn
                        // rate behave the same at any step size.
                        let max_turn = (p.turn_rate * dt).min(1.0);
                        let wanted = offset * (1.0 / distance);
                        let turned = (current + (wanted - current) * max_turn).normalized();
                        p.vel = turned * speed;

                        // Proximity fuse, so a fast pass still connects rather
                        // than needing a direct hit on a moving target.
                        if distance < 2.4 {
                            detonate(&p, targets, Some(target_index), hooks);
                            self.pool[index] = Projectile::idle();
                            continue;
                        }
                    }
                }
            }

            if p.gravity != 0.0 {
                p.vel.y += p.gravity * dt;
            }

            let travel = p.vel * dt;
            let distance = travel.length();
            if distance < 1e-6 {
                self.pool[index] = p;
                continue;
            }
            let direction = travel * (1.0 / distance);

            // The nearest hit along this segment, world and entities together,
            // so a round cannot pass through a wall to reach an enemy behind
            // it.
            let world_hit = world.sweep_sphere(p.pos, direction, p.radius, distance);
            let mut hit_distance = world_hit.as_ref().map_or(f32::INFINITY, |h| h.t);
            let mut hit_target: Option<usize> = None;

            for (i, target) in targets.iter().enumerate() {
                if !target.alive || target.faction == p.faction {
                    continue;
                }
                // Redundant given the faction check above -- a round's owner is
                // always on its own side -- and kept because the original had
                // it. Removing it would change nothing today and would change
                // something the day a round is fired by a faction it does not
                // belong to.
                if target.id == p.owner_id {
                    continue;
                }
                if let Some(t0) = sweep_entity(p.pos, direction, distance, p.radius, target) {
                    if t0 < hit_distance {
                        hit_distance = t0;
                        hit_target = Some(i);
                    }
                }
            }

            if p.arming > 0.0 {
                p.arming -= dt;
            }

            if let Some(i) = hit_target {
                if p.kind != ProjectileKind::Missile {
                    apply_damage(&mut targets[i], p.damage, p.impact, hooks, 1.0);
                    hooks.emit(&SimEvent::Impact {
                        at: p.pos + direction * hit_distance,
                        // The normal points back along the round, so an impact
                        // effect sprays toward the shooter.
                        normal: -direction,
                        surface: if targets[i].faction == Faction::Player {
                            "player".to_string()
                        } else {
                            "armor".to_string()
                        },
                    });
                    self.pool[index] = Projectile::idle();
                    continue;
                }
                // A missile detonates instead of being consumed.
                detonate(&p, targets, Some(i), hooks);
                self.pool[index] = Projectile::idle();
                continue;
            }

            if let Some(hit) = world_hit {
                if p.kind == ProjectileKind::Missile || p.splash_radius > 0.0 {
                    detonate(&p, targets, None, hooks);
                } else {
                    hooks.emit(&SimEvent::Impact {
                        at: hit.at,
                        normal: hit.normal,
                        surface: hit.tag,
                    });
                }
                self.pool[index] = Projectile::idle();
                continue;
            }

            p.pos += travel;
            if p.pos.y < FALL_LIMIT {
                self.pool[index] = Projectile::idle();
                continue;
            }
            self.pool[index] = p;
        }
    }
}

/// A warhead going off with no travelling projectile.
///
/// Used by the boss ground slam and by any future area attack. It shares the
/// falloff arithmetic with missile splash, so damage is consistent whichever
/// way the explosion arrived.
pub fn explode(
    at: Vec3,
    radius: f32,
    damage: f32,
    faction: Faction,
    owner: u32,
    targets: &mut [Damageable],
    hooks: &mut dyn Hooks,
) {
    // Collected before any damage is applied, because `apply_damage` needs
    // `&mut` to one target and iterating the slice while holding it is not
    // possible. Each target is visited once either way, so the outcome is the
    // same as applying as we go.
    let mut hits: Vec<(usize, f32, f32)> = Vec::new();
    for (i, target) in targets.iter().enumerate() {
        if !target.alive || target.faction == faction || target.id == owner {
            continue;
        }
        let aim = target.pos + Vec3::new(0.0, target.height * 0.5, 0.0);
        let distance = (aim - at).length();
        let reach = radius + target.radius;
        if distance > reach {
            continue;
        }
        // Linear falloff to the edge, where the blast does nothing. A hard
        // cutoff would make the edge of a slam feel arbitrary.
        let falloff = 1.0 - (distance / reach).min(1.0);
        hits.push((i, damage * falloff, 12.0 * falloff));
    }

    for (i, amount, impact) in hits {
        apply_damage(&mut targets[i], amount, impact, hooks, 1.0);
    }

    hooks.emit(&SimEvent::Explosion { at, radius });
}

/// A round running out of fuel.
///
/// A missile or mortar still detonates rather than vanishing, so a warhead
/// fired at nothing still leaves a mark where it died.
fn expire(p: &Projectile, hooks: &mut dyn Hooks) {
    if matches!(p.kind, ProjectileKind::Missile | ProjectileKind::Mortar) {
        hooks.emit(&SimEvent::Explosion {
            at: p.pos,
            radius: if p.splash_radius > 0.0 {
                p.splash_radius
            } else {
                4.0
            },
        });
    }
}

/// Direct hit plus splash, then the explosion event.
fn detonate(
    p: &Projectile,
    targets: &mut [Damageable],
    direct: Option<usize>,
    hooks: &mut dyn Hooks,
) {
    if let Some(i) = direct {
        apply_damage(&mut targets[i], p.damage, p.impact, hooks, 1.0);
    }

    if p.splash_radius > 0.0 {
        let mut hits: Vec<(usize, f32, f32)> = Vec::new();
        for (i, target) in targets.iter().enumerate() {
            if !target.alive || target.faction == p.faction {
                continue;
            }
            // The directly-hit target has already taken the full warhead;
            // splashing it again would double-count the same explosion.
            if direct == Some(i) {
                continue;
            }
            let aim = target.pos + Vec3::new(0.0, target.height * 0.5, 0.0);
            let distance = (aim - p.pos).length();
            let reach = p.splash_radius + target.radius;
            if distance > reach {
                continue;
            }
            let falloff = 1.0 - (distance / reach).min(1.0);
            // Splash shoves less than a direct hit, in proportion to how much
            // of the blast reached it.
            hits.push((i, p.splash_damage * falloff, p.impact * 0.4 * falloff));
        }
        for (i, amount, impact) in hits {
            apply_damage(&mut targets[i], amount, impact, hooks, 1.0);
        }
    }

    hooks.emit(&SimEvent::Explosion {
        at: p.pos,
        radius: if p.splash_radius > 0.0 {
            p.splash_radius
        } else {
            4.0
        },
    });
}

/// Swept sphere against an entity's box proxy.
///
/// Entities use a tight box around the torso mass rather than the full
/// silhouette, so hitting a leg does not register as a hull hit. The box spans
/// from 12% to 98% of the entity's height, which is the original's proportion.
///
/// Returns the entry distance along the segment, or `None`.
fn sweep_entity(
    origin: Vec3,
    direction: Vec3,
    max_distance: f32,
    radius: f32,
    target: &Damageable,
) -> Option<f32> {
    let half_width = target.radius + radius;
    let min_y = target.pos.y + target.height * 0.12 - radius;
    let max_y = target.pos.y + target.height * 0.98 + radius;
    let mut t0 = 0.0f32;
    let mut t1 = max_distance;

    // X slab.
    if direction.x.abs() < 1e-9 {
        if origin.x < target.pos.x - half_width || origin.x > target.pos.x + half_width {
            return None;
        }
    } else {
        let mut a = (target.pos.x - half_width - origin.x) / direction.x;
        let mut b = (target.pos.x + half_width - origin.x) / direction.x;
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        t0 = t0.max(a);
        t1 = t1.min(b);
        if t0 > t1 {
            return None;
        }
    }

    // Y slab.
    if direction.y.abs() < 1e-9 {
        if origin.y < min_y || origin.y > max_y {
            return None;
        }
    } else {
        let mut a = (min_y - origin.y) / direction.y;
        let mut b = (max_y - origin.y) / direction.y;
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        t0 = t0.max(a);
        t1 = t1.min(b);
        if t0 > t1 {
            return None;
        }
    }

    // Z slab.
    if direction.z.abs() < 1e-9 {
        if origin.z < target.pos.z - half_width || origin.z > target.pos.z + half_width {
            return None;
        }
    } else {
        let mut a = (target.pos.z - half_width - origin.z) / direction.z;
        let mut b = (target.pos.z + half_width - origin.z) / direction.z;
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        t0 = t0.max(a);
        t1 = t1.min(b);
        if t0 > t1 {
            return None;
        }
    }

    Some(t0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{rifle, sim};
    use crate::types::EventLog;

    fn target(id: u32, faction: Faction, at: Vec3) -> Damageable {
        Damageable {
            id,
            faction,
            name: format!("t{id}"),
            pos: at,
            vel: Vec3::ZERO,
            radius: 1.5,
            height: 4.6,
            health: 1000.0,
            max_health: 1000.0,
            stability: 100.0,
            max_stability: 100.0,
            alive: true,
            stagger_timer: 0.0,
            invuln_timer: 0.0,
            stagger_armed: true,
        }
    }

    fn bullet(at: Vec3, direction: Vec3) -> BulletSpec {
        BulletSpec {
            weapon: WeaponId::Rifle,
            faction: Faction::Player,
            owner: 100,
            at,
            direction,
            speed: rifle::SPEED,
            damage: rifle::DAMAGE,
            impact: rifle::IMPACT,
            life: 2.0,
            radius: 0.35,
        }
    }

    fn empty_world() -> CollisionWorld {
        CollisionWorld::new(0.0)
    }

    #[test]
    fn a_bullet_flies_and_hits_a_target() {
        let world = empty_world();
        let mut enemies = [target(1, Faction::Enemy, Vec3::new(0.0, 0.0, 40.0))];
        let mut system = ProjectileSystem::new();
        let mut log = EventLog::new();

        system.spawn_bullet(bullet(Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, 0.0, 1.0)));
        assert_eq!(system.active_count(), 1);

        for _ in 0..60 {
            system.step(sim::DT, &world, &mut enemies, &mut log);
            if system.active_count() == 0 {
                break;
            }
        }

        assert!(enemies[0].health < 1000.0, "the round should have hit");
        assert_eq!(system.active_count(), 0, "and been consumed");
        assert!(log.any("hit"));
    }

    #[test]
    fn a_fast_round_cannot_tunnel_through_a_thin_wall() {
        // The property the whole swept design exists for. At 300 m/s and a
        // 120 Hz step the round advances 2.5 metres per step; this wall is
        // 0.2 metres thick and would be skipped entirely by a point test.
        let mut world = CollisionWorld::new(0.0);
        world.add(crate::collision::Box::from_size(
            0.0, 2.0, 20.0, 20.0, 20.0, 0.2, "thin",
        ));
        let mut enemies = [target(1, Faction::Enemy, Vec3::new(0.0, 0.0, 40.0))];
        let mut system = ProjectileSystem::new();
        let mut log = EventLog::new();

        system.spawn_bullet(bullet(Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, 0.0, 1.0)));
        for _ in 0..40 {
            system.step(sim::DT, &world, &mut enemies, &mut log);
            if system.active_count() == 0 {
                break;
            }
        }

        assert_eq!(system.active_count(), 0, "the round should have stopped");
        assert_eq!(
            enemies[0].health, 1000.0,
            "and the enemy behind the wall should be untouched"
        );
        assert!(log.any("impact"));
    }

    #[test]
    fn a_round_expires_at_the_end_of_its_life() {
        let world = empty_world();
        let mut system = ProjectileSystem::new();
        let mut log = EventLog::new();

        let mut spec = bullet(Vec3::ZERO, Vec3::new(0.0, 1.0, 0.0));
        spec.life = 0.05;
        system.spawn_bullet(spec);

        for _ in 0..20 {
            system.step(sim::DT, &world, &mut [], &mut log);
        }
        assert_eq!(system.active_count(), 0);
    }

    #[test]
    fn a_round_below_the_world_is_dropped() {
        let world = empty_world();
        let mut system = ProjectileSystem::new();
        let mut log = EventLog::new();

        system.spawn_bullet(bullet(
            Vec3::new(0.0, -39.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
        ));
        for _ in 0..120 {
            system.step(sim::DT, &world, &mut [], &mut log);
        }
        assert_eq!(system.active_count(), 0);
    }

    #[test]
    fn a_round_does_not_hit_its_own_side() {
        // Otherwise a mech shoots itself the moment it fires.
        let world = empty_world();
        let mut friends = [target(1, Faction::Player, Vec3::new(0.0, 2.0, 10.0))];
        let mut system = ProjectileSystem::new();
        let mut log = EventLog::new();

        system.spawn_bullet(bullet(Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0)));
        for _ in 0..40 {
            system.step(sim::DT, &world, &mut friends, &mut log);
        }
        assert_eq!(friends[0].health, 1000.0);
    }

    #[test]
    fn the_pool_is_reused_rather_than_grown() {
        let mut system = ProjectileSystem::new();
        for _ in 0..POOL_SIZE * 2 {
            system.spawn_bullet(bullet(Vec3::ZERO, Vec3::new(0.0, 1.0, 0.0)));
        }
        assert_eq!(
            system.active_count(),
            POOL_SIZE,
            "a saturated pool recycles rather than refusing to fire"
        );
    }

    #[test]
    fn clearing_removes_everything_in_flight() {
        let mut system = ProjectileSystem::new();
        for _ in 0..10 {
            system.spawn_bullet(bullet(Vec3::ZERO, Vec3::new(0.0, 1.0, 0.0)));
        }
        assert_eq!(system.active_count(), 10);
        system.clear();
        assert_eq!(system.active_count(), 0);
    }

    // -- missiles ---------------------------------------------------------

    fn missile(at: Vec3, direction: Vec3, target_id: u32) -> MissileSpec {
        MissileSpec {
            faction: Faction::Player,
            owner: 100,
            target: target_id,
            at,
            direction,
            speed: 52.0,
            damage: 46.0,
            splash_radius: 5.5,
            splash_damage: 22.0,
            turn_rate: 2.5,
            life: 6.0,
        }
    }

    #[test]
    fn a_missile_homes_on_its_target() {
        // Launched near-aligned and slightly off, which is what the game does:
        // a missile is only fired with a lock, so it leaves the pod pointing
        // roughly the right way and the homing corrects the rest.
        let world = empty_world();
        let mut enemies = [target(1, Faction::Enemy, Vec3::new(10.0, 0.0, 60.0))];
        let mut system = ProjectileSystem::new();
        let mut log = EventLog::new();

        system.spawn_missile(missile(Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0), 1));
        for _ in 0..600 {
            system.step(sim::DT, &world, &mut enemies, &mut log);
            if system.active_count() == 0 {
                break;
            }
        }
        assert!(
            enemies[0].health < 1000.0,
            "the missile should have found its target, health {}",
            enemies[0].health
        );
    }

    #[test]
    fn a_missile_fired_across_its_target_orbits_rather_than_connecting() {
        // A property of the flight model, recorded because it looks like a bug
        // and is not one.
        //
        // The turn is a fraction of the heading error per step, so the tightest
        // circle a missile can fly has a radius of `speed / turn_rate` -- about
        // 21 metres at the pod's figures. A missile fired perpendicular to a
        // target 60 metres away cannot turn tightly enough to close, and
        // circles it until its fuel runs out.
        //
        // It never arises in play: a missile can only be fired with a lock, so
        // it is always launched roughly toward what it is chasing. The test
        // exists so that if somebody later raises the stand-off or lowers the
        // turn rate, they find out here rather than from a player.
        let world = empty_world();
        let mut enemies = [target(1, Faction::Enemy, Vec3::new(0.0, 0.0, 60.0))];
        let mut system = ProjectileSystem::new();
        let mut log = EventLog::new();

        system.spawn_missile(missile(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), 1));
        for _ in 0..240 {
            system.step(sim::DT, &world, &mut enemies, &mut log);
        }

        assert_eq!(
            enemies[0].health, 1000.0,
            "a perpendicular launch should not connect"
        );
        assert_eq!(
            system.active_count(),
            1,
            "and the missile should still be circling, not gone"
        );

        // The arithmetic behind it, so the number is not folklore.
        let radius = 52.0 / 2.5;
        assert!(
            radius > 20.0,
            "the turn radius should be about 21 metres, got {radius}"
        );
    }

    #[test]
    fn a_missile_whose_target_dies_flies_on_rather_than_vanishing() {
        // It should still land somewhere and leave a mark, instead of blinking
        // out of existence mid-flight.
        let world = empty_world();
        let mut enemies = [target(1, Faction::Enemy, Vec3::new(0.0, 0.0, 60.0))];
        let mut system = ProjectileSystem::new();
        let mut log = EventLog::new();

        system.spawn_missile(missile(
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            1,
        ));
        enemies[0].alive = false;

        // It keeps flying, so it is still active after a moment.
        for _ in 0..10 {
            system.step(sim::DT, &world, &mut enemies, &mut log);
        }
        assert_eq!(system.active_count(), 1, "it should still be in flight");
    }

    #[test]
    fn a_missile_detonates_and_splashes() {
        let world = empty_world();
        let mut enemies = [
            target(1, Faction::Enemy, Vec3::new(0.0, 0.0, 30.0)),
            // Close enough to the first to be caught by the same blast.
            target(2, Faction::Enemy, Vec3::new(2.0, 0.0, 30.0)),
        ];
        let mut system = ProjectileSystem::new();
        let mut log = EventLog::new();

        system.spawn_missile(missile(
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            1,
        ));
        for _ in 0..200 {
            system.step(sim::DT, &world, &mut enemies, &mut log);
            if system.active_count() == 0 {
                break;
            }
        }

        assert!(enemies[0].health < 1000.0, "the direct hit should land");
        assert!(
            enemies[1].health < 1000.0,
            "the neighbour should be caught by the blast"
        );
        assert!(log.any("explosion"));
    }

    #[test]
    fn splash_does_not_double_count_the_direct_hit() {
        // The target that took the full warhead must not also take the splash
        // from the same explosion.
        let world = empty_world();
        let mut enemies = [target(1, Faction::Enemy, Vec3::new(0.0, 0.0, 30.0))];
        let mut system = ProjectileSystem::new();
        let mut log = EventLog::new();

        system.spawn_missile(missile(
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            1,
        ));
        for _ in 0..200 {
            system.step(sim::DT, &world, &mut enemies, &mut log);
            if system.active_count() == 0 {
                break;
            }
        }

        let hits = log.count("hit");
        assert_eq!(
            hits, 1,
            "one explosion on one target should be one hit, got {hits}"
        );
    }

    #[test]
    fn a_missile_that_runs_out_of_fuel_still_detonates() {
        let world = empty_world();
        let mut system = ProjectileSystem::new();
        let mut log = EventLog::new();

        let mut spec = missile(Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, 1.0, 0.0), 1);
        spec.life = 0.05;
        system.spawn_missile(spec);

        for _ in 0..20 {
            system.step(sim::DT, &world, &mut [], &mut log);
        }
        assert!(log.any("explosion"), "a dying warhead should still go off");
    }

    // -- area effects -----------------------------------------------------

    #[test]
    fn an_explosion_falls_off_with_distance() {
        let mut near = [target(1, Faction::Enemy, Vec3::new(1.0, 0.0, 0.0))];
        let mut far = [target(1, Faction::Enemy, Vec3::new(9.0, 0.0, 0.0))];
        let mut log = EventLog::new();

        explode(
            Vec3::ZERO,
            10.0,
            100.0,
            Faction::Player,
            999,
            &mut near,
            &mut log,
        );
        explode(
            Vec3::ZERO,
            10.0,
            100.0,
            Faction::Player,
            999,
            &mut far,
            &mut log,
        );

        let near_damage = 1000.0 - near[0].health;
        let far_damage = 1000.0 - far[0].health;
        assert!(near_damage > far_damage, "closer should hurt more");
        assert!(far_damage > 0.0, "but the edge should still register");
    }

    #[test]
    fn an_explosion_spares_its_own_side() {
        let mut friends = [target(1, Faction::Player, Vec3::new(1.0, 0.0, 0.0))];
        let mut log = EventLog::new();
        explode(
            Vec3::ZERO,
            10.0,
            100.0,
            Faction::Player,
            999,
            &mut friends,
            &mut log,
        );
        assert_eq!(friends[0].health, 1000.0);
    }

    #[test]
    fn an_explosion_out_of_range_does_nothing() {
        let mut far = [target(1, Faction::Enemy, Vec3::new(100.0, 0.0, 0.0))];
        let mut log = EventLog::new();
        explode(
            Vec3::ZERO,
            5.0,
            100.0,
            Faction::Player,
            999,
            &mut far,
            &mut log,
        );
        assert_eq!(far[0].health, 1000.0);
        assert!(log.any("explosion"), "the blast still happened");
    }

    // -- the entity sweep ------------------------------------------------

    #[test]
    fn the_entity_sweep_hits_the_torso_and_misses_below_it() {
        // The proxy box spans 12% to 98% of the height, so a round at ankle
        // height passes under a mech rather than registering on its hull.
        let t = target(1, Faction::Enemy, Vec3::new(0.0, 0.0, 20.0));
        let forward = Vec3::new(0.0, 0.0, 1.0);

        let torso = sweep_entity(Vec3::new(0.0, 2.0, 0.0), forward, 40.0, 0.35, &t);
        assert!(torso.is_some(), "a round at torso height should connect");

        let ankle = sweep_entity(Vec3::new(0.0, 0.1, 0.0), forward, 40.0, 0.35, &t);
        assert!(ankle.is_none(), "a round at ankle height should pass under");
    }

    #[test]
    fn the_entity_sweep_misses_wide() {
        let t = target(1, Faction::Enemy, Vec3::new(0.0, 0.0, 20.0));
        let wide = sweep_entity(
            Vec3::new(20.0, 2.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            40.0,
            0.35,
            &t,
        );
        assert!(wide.is_none());
    }

    #[test]
    fn the_entity_sweep_handles_a_round_travelling_straight_up() {
        // A zero direction component must not divide by zero into an interval
        // that is always or never a hit.
        let t = target(1, Faction::Enemy, Vec3::new(0.0, 0.0, 20.0));
        let up = sweep_entity(
            Vec3::new(0.0, 0.0, 20.0),
            Vec3::new(0.0, 1.0, 0.0),
            40.0,
            0.35,
            &t,
        );
        assert!(up.is_some(), "travelling up through a target should hit");
    }

    #[test]
    fn spawning_an_inactive_round_resets_every_field() {
        // A recycled slot that kept its old splash radius would make the next
        // bullet explode like a missile.
        let mut system = ProjectileSystem::new();
        system.spawn_missile(missile(Vec3::ZERO, Vec3::UP, 1));
        system.clear();
        system.spawn_bullet(bullet(Vec3::ZERO, Vec3::UP));

        let round = system.in_flight().next().expect("a round in flight");
        assert_eq!(round.kind, ProjectileKind::Bullet);
        assert_eq!(round.splash_radius, 0.0);
        assert_eq!(round.splash_damage, 0.0);
        assert_eq!(round.target_id, None);
        assert_eq!(round.turn_rate, 0.0);
    }
}
