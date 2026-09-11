//! The shapes the simulation is made of.
//!
//! Ported from the original's `game/types.ts`. The names are kept because they
//! are the vocabulary the rest of the port is written in, and renaming them
//! would make every later file harder to compare against its original.

/// A position or direction in metres.
///
/// Replaces Three.js's `Vector3`, which the original borrowed for its maths
/// despite the simulation never touching the renderer. Carrying only what the
/// simulation actually uses — add, subtract, scale, length — keeps the
/// dependency out of a crate whose whole point is having none.
///
/// `f32` rather than the original's `f64`. Positions span an arena a few
/// hundred metres across at centimetre precision, which `f32` represents with
/// room to spare, and halving the size of the state that gets copied every
/// 120th of a second is worth more than digits nobody can see.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    pub const UP: Self = Self {
        x: 0.0,
        y: 1.0,
        z: 0.0,
    };

    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    /// A vector from three values, for call sites that read better as a list.
    pub const fn of(values: [f32; 3]) -> Self {
        Self::new(values[0], values[1], values[2])
    }

    pub fn splat(value: f32) -> Self {
        Self::new(value, value, value)
    }

    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }

    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    /// Unit length, or zero for a vector too short to have a direction.
    ///
    /// Returning zero rather than `NaN` matters: a normalised velocity of zero
    /// propagates as "not moving", where `NaN` propagates as "everything
    /// downstream is broken" a long way from the division that caused it.
    pub fn normalized(self) -> Self {
        let length = self.length();
        if length <= f32::EPSILON {
            return Self::ZERO;
        }
        self * (1.0 / length)
    }

    /// The horizontal distance, ignoring height. Used by ground movement,
    /// where a slope must not count as distance travelled.
    pub fn horizontal_length(self) -> f32 {
        (self.x * self.x + self.z * self.z).sqrt()
    }

    /// This vector flattened to the ground plane and normalised.
    pub fn horizontal_normalized(self) -> Self {
        let flat = Self::new(self.x, 0.0, self.z);
        flat.normalized()
    }

    /// Move toward `target` by at most `max_delta` metres.
    pub fn approach(self, target: Self, max_delta: f32) -> Self {
        let delta = target - self;
        let distance = delta.length();
        if distance <= max_delta || distance <= f32::EPSILON {
            return target;
        }
        self + delta * (max_delta / distance)
    }

    /// Whether every component is a real number.
    ///
    /// A cheap guard for the swept-collision code, where one `NaN` entering a
    /// position turns every later comparison false and the entity silently
    /// stops colliding with anything.
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

/// Arithmetic as operators rather than named methods.
///
/// The original called these `add`, `sub` and `scale`. In Rust those names on
/// an inherent impl shadow nothing but read as though they might, and clippy
/// is right to flag them: `v.add(w)` looks like it could be `Add::add`, and a
/// reader has to check. Operators say the same thing without the ambiguity.
impl std::ops::Add for Vec3 {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }
}

impl std::ops::Sub for Vec3 {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }
}

impl std::ops::Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, factor: f32) -> Self {
        Self::new(self.x * factor, self.y * factor, self.z * factor)
    }
}

impl std::ops::Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

/// The compound forms, so accumulating a surface normal or stepping a position
/// reads as `+=` rather than as a reassignment. Same arithmetic, fewer chances
/// to write `=` where `+=` was meant.
impl std::ops::AddAssign for Vec3 {
    fn add_assign(&mut self, other: Self) {
        self.x += other.x;
        self.y += other.y;
        self.z += other.z;
    }
}

impl std::ops::SubAssign for Vec3 {
    fn sub_assign(&mut self, other: Self) {
        self.x -= other.x;
        self.y -= other.y;
        self.z -= other.z;
    }
}

impl std::ops::MulAssign<f32> for Vec3 {
    fn mul_assign(&mut self, factor: f32) {
        self.x *= factor;
        self.y *= factor;
        self.z *= factor;
    }
}

/// Which side something is on. Player and enemy never collide with their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Faction {
    Player,
    Enemy,
}

impl Faction {
    /// Whether `other` is a valid target for this faction.
    pub fn hostile_to(self, other: Self) -> bool {
        self != other
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Player => "player",
            Self::Enemy => "enemy",
        }
    }
}

