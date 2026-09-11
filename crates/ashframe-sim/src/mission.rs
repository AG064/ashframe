//! Mission flow.
//!
//! Ported from the original's `game/mission.ts`: *"Operation ASHEN YARD:
//! deploy, break the skirmisher screen, survive the siege pair, then bring down
//! the Patriarch. The mission owns phase transitions and the real statistics the
//! results screen reports; it never invents numbers."*
//!
//! That last clause is the contract this module keeps. Every number on the
//! results screen is incremented from an event that actually happened — a hit
//! that landed, a repair that started, a shot that left the barrel — and none of
//! it is estimated from elapsed time or sampled from the world state at the end.
//! A mission that reported kills by counting corpses would report a different
//! number from one that counted destruction events the moment two units died in
//! the same step, and the second number is the true one.
//!
//! ## What the mission is allowed to touch
//!
//! The original passed a context object holding live world facts alongside four
//! closures. Rust keeps the facts in a plain struct and the four verbs in
//! [`MissionHooks`], so the mission can ask for a spawn without being able to
//! reach the roster, the collision world or the arena. Everything about *where*
//! a unit appears belongs to the sim; the mission only decides *what* and *when*.

use crate::config::mission as cfg;
use crate::types::{EnemyKind, MissionPhase};

/// A request to put one unit into the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnOrder {
    pub kind: EnemyKind,
    /// Index into the arena's spawn list for that kind.
    pub slot: usize,
}

/// The opening screen: three skirmishers, one per slot.
const WAVE1: [SpawnOrder; 3] = [
    SpawnOrder {
        kind: EnemyKind::Skirmisher,
        slot: 0,
    },
    SpawnOrder {
        kind: EnemyKind::Skirmisher,
        slot: 1,
    },
    SpawnOrder {
        kind: EnemyKind::Skirmisher,
        slot: 2,
    },
];

/// The siege wave: three more skirmishers covering a pair of artillery units.
const WAVE2: [SpawnOrder; 5] = [
    SpawnOrder {
        kind: EnemyKind::Skirmisher,
        slot: 3,
    },
    SpawnOrder {
        kind: EnemyKind::Skirmisher,
        slot: 4,
    },
    SpawnOrder {
        kind: EnemyKind::Artillery,
        slot: 0,
    },
    SpawnOrder {
        kind: EnemyKind::Skirmisher,
        slot: 5,
    },
    SpawnOrder {
        kind: EnemyKind::Artillery,
        slot: 1,
    },
];

/// The boss and its escort.
const BOSS: [SpawnOrder; 3] = [
    SpawnOrder {
        kind: EnemyKind::Boss,
        slot: 0,
    },
    SpawnOrder {
        kind: EnemyKind::Skirmisher,
        slot: 6,
    },
    SpawnOrder {
        kind: EnemyKind::Skirmisher,
        slot: 7,
    },
];

/// What the mission is allowed to ask the world to do.
pub trait MissionHooks {
    fn spawn(&mut self, order: SpawnOrder);
    fn set_objective(&mut self, text: &str);
    fn complete(&mut self);
    fn fail(&mut self);
}

/// Live world facts the mission reacts to. Read only, and deliberately narrow:
/// the mission has no business knowing where anything is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MissionFacts {
    pub alive_enemies: usize,
    pub boss_alive: bool,
    pub player_alive: bool,
}

/// The real statistics the results screen reports.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MissionStats {
    /// Seconds of combat elapsed.
    pub elapsed: f32,
    pub kills: u32,
    pub damage_taken: f32,
    pub repairs_used: u32,
    pub shots_fired: u32,
    pub score: u32,
}

/// Operation ASHEN YARD.
#[derive(Debug, Clone)]
pub struct Mission {
    pub phase: MissionPhase,
    /// Counts down through the intro and the two gaps, unused while fighting.
    pub timer: f32,
    pub stats: MissionStats,
    /// Hints already shown, in the order they were claimed.
    ///
    /// A vector rather than a set: there are a handful of them, the scan is
    /// free, and iteration order is the order they were shown, which is what a
    /// tutorial log wants to print.
    hints: Vec<String>,
    /// Whether the boss has been ordered in. Tracked here rather than in the
    /// sim, because the mission is what issues the order, so it already knows.
    boss_ordered: bool,
}