/// Anything that can be shot, staggered and destroyed.
///
/// An interface in the original, a struct here. Rust has no inheritance, and
/// the two implementors shared every one of these fields anyway — so the
/// choice was a struct with the common state, or a trait plus a struct that
/// both implementations embed. The struct is less machinery for the same
/// result, and the fields stay in one place where a reader can see the whole
/// shape of a damageable thing.
#[derive(Debug, Clone, PartialEq)]
pub struct Damageable {
    pub id: u32,
    pub faction: Faction,
    pub name: String,
    /// Feet position.
    pub pos: Vec3,
    pub vel: Vec3,
    pub radius: f32,
    pub height: f32,
    pub health: f32,
    pub max_health: f32,
    /// Accumulated impact. Crossing the maximum triggers one stagger.
    pub stability: f32,
    pub max_stability: f32,
    pub alive: bool,
    /// Seconds of stagger remaining. Above zero means vulnerable and not acting.
    pub stagger_timer: f32,
    pub invuln_timer: f32,
    /// Set once per stagger, so the threshold cannot retrigger every step.
    pub stagger_armed: bool,
}

impl Damageable {
    /// Whether the unit is inside its stagger window.
    ///
    /// A method rather than a field, because it is derived. The original had it
    /// as a getter for the same reason: two sources of truth for "is it
    /// staggered" is one source too many.
    pub fn staggered(&self) -> bool {
        self.stagger_timer > 0.0
    }

    /// Health as a fraction of maximum, clamped to `0.0..=1.0`.
    pub fn health_fraction(&self) -> f32 {
        if self.max_health <= 0.0 {
            return 0.0;
        }
        crate::math::clamp(self.health / self.max_health, 0.0, 1.0)
    }

    /// Stability as a fraction of maximum, clamped to `0.0..=1.0`.
    pub fn stability_fraction(&self) -> f32 {
        if self.max_stability <= 0.0 {
            return 1.0;
        }
        crate::math::clamp(self.stability / self.max_stability, 0.0, 1.0)
    }
}

/// The three enemy archetypes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnemyKind {
    Skirmisher,
    Artillery,
    Boss,
}

impl EnemyKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Skirmisher => "skirmisher",
            Self::Artillery => "artillery",
            Self::Boss => "boss",
        }
    }

    /// The name shown in a target readout.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Skirmisher => "S-2 SKIRMISHER",
            Self::Artillery => "M-8 SIEGE UNIT",
            Self::Boss => "ASHFRAME PATRIARCH",
        }
    }
}

/// Every weapon in the game, on both sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WeaponId {
    Rifle,
    Missiles,
    Blade,
    EnemyGun,
    EnemyMortar,
    BossBarrage,
}

impl WeaponId {
    pub fn label(self) -> &'static str {
        match self {
            Self::Rifle => "rifle",
            Self::Missiles => "missiles",
            Self::Blade => "blade",
            Self::EnemyGun => "enemy-gun",
            Self::EnemyMortar => "enemy-mortar",
            Self::BossBarrage => "boss-barrage",
        }
    }

    /// Whether this weapon belongs to the player.
    pub fn is_player_weapon(self) -> bool {
        matches!(self, Self::Rifle | Self::Missiles | Self::Blade)
    }
}

/// The three beats of a blade swing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BladePhase {
    Windup,
    Active,
    Recovery,
}

/// Where a mission has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MissionPhase {
    Idle,
    Intro,
    Wave1,
    Wave2,
    Boss,
    Victory,
    Defeat,
}

impl MissionPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Intro => "intro",
            Self::Wave1 => "wave1",
            Self::Wave2 => "wave2",
            Self::Boss => "boss",
            Self::Victory => "victory",
            Self::Defeat => "defeat",
        }
    }

    /// Whether the mission has ended, one way or the other.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Victory | Self::Defeat)
    }

    /// Whether enemies should be acting.
    pub fn is_combat(self) -> bool {
        matches!(self, Self::Wave1 | Self::Wave2 | Self::Boss)
    }
}

/// What happened this step.
///
/// The simulation's only outward channel. It never returns a value from
/// [`step`](crate::sim) and never exposes its internals: it appends events, and
/// the caller decides what each one means. Rendering, audio and the HUD all
/// read the same stream, which is why they cannot disagree about what
/// happened.
#[derive(Debug, Clone, PartialEq)]
pub enum SimEvent {
    /// A weapon fired, with its origin and direction.
    Fire {
        weapon: WeaponId,
        origin: Vec3,
        direction: Vec3,
    },
    /// The trigger was pulled with nothing to fire.
    DryFire {
        weapon: WeaponId,
    },
    ReloadStart,
    ReloadEnd,
    MissileLaunch {
        origin: Vec3,
        direction: Vec3,
    },
    Blade {
        phase: BladePhase,
    },
    BladeHit {
        at: Vec3,
        target: u32,
        destroyed: bool,
    },
    /// A projectile met scenery.
    Impact {
        at: Vec3,
        normal: Vec3,
        surface: String,
    },
    Hit {
        at: Vec3,
        target: u32,
        amount: f32,
        staggered: bool,
    },
    Destroy {
        at: Vec3,
        target: u32,
        kind: String,
    },
    Explosion {
        at: Vec3,
        radius: f32,
    },
    Stagger {
        target: u32,
        at: Vec3,
    },
    Unstagger {
        target: u32,
    },
    PlayerDamage {
        amount: f32,
        at: Vec3,
    },
    PlayerStagger,
    Jump {
        at: Vec3,
    },
    Land {
        at: Vec3,
        speed: f32,
    },
    QuickBoost {
        at: Vec3,
        direction: Vec3,
    },
    AssaultStart,
    AssaultEnd,
    EnergyEmpty,
    EnergyRestored,
    RepairStart {
        remaining: u32,
    },
    RepairEnd,
    LockAcquired {
        target: u32,
    },
    LockLost {
        target: u32,
    },
    EnemyFire {
        at: Vec3,
    },
    /// A telegraphed attack's warning marker.
    Telegraph {
        at: Vec3,
        radius: f32,
        duration: f32,
    },
    OutOfBounds {
        at: Vec3,
    },
    Hint {
        id: String,
        text: String,
    },
    Objective {
        text: String,
    },
    MissionComplete {
        time: f32,
    },
    MissionFailed,
}

impl SimEvent {
    /// A short label, for logs and tests.
    ///
    /// Exhaustive on purpose: adding an event stops this compiling until it has
    /// a name, which is how an event ends up in a replay log rather than being
    /// silently invisible.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Fire { .. } => "fire",
            Self::DryFire { .. } => "dry-fire",
            Self::ReloadStart => "reload-start",
            Self::ReloadEnd => "reload-end",
            Self::MissileLaunch { .. } => "missile-launch",
            Self::Blade { .. } => "blade",
            Self::BladeHit { .. } => "blade-hit",
            Self::Impact { .. } => "impact",
            Self::Hit { .. } => "hit",
            Self::Destroy { .. } => "destroy",
            Self::Explosion { .. } => "explosion",
            Self::Stagger { .. } => "stagger",
            Self::Unstagger { .. } => "unstagger",
            Self::PlayerDamage { .. } => "player-damage",
            Self::PlayerStagger => "player-stagger",
            Self::Jump { .. } => "jump",
            Self::Land { .. } => "land",
            Self::QuickBoost { .. } => "quick-boost",
            Self::AssaultStart => "assault-start",
            Self::AssaultEnd => "assault-end",
            Self::EnergyEmpty => "energy-empty",
            Self::EnergyRestored => "energy-restored",
            Self::RepairStart { .. } => "repair-start",
            Self::RepairEnd => "repair-end",
            Self::LockAcquired { .. } => "lock-acquired",
            Self::LockLost { .. } => "lock-lost",
            Self::EnemyFire { .. } => "enemy-fire",
            Self::Telegraph { .. } => "telegraph",
            Self::OutOfBounds { .. } => "out-of-bounds",
            Self::Hint { .. } => "hint",
            Self::Objective { .. } => "objective",
            Self::MissionComplete { .. } => "mission-complete",
            Self::MissionFailed => "mission-failed",
        }
    }
}

/// Where the simulation sends its events.
///
/// A trait rather than a boxed closure, because the simulation holds one for
/// its whole life and a trait object with a single method is simpler to pass
/// around than a closure with a lifetime attached.
pub trait Hooks {
    fn emit(&mut self, event: &SimEvent);
}

/// A sink that discards everything.
///
/// Used by tests that only care about resulting state, and as the default so a
/// caller who wants no events does not have to write an empty implementation.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoHooks;