impl Default for Mission {
    fn default() -> Self {
        Self::new()
    }
}

impl Mission {
    pub fn new() -> Self {
        Self {
            phase: MissionPhase::Intro,
            timer: cfg::INTRO_TIME,
            stats: MissionStats::default(),
            hints: Vec::new(),
            boss_ordered: false,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Whether the mission has ended, one way or the other.
    pub fn finished(&self) -> bool {
        self.phase.is_terminal()
    }

    /// The objective line for the HUD.
    pub fn objective(&self) -> &'static str {
        match self.phase {
            MissionPhase::Intro => "DEPLOY — move out and sweep the yard",
            MissionPhase::Wave1 => "CONTACT — destroy the skirmisher screen",
            MissionPhase::Gap1 | MissionPhase::Gap2 => "REGROUP — more hostiles inbound",
            MissionPhase::Wave2 => "HOLD — siege units detected",
            MissionPhase::Boss => "PRIORITY — destroy the ASHFRAME PATRIARCH",
            MissionPhase::Victory => "MISSION COMPLETE",
            MissionPhase::Defeat => "FRAME LOST",
        }
    }

    /// Claim a named hint. True the first time only, so the UI shows it once.
    pub fn claim_hint(&mut self, id: &str) -> bool {
        if self.hints.iter().any(|shown| shown == id) {
            return false;
        }
        self.hints.push(id.to_string());
        true
    }

    /// Record a kill against the score table.
    pub fn register_kill(&mut self, kind: EnemyKind) {
        self.stats.kills += 1;
        self.stats.score += cfg::score_for(kind);
    }

    /// Advance the mission one fixed step.
    pub fn update(&mut self, dt: f32, facts: MissionFacts, hooks: &mut dyn MissionHooks) {
        if self.finished() {
            return;
        }
        self.stats.elapsed += dt;

        // Losing the frame ends the operation whatever else was happening, and
        // it is checked before the phase runs so a wave cannot be cleared by a
        // shot that lands in the same step the player dies in.
        if !facts.player_alive {
            self.phase = MissionPhase::Defeat;
            hooks.set_objective(self.objective());
            hooks.fail();
            return;
        }

        match self.phase {
            MissionPhase::Intro => {
                self.timer -= dt;
                if self.timer <= 0.0 {
                    self.enter(MissionPhase::Wave1, hooks);
                    self.deploy(&WAVE1, hooks);
                }
            }
            MissionPhase::Wave1 if facts.alive_enemies == 0 => {
                self.enter(MissionPhase::Gap1, hooks);
                self.timer = cfg::WAVE_GAP;
            }
            MissionPhase::Gap1 => {
                self.timer -= dt;
                if self.timer <= 0.0 {
                    self.enter(MissionPhase::Wave2, hooks);
                    self.deploy(&WAVE2, hooks);
                }
            }
            // The boss commits once the siege pair is broken or its escorts are
            // dead — which is to say, once the yard is clear again.
            MissionPhase::Wave2 if facts.alive_enemies == 0 => {
                self.enter(MissionPhase::Gap2, hooks);
                self.timer = cfg::WAVE_GAP;
            }
            MissionPhase::Gap2 => {
                self.timer -= dt;
                if self.timer <= 0.0 {
                    self.enter(MissionPhase::Boss, hooks);
                    self.boss_ordered = true;
                    self.deploy(&BOSS, hooks);
                }
            }
            MissionPhase::Boss if self.boss_ordered && !facts.boss_alive => {
                self.enter(MissionPhase::Victory, hooks);
                hooks.complete();
            }
            _ => {}
        }
    }

    fn enter(&mut self, phase: MissionPhase, hooks: &mut dyn MissionHooks) {
        self.phase = phase;
        hooks.set_objective(self.objective());
    }

    fn deploy(&mut self, orders: &[SpawnOrder], hooks: &mut dyn MissionHooks) {
        for order in orders {
            hooks.spawn(*order);
        }
    }

    /// Whether the boss has been sent in. The sim reads this to know that an
    /// absent boss means a dead boss rather than one that has not arrived.
    pub fn boss_ordered(&self) -> bool {
        self.boss_ordered
    }

    /// Hints shown so far, in order.
    pub fn hints(&self) -> &[String] {
        &self.hints
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::EnemyKind;

    #[derive(Debug, Default)]
    struct Recorder {
        spawned: Vec<SpawnOrder>,
        objectives: Vec<String>,
        completed: u32,
        failed: u32,
    }

    impl MissionHooks for Recorder {
        fn spawn(&mut self, order: SpawnOrder) {
            self.spawned.push(order);
        }
        fn set_objective(&mut self, text: &str) {
            self.objectives.push(text.to_string());
        }
        fn complete(&mut self) {
            self.completed += 1;
        }
        fn fail(&mut self) {
            self.failed += 1;
        }
    }

    const ALIVE: MissionFacts = MissionFacts {
        alive_enemies: 1,
        boss_alive: false,
        player_alive: true,
    };

    /// Nothing standing between the player and the next phase.
    const CLEAR: MissionFacts = MissionFacts {
        alive_enemies: 0,
        boss_alive: false,
        player_alive: true,
    };

    /// Step until the phase changes or the budget runs out.
    fn run_to(m: &mut Mission, rec: &mut Recorder, facts: MissionFacts, phase: MissionPhase) {
        for _ in 0..2000 {
            if m.phase == phase {
                return;
            }
            m.update(1.0 / 60.0, facts, rec);
        }
        panic!("never reached {:?}", phase);
    }

    fn at(m: &mut Mission, rec: &mut Recorder, facts: MissionFacts, n: u32) {
        for _ in 0..n {
            m.update(1.0 / 60.0, facts, rec);
        }
    }

    #[test]
    fn the_mission_opens_on_a_deploy_countdown() {
        let m = Mission::new();
        assert_eq!(m.phase, MissionPhase::Intro);
        assert_eq!(m.timer, cfg::INTRO_TIME);
        assert!(m.objective().starts_with("DEPLOY"));
    }

    #[test]
    fn the_intro_ends_with_the_skirmisher_screen_arriving() {
        let mut m = Mission::new();
        let mut rec = Recorder::default();
        run_to(&mut m, &mut rec, ALIVE, MissionPhase::Wave1);

        assert_eq!(rec.spawned.len(), 3);
        assert!(rec.spawned.iter().all(|o| o.kind == EnemyKind::Skirmisher));
        assert_eq!(
            rec.spawned.iter().map(|o| o.slot).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(
            rec.objectives.last().unwrap(),
            "CONTACT — destroy the skirmisher screen"
        );
    }

    #[test]
    fn a_cleared_wave_starts_a_gap_rather_than_the_next_wave() {
        let mut m = Mission::new();
        let mut rec = Recorder::default();
        run_to(&mut m, &mut rec, ALIVE, MissionPhase::Wave1);

        let clear = MissionFacts {
            alive_enemies: 0,
            ..ALIVE
        };
        m.update(1.0 / 60.0, clear, &mut rec);
        assert_eq!(m.phase, MissionPhase::Gap1);
        assert!((m.timer - cfg::WAVE_GAP).abs() < 1e-6);
        assert!(m.objective().starts_with("REGROUP"));
        // The gap is a pause, not a spawn trigger: nothing arrives on the step
        // the yard is cleared.
        assert_eq!(rec.spawned.len(), 3);
    }

    #[test]
    fn the_gap_runs_down_before_the_siege_wave_deploys() {
        let mut m = Mission::new();
        let mut rec = Recorder::default();
        run_to(&mut m, &mut rec, ALIVE, MissionPhase::Wave1);
        let clear = MissionFacts {
            alive_enemies: 0,
            ..ALIVE
        };
        m.update(1.0 / 60.0, clear, &mut rec);

        // One step short of the gap elapsing, nothing has deployed.
        let steps = (cfg::WAVE_GAP * 60.0).floor() as u32 - 1;
        at(&mut m, &mut rec, clear, steps);
        assert_eq!(m.phase, MissionPhase::Gap1);
        assert_eq!(rec.spawned.len(), 3);

        // The next step is the one the gap elapses on. It is caught on the way
        // past rather than checked after a batch, because the step after a
        // deploy sees an empty yard and moves the mission straight on again.
        let mut deployed = false;
        for _ in 0..4 {
            m.update(1.0 / 60.0, clear, &mut rec);
            if m.phase == MissionPhase::Wave2 {
                deployed = true;
                break;
            }
        }
        assert!(deployed, "the gap elapsed without deploying the siege wave");
        assert_eq!(rec.spawned.len(), 8);
        assert_eq!(
            rec.spawned[3..]
                .iter()
                .filter(|o| o.kind == EnemyKind::Artillery)
                .count(),
            2,
            "the siege wave is the one with artillery in it"
        );
    }

    #[test]
    fn the_boss_arrives_only_after_the_siege_wave_is_broken() {
        let mut m = Mission::new();
        let mut rec = Recorder::default();
        run_to(&mut m, &mut rec, ALIVE, MissionPhase::Wave1);
        let clear = MissionFacts {
            alive_enemies: 0,
            ..ALIVE
        };
        m.update(1.0 / 60.0, clear, &mut rec);
        run_to(&mut m, &mut rec, clear, MissionPhase::Wave2);
        m.update(1.0 / 60.0, clear, &mut rec);
        assert_eq!(m.phase, MissionPhase::Gap2);

        run_to(&mut m, &mut rec, clear, MissionPhase::Boss);
        assert!(m.boss_ordered());
        let boss = rec.spawned.last().unwrap();
        assert_eq!(rec.spawned[rec.spawned.len() - 3].kind, EnemyKind::Boss);
        assert_eq!(
            boss.kind,
            EnemyKind::Skirmisher,
            "the escorts come in after"
        );
        assert_eq!(rec.spawned.len(), 11);
    }

    #[test]
    fn killing_the_boss_completes_the_mission() {
        let mut m = Mission::new();
        let mut rec = Recorder::default();
        run_to(&mut m, &mut rec, CLEAR, MissionPhase::Boss);

        let boss_up = MissionFacts {
            alive_enemies: 3,
            boss_alive: true,
            player_alive: true,
        };
        at(&mut m, &mut rec, boss_up, 10);
        assert_eq!(m.phase, MissionPhase::Boss, "the boss is still standing");
        assert_eq!(rec.completed, 0);

        let boss_down = MissionFacts {
            alive_enemies: 2,
            boss_alive: false,
            player_alive: true,
        };
        m.update(1.0 / 60.0, boss_down, &mut rec);
        assert_eq!(m.phase, MissionPhase::Victory);
        assert_eq!(rec.completed, 1);
        assert!(m.finished());
        assert_eq!(m.objective(), "MISSION COMPLETE");
    }

    #[test]
    fn the_escorts_do_not_have_to_die_for_the_mission_to_end() {
        // The boss is the objective. Requiring the escorts as well would let a
        // player who killed the Patriarch watch the operation stall.
        let mut m = Mission::new();
        let mut rec = Recorder::default();
        run_to(&mut m, &mut rec, CLEAR, MissionPhase::Boss);
        let facts = MissionFacts {
            alive_enemies: 2,
            boss_alive: false,
            player_alive: true,
        };
        m.update(1.0 / 60.0, facts, &mut rec);
        assert_eq!(m.phase, MissionPhase::Victory);
    }

    #[test]
    fn losing_the_frame_fails_the_mission_from_any_phase() {
        for phase in [
            MissionPhase::Intro,
            MissionPhase::Wave1,
            MissionPhase::Gap1,
            MissionPhase::Wave2,
            MissionPhase::Gap2,
            MissionPhase::Boss,
        ] {
            let mut m = Mission::new();
            m.phase = phase;
            let mut rec = Recorder::default();
            m.update(
                1.0 / 60.0,
                MissionFacts {
                    alive_enemies: 0,
                    boss_alive: false,
                    player_alive: false,
                },
                &mut rec,
            );
            assert_eq!(m.phase, MissionPhase::Defeat, "from {:?}", phase);
            assert_eq!(rec.failed, 1);
            assert_eq!(rec.completed, 0);
        }
    }

    #[test]
    fn death_beats_a_cleared_yard_in_the_same_step() {
        // Both conditions can be true at once. The player losing is the one
        // that ends the operation; a victory awarded to a dead pilot is worse
        // than a defeat awarded a step early.
        let mut m = Mission::new();
        m.phase = MissionPhase::Wave2;
        let mut rec = Recorder::default();
        m.update(
            1.0 / 60.0,
            MissionFacts {
                alive_enemies: 0,
                boss_alive: false,
                player_alive: false,
            },
            &mut rec,
        );
        assert_eq!(m.phase, MissionPhase::Defeat);
    }

    #[test]
    fn a_finished_mission_stops_counting() {
        let mut m = Mission::new();
        let mut rec = Recorder::default();
        m.phase = MissionPhase::Victory;
        let before = m.stats.elapsed;
        at(&mut m, &mut rec, ALIVE, 60);
        assert_eq!(
            m.stats.elapsed, before,
            "elapsed kept running after the end"
        );
        assert!(rec.objectives.is_empty(), "a finished mission kept talking");
    }

    #[test]
    fn kills_are_scored_by_archetype() {
        let mut m = Mission::new();
        m.register_kill(EnemyKind::Skirmisher);
        m.register_kill(EnemyKind::Artillery);
        m.register_kill(EnemyKind::Boss);
        assert_eq!(m.stats.kills, 3);
        assert_eq!(m.stats.score, 100 + 175 + 2500);
    }

    #[test]
    fn every_phase_says_something_different() {
        // The HUD prints whatever the mission says; two phases sharing a line
        // would leave the player unable to tell them apart.
        let phases = [
            MissionPhase::Intro,
            MissionPhase::Wave1,
            MissionPhase::Gap1,
            MissionPhase::Wave2,
            MissionPhase::Gap2,
            MissionPhase::Boss,
            MissionPhase::Victory,
            MissionPhase::Defeat,
        ];
        for (i, a) in phases.iter().enumerate() {
            for b in &phases[i + 1..] {
                let (mut ma, mut mb) = (Mission::new(), Mission::new());
                ma.phase = *a;
                mb.phase = *b;
                // The two gaps deliberately share their line, and say so.
                let both_gaps = matches!(a, MissionPhase::Gap1 | MissionPhase::Gap2)
                    && matches!(b, MissionPhase::Gap1 | MissionPhase::Gap2);
                if both_gaps {
                    assert_eq!(ma.objective(), mb.objective());
                } else {
                    assert_ne!(ma.objective(), mb.objective(), "{:?} and {:?}", a, b);
                }
            }
        }
    }

    #[test]
    fn a_hint_is_claimed_once() {
        let mut m = Mission::new();
        assert!(m.claim_hint("move"));
        assert!(!m.claim_hint("move"));
        assert!(m.claim_hint("boost"));
        assert_eq!(m.hints(), ["move".to_string(), "boost".to_string()]);
    }

    #[test]
    fn a_reset_puts_everything_back() {
        let mut m = Mission::new();
        let mut rec = Recorder::default();
        m.register_kill(EnemyKind::Boss);
        m.claim_hint("move");
        run_to(&mut m, &mut rec, CLEAR, MissionPhase::Boss);
        m.reset();

        assert_eq!(m.phase, MissionPhase::Intro);
        assert_eq!(m.timer, cfg::INTRO_TIME);
        assert_eq!(m.stats.kills, 0);
        assert_eq!(m.stats.score, 0);
        assert!(m.hints().is_empty());
        assert!(!m.boss_ordered());
    }
}