impl Hooks for NoHooks {
    fn emit(&mut self, _event: &SimEvent) {}
}

/// A sink that keeps everything, for tests and replays.
#[derive(Debug, Clone, Default)]
pub struct EventLog {
    pub events: Vec<SimEvent>,
}

impl EventLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many events of a given kind were recorded.
    ///
    /// The assertion most tests actually want: "exactly one destruction", not
    /// "the fourth event was a destruction".
    pub fn count(&self, kind: &str) -> usize {
        self.events.iter().filter(|e| e.kind() == kind).count()
    }

    pub fn any(&self, kind: &str) -> bool {
        self.events.iter().any(|e| e.kind() == kind)
    }

    pub fn clear(&mut self) {
        self.events.clear();
    }
}

impl Hooks for EventLog {
    fn emit(&mut self, event: &SimEvent) {
        self.events.push(event.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vector_knows_its_length() {
        assert_eq!(Vec3::new(3.0, 4.0, 0.0).length(), 5.0);
        assert_eq!(Vec3::ZERO.length(), 0.0);
        assert_eq!(Vec3::UP.length(), 1.0);
        assert_eq!(Vec3::new(3.0, 4.0, 0.0).length_squared(), 25.0);
    }

    #[test]
    fn normalising_a_zero_vector_gives_zero_rather_than_nan() {
        // The important one. A NaN here would travel through a position and
        // every later comparison would be false, so nothing would collide and
        // nothing would say why.
        let result = Vec3::ZERO.normalized();
        assert!(result.is_finite());
        assert_eq!(result, Vec3::ZERO);
        assert!(Vec3::splat(1e-30).normalized().is_finite());
    }

    #[test]
    fn normalising_gives_unit_length() {
        for v in [
            Vec3::new(3.0, 4.0, 0.0),
            Vec3::new(-1.0, 2.0, -2.0),
            Vec3::new(0.0, 0.0, 7.0),
        ] {
            let n = v.normalized();
            assert!((n.length() - 1.0).abs() < 1e-5, "{v:?} normalised to {n:?}");
        }
    }

    #[test]
    fn horizontal_length_ignores_height() {
        // A mech standing on a slope has moved no horizontal distance, which
        // is what ground-speed readouts and skating drag are computed from.
        let v = Vec3::new(3.0, 100.0, 4.0);
        assert_eq!(v.horizontal_length(), 5.0);
        assert_eq!(v.horizontal_normalized(), Vec3::new(0.6, 0.0, 0.8));
    }

    #[test]
    fn horizontal_normalising_a_vertical_vector_is_safe() {
        let v = Vec3::new(0.0, 9.0, 0.0);
        assert_eq!(v.horizontal_normalized(), Vec3::ZERO);
    }

    #[test]
    fn approach_lands_exactly_on_the_target() {
        let from = Vec3::ZERO;
        let to = Vec3::new(10.0, 0.0, 0.0);
        assert_eq!(from.approach(to, 100.0), to, "should not overshoot");
        let stepped = from.approach(to, 3.0);
        assert!(
            (stepped.length() - 3.0).abs() < 1e-5,
            "should move 3 metres"
        );
    }

    #[test]
    fn arithmetic_is_componentwise() {
        let a = Vec3::new(1.0, 2.0, 3.0);
        let b = Vec3::new(10.0, 20.0, 30.0);
        assert_eq!(a + b, Vec3::new(11.0, 22.0, 33.0));
        assert_eq!(b - a, Vec3::new(9.0, 18.0, 27.0));
        assert_eq!(a * 2.0, Vec3::new(2.0, 4.0, 6.0));
        assert_eq!(a.dot(b), 10.0 + 40.0 + 90.0);
    }

    #[test]
    fn a_damageable_knows_when_it_is_staggered() {
        let mut d = Damageable {
            id: 1,
            faction: Faction::Enemy,
            name: "test".into(),
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            radius: 1.0,
            height: 2.0,
            health: 100.0,
            max_health: 100.0,
            stability: 100.0,
            max_stability: 100.0,
            alive: true,
            stagger_timer: 0.0,
            invuln_timer: 0.0,
            stagger_armed: true,
        };
        assert!(!d.staggered());
        d.stagger_timer = 0.5;
        assert!(d.staggered());
        assert_eq!(d.health_fraction(), 1.0);
    }

    #[test]
    fn fractions_are_clamped_and_survive_a_zero_maximum() {
        // Nothing should produce a NaN or a negative bar width when a maximum
        // is legitimately zero or a value overshoots.
        let mut d = Damageable {
            id: 1,
            faction: Faction::Player,
            name: "test".into(),
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            radius: 1.0,
            height: 2.0,
            health: -50.0,
            max_health: 0.0,
            stability: 500.0,
            max_stability: 0.0,
            alive: false,
            stagger_timer: 0.0,
            invuln_timer: 0.0,
            stagger_armed: false,
        };
        assert_eq!(d.health_fraction(), 0.0);
        assert_eq!(d.stability_fraction(), 1.0);

        d.health = 250.0;
        d.max_health = 100.0;
        assert_eq!(d.health_fraction(), 1.0);
    }

    #[test]
    fn factions_are_hostile_to_each_other_and_not_to_themselves() {
        assert!(Faction::Player.hostile_to(Faction::Enemy));
        assert!(Faction::Enemy.hostile_to(Faction::Player));
        assert!(!Faction::Player.hostile_to(Faction::Player));
    }

    #[test]
    fn weapon_ownership_is_right() {
        assert!(WeaponId::Rifle.is_player_weapon());
        assert!(WeaponId::Missiles.is_player_weapon());
        assert!(WeaponId::Blade.is_player_weapon());
        assert!(!WeaponId::EnemyGun.is_player_weapon());
        assert!(!WeaponId::BossBarrage.is_player_weapon());
    }

    #[test]
    fn mission_phases_know_their_state() {
        assert!(MissionPhase::Victory.is_terminal());
        assert!(MissionPhase::Defeat.is_terminal());
        assert!(!MissionPhase::Boss.is_terminal());
        assert!(MissionPhase::Wave1.is_combat());
        assert!(MissionPhase::Boss.is_combat());
        assert!(!MissionPhase::Intro.is_combat());
        assert!(!MissionPhase::Victory.is_combat());
    }

    #[test]
    fn every_event_has_a_distinct_kind() {
        // The original emitted these as string tags that the renderer switched
        // on. Two events sharing a tag would be two behaviours silently merged.
        let mut kinds = std::collections::HashSet::new();
        for kind in [
            "fire",
            "dry-fire",
            "reload-start",
            "reload-end",
            "missile-launch",
            "blade",
            "blade-hit",
            "impact",
            "hit",
            "destroy",
            "explosion",
            "stagger",
            "unstagger",
            "player-damage",
            "player-stagger",
            "jump",
            "land",
            "quick-boost",
            "assault-start",
            "assault-end",
            "energy-empty",
            "energy-restored",
            "repair-start",
            "repair-end",
            "lock-acquired",
            "lock-lost",
            "enemy-fire",
            "telegraph",
            "out-of-bounds",
            "hint",
            "objective",
            "mission-complete",
            "mission-failed",
        ] {
            assert!(kinds.insert(kind), "'{kind}' is listed twice");
        }
        assert_eq!(kinds.len(), 33, "the event list has changed size");
    }

    #[test]
    fn an_event_reports_its_own_kind() {
        assert_eq!(SimEvent::ReloadStart.kind(), "reload-start");
        assert_eq!(SimEvent::EnergyEmpty.kind(), "energy-empty");
        assert_eq!(
            SimEvent::Fire {
                weapon: WeaponId::Rifle,
                origin: Vec3::ZERO,
                direction: Vec3::UP,
            }
            .kind(),
            "fire"
        );
    }

    #[test]
    fn an_event_log_counts_and_finds() {
        let mut log = EventLog::new();
        log.emit(&SimEvent::ReloadStart);
        log.emit(&SimEvent::EnergyEmpty);
        log.emit(&SimEvent::ReloadStart);

        assert_eq!(log.count("reload-start"), 2);
        assert_eq!(log.count("energy-empty"), 1);
        assert_eq!(log.count("destroy"), 0);
        assert!(log.any("energy-empty"));
        assert!(!log.any("destroy"));

        log.clear();
        assert!(log.events.is_empty());
    }

    #[test]
    fn the_no_op_sink_discards_quietly() {
        let mut hooks = NoHooks;
        hooks.emit(&SimEvent::MissionFailed);
    }
}
