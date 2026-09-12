//! Sound.
//!
//! Every sound is rendered at load from the recipes in [`crate::synth`], so the
//! project ships no audio files. That is the original's approach kept intact —
//! it had no assets either — and it means a sound can be changed by changing a
//! number rather than by finding a replacement recording and licensing it.
//!
//! Two kinds of voice. One-shots are drawn from a fixed pool: a firefight can
//! ask for more simultaneous sounds than a mixer can render, and the pool
//! answers by dropping the request rather than by growing, so the frame time
//! does not degrade exactly when the fight gets busy. The engine and the
//! thrusters are sustained loops whose pitch and volume follow actual movement,
//! which is most of what makes a mech feel like it has mass.
//!
//! This file is plumbing: it decides which rendered buffer to play, where, and
//! how loudly, and it owns nothing about how a sound is made. Everything that
//! shapes a sound lives in `synth.rs`, where it can be measured.

use godot::classes::audio_stream_wav::{Format, LoopMode};
use godot::classes::{AudioStreamPlayer, AudioStreamPlayer3D, AudioStreamWav, Node3D};
use godot::prelude::*;

use ashframe_sim::config::sim::DT;
use ashframe_sim::types::Vec3;

use crate::palette::to_godot;
use crate::synth::{self, Buffer, SAMPLE_RATE};

/// Simultaneous positional one-shots.
///
/// The original capped its voices at twenty-eight for the same reason.
const VOICE_POOL: usize = 28;
/// Simultaneous non-positional sounds: the HUD and the results screen.
const UI_POOL: usize = 6;
/// The player's own footsteps.
///
/// A sound at the listener's own position gains nothing from being positional,
/// so the feet have a small non-positional pool of their own. Their own pool
/// rather than the HUD's, because borrowing a HUD voice four times a second
/// would cut off the lock tone to play a footstep.
const FOOT_POOL: usize = 3;

/// The sustained loops, in the order they are built.
const ENGINE_BED: usize = 0;
const ENGINE_LOAD: usize = 1;
const THRUSTER_ROAR: usize = 2;
const THRUSTER_HISS: usize = 3;
const LOOPS: usize = 4;

/// How far the mech's walk cycle advances per metre travelled.
///
/// The rig's own gait uses this number, and the audio follows it rather than
/// keeping a clock of its own: a footstep that drifts out of step with the foot
/// on screen is worse than no footstep at all.
const STRIDE: f32 = 0.22;

/// Which pool a one-shot is drawn from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pool {
    /// A sound in the world, positional and distance-attenuated.
    Field,
    /// A sound in the player's head: the HUD.
    Hud,
    /// The player's own feet.
    Feet,
}

/// Every sound the game can make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    Autocannon,
    EnemyFire,
    MissileLaunch,
    BladeSwing,
    BladeHit,
    ImpactHard,
    ImpactSoft,
    Explosion,
    PlayerDamage,
    PlayerStagger,
    EnergyWarning,
    Boost,
    Landing,
    Footstep,
    LockAcquired,
    LockLost,
    Telegraph,
    Repair,
    UiClick,
    UiConfirm,
    MissionComplete,
    MissionFailed,
}

impl Sound {
    /// Every variant, in declaration order.
    ///
    /// Written out because a table indexed by a discriminant needs a list of
    /// the discriminants, and a list that is out of step with the enum is a
    /// sound that silently plays another sound's buffer.
    /// `every_sound_is_rendered` fails if this list and the enum disagree.
    pub const ALL: [Sound; 22] = [
        Sound::Autocannon,
        Sound::EnemyFire,
        Sound::MissileLaunch,
        Sound::BladeSwing,
        Sound::BladeHit,
        Sound::ImpactHard,
        Sound::ImpactSoft,
        Sound::Explosion,
        Sound::PlayerDamage,
        Sound::PlayerStagger,
        Sound::EnergyWarning,
        Sound::Boost,
        Sound::Landing,
        Sound::Footstep,
        Sound::LockAcquired,
        Sound::LockLost,
        Sound::Telegraph,
        Sound::Repair,
        Sound::UiClick,
        Sound::UiConfirm,
        Sound::MissionComplete,
        Sound::MissionFailed,
    ];

    pub const COUNT: usize = Self::ALL.len();

    fn index(self) -> usize {
        self as usize
    }
}

/// The rendered takes of one sound.
///
/// Some sounds have more than one, and those are exactly the sounds that
/// repeat. A burst of autocannon fire that plays the identical buffer six times
/// a second is heard as a loop, and a footstep that is always the same foot is
/// heard as a hop; alternating two renders costs a few kilobytes of memory and
/// removes both.
#[derive(Default)]
struct Takes {
    streams: Vec<Gd<AudioStreamWav>>,
}

impl Takes {
    fn get(&self, turn: usize) -> Option<&Gd<AudioStreamWav>> {
        take_index(self.streams.len(), turn).map(|index| &self.streams[index])
    }
}

/// Which take a turn lands on.
///
/// Alternating rather than random: two takes played in order are heard as a
/// mechanism working, and the same two played at random are heard as an
/// inconsistency.
fn take_index(takes: usize, turn: usize) -> Option<usize> {
    if takes == 0 {
        None
    } else {
        Some(turn % takes)
    }
}

/// The rendered set.
struct Streams {
    takes: Vec<Takes>,
    engine_bed: Gd<AudioStreamWav>,
    engine_load: Gd<AudioStreamWav>,
    thruster_roar: Gd<AudioStreamWav>,
    thruster_hiss: Gd<AudioStreamWav>,
}

/// The audio system.
pub struct Audio {
    voices: Vec<Gd<AudioStreamPlayer3D>>,
    ui: Vec<Gd<AudioStreamPlayer>>,
    feet: Vec<Gd<AudioStreamPlayer>>,
    loops: [Gd<AudioStreamPlayer>; LOOPS],
    /// The levels and pitches the loops are at, smoothed towards the values the
    /// movement calls for.
    mix: [f32; LOOPS],
    pitch: [f32; LOOPS],
    streams: Streams,
    next_voice: usize,
    next_ui: usize,
    next_foot: usize,
    /// How far through the walk cycle the mech is, in radians.
    gait: f32,
    /// Distance past which a one-shot is not worth starting.
    ///
    /// A round landing two hundred metres away is inaudible under the engine
    /// anyway, and starting it would take a voice from something the player can
    /// actually hear.
    pub audible_range: f32,
    /// Rate limiting for sounds that would otherwise machine-gun.
    last_damage: f32,
    last_warning: f32,
    clock: f32,
}

impl Audio {
    /// Build the players and render every sound.
    pub fn build(parent: &mut Gd<Node3D>) -> Self {
        let mut voices = Vec::with_capacity(VOICE_POOL);
        for _ in 0..VOICE_POOL {
            let mut player = AudioStreamPlayer3D::new_alloc();
            player.set_unit_size(12.0);
            player.set_max_distance(260.0);
            // Inverse-square: a shot further away is quieter, which is the only
            // cue the player has for how far off a fight is.
            player.set_attenuation_model(
                godot::classes::audio_stream_player_3d::AttenuationModel::INVERSE_SQUARE_DISTANCE,
            );
            // And darker with distance. Air absorbs the top of a sound long
            // before it absorbs the bottom, so a distant explosion is not just a
            // quieter one — it is a duller one. Without this every sound in the
            // game is the same sound at a different volume.
            player.set_attenuation_filter_cutoff_hz(3_200.0);
            player.set_attenuation_filter_db(-28.0);
            parent.add_child(&player);
            voices.push(player);
        }

        let mut ui = Vec::with_capacity(UI_POOL);
        for _ in 0..UI_POOL {
            let mut player = AudioStreamPlayer::new_alloc();
            player.set_bus(&StringName::from("Master"));
            parent.add_child(&player);
            ui.push(player);
        }

        let mut feet = Vec::with_capacity(FOOT_POOL);
        for _ in 0..FOOT_POOL {
            let mut player = AudioStreamPlayer::new_alloc();
            player.set_bus(&StringName::from("Master"));
            parent.add_child(&player);
            feet.push(player);
        }

        let streams = render_all();
        let loops = [
            // The bed is alone under the briefing camera, so it starts audible;
            // the load layers start silent and are brought in by `drive`.
            loop_player(parent, &streams.engine_bed, -12.0),
            loop_player(parent, &streams.engine_load, -60.0),
            loop_player(parent, &streams.thruster_roar, -60.0),
            loop_player(parent, &streams.thruster_hiss, -60.0),
        ];

        Self {
            voices,
            ui,
            feet,
            loops,
            mix: [0.32, 0.0, 0.0, 0.0],
            pitch: [0.86, 0.9, 0.82, 0.9],
            streams,
            next_voice: 0,
            next_ui: 0,
            next_foot: 0,
            gait: 0.0,
            audible_range: 220.0,
            last_damage: -10.0,
            last_warning: -10.0,
            clock: 0.0,
        }
    }

    /// A one-shot at a world position.
    pub fn play_at(&mut self, sound: Sound, at: Vec3, listener: Vec3, pitch: f32) {
        if (at - listener).length() > self.audible_range {
            return;
        }
        let turn = self.next_voice;
        self.next_voice = self.next_voice.wrapping_add(1);
        self.start(sound, at, pitch, turn, Pool::Field);
    }

    /// A one-shot with no position: the HUD and the results screen.
    pub fn play_ui(&mut self, sound: Sound) {
        let turn = self.next_ui;
        self.next_ui = self.next_ui.wrapping_add(1);
        self.start(sound, Vec3::ZERO, 1.0, turn, Pool::Hud);
    }

    /// Start one voice from the pool for a sound.
    fn start(&mut self, sound: Sound, at: Vec3, pitch: f32, turn: usize, pool: Pool) {
        let Some(stream) = self
            .streams
            .takes
            .get(sound.index())
            .and_then(|takes| takes.get(turn))
            .cloned()
        else {
            return;
        };
        // A HUD blip that is a few per cent off every time is a broken HUD.
        let pitch = if pool == Pool::Hud {
            pitch
        } else {
            pitch * detune(turn)
        };

        match pool {
            Pool::Field => {
                if self.voices.is_empty() {
                    return;
                }
                let index = self.next_voice % self.voices.len();
                let player = &mut self.voices[index];
                player.set_stream(&stream);
                player.set_position(to_godot(at));
                player.set_pitch_scale(pitch);
                player.play();
            }
            Pool::Hud | Pool::Feet => {
                let pool = if pool == Pool::Hud {
                    &mut self.ui
                } else {
                    &mut self.feet
                };
                if pool.is_empty() {
                    return;
                }
                // The pools are small and round-robin, so a burst steals the
                // oldest voice rather than being dropped: at six voices of HUD
                // the oldest is always finished anyway.
                let index = turn % pool.len();
                let player = &mut pool[index];
                player.set_stream(&stream);
                player.set_pitch_scale(pitch);
                player.play();
            }
        }
    }

    /// Follow the mech with the sustained voices.
    ///
    /// `speed` in metres per second, `thrust` 0..1, `grounded` whether the feet
    /// are on something.
    pub fn drive(&mut self, speed: f32, thrust: f32, boosting: bool, grounded: bool) {
        let norm = (speed / 48.0).clamp(0.0, 1.0);
        let load = (norm + if boosting { 0.3 } else { 0.0 }).clamp(0.0, 1.0);

        // Pitch rather than volume does most of the work: a mech under load
        // sounds like it is working harder, and simply making it louder reads
        // as a mixing mistake. The second pair of layers is the other half of
        // it — what arrives with speed is not the same sound turned up, it is
        // the whine of a machine being asked for more.
        self.aim(ENGINE_BED, 0.86 + load * 0.20, 0.32 + load * 0.30);
        self.aim(
            ENGINE_LOAD,
            0.90 + load * 0.40,
            (load * 1.15).powf(1.6) * 0.45,
        );
        self.aim(THRUSTER_ROAR, 0.82 + thrust * 0.50, thrust * 0.50);
        self.aim(THRUSTER_HISS, 0.90 + thrust * 0.70, thrust.powf(2.2) * 0.32);

        self.walk(speed, grounded);
    }

    /// Aim a sustained voice at a pitch and a level, and move it part of the
    /// way there.
    ///
    /// This is called from the fixed simulation step, a hundred and twenty
    /// times a second, and a mixer value that jumps a hundred and twenty times
    /// a second is a zipper. One small step towards the target per call is a
    /// filter with a time constant, and it costs two multiplies.
    fn aim(&mut self, voice: usize, pitch: f32, level: f32) {
        const SMOOTHING: f32 = 0.12;
        self.pitch[voice] += (pitch - self.pitch[voice]) * SMOOTHING;
        self.mix[voice] += (level - self.mix[voice]) * SMOOTHING;
        let player = &mut self.loops[voice];
        player.set_pitch_scale(self.pitch[voice]);
        player.set_volume_db(linear_to_db(self.mix[voice]));
    }

    /// Footsteps.
    ///
    /// The walk cycle advances with distance travelled rather than with time,
    /// which is the same rule the legs on screen follow: a machine standing
    /// still does not march on the spot, and a fast one does not glide. It also
    /// means a footstep can only be heard from a mech that is actually walking.
    fn walk(&mut self, speed: f32, grounded: bool) {
        if !grounded || speed < 1.0 {
            // The cycle holds where it is rather than resetting, so a mech that
            // stops mid-stride puts the same foot down when it moves again.
            return;
        }
        self.gait += speed * STRIDE * DT;
        while self.gait >= std::f32::consts::PI {
            self.gait -= std::f32::consts::PI;
            let turn = self.next_foot;
            self.next_foot = (self.next_foot + 1) % 2;
            self.start(Sound::Footstep, Vec3::ZERO, 1.0, turn, Pool::Feet);
        }
    }

    /// Silence the sustained voices, for the briefing and the results screen.
    pub fn hush(&mut self) {
        for (voice, player) in self.loops.iter_mut().enumerate() {
            player.set_volume_db(-60.0);
            // The smoothed levels go with them, so the engine comes back up over
            // a tenth of a second rather than arriving at the level it happened
            // to be at when the mission ended.
            self.mix[voice] = 0.0;
        }
    }

    /// Advance the rate limiters.
    pub fn step(&mut self, dt: f32) {
        self.clock += dt;
    }

    /// A damage sound, at most one every 140 ms.
    ///
    /// A burst that lands four rounds in one step would otherwise start four
    /// identical sounds on the same sample, which is a click rather than a hit.
    pub fn damage(&mut self, at: Vec3, listener: Vec3) {
        if self.clock - self.last_damage < 0.14 {
            return;
        }
        self.last_damage = self.clock;
        self.play_at(Sound::PlayerDamage, at, listener, 1.0);
    }

    /// An energy warning, at most one every 900 ms.
    pub fn energy_warning(&mut self) {
        if self.clock - self.last_warning < 0.9 {
            return;
        }
        self.last_warning = self.clock;
        self.play_ui(Sound::EnergyWarning);
    }
}

/// A deterministic few per cent of pitch, per shot.
///
/// Two takes remove most of the repetition in a burst of fire; a couple of per
/// cent of detune removes the rest, because the ear stops hearing a repeated
/// sample and starts hearing a gun. It is a hash of the shot counter rather
/// than a random number so that the same burst is the same burst every run.
fn detune(turn: usize) -> f32 {
    let hash = turn.wrapping_mul(2_654_435_761) >> 24;
    1.0 + (hash % 7) as f32 * 0.008 - 0.024
}

/// Decibels for a linear 0..1 gain.
///
/// Guards zero, because the logarithm of zero is not a quiet sound.
fn linear_to_db(gain: f32) -> f32 {
    if gain <= 0.0001 {
        return -60.0;
    }
    20.0 * gain.log10()
}

/// A sustained voice: it is started once and then only ever re-aimed.
fn loop_player(
    parent: &mut Gd<Node3D>,
    stream: &Gd<AudioStreamWav>,
    volume_db: f32,
) -> Gd<AudioStreamPlayer> {
    let mut player = AudioStreamPlayer::new_alloc();
    player.set_bus(&StringName::from("Master"));
    player.set_stream(stream);
    player.set_volume_db(volume_db);
    parent.add_child(&player);
    // Started now and left running: position is kept by the stream itself, so
    // stopping and restarting the engine on every speed change would be a
    // stutter rather than an engine.
    player.play();
    player
}

/// Every sound, rendered but not yet wrapped: one entry per [`Sound`], each
/// with one or more takes.
fn recipes() -> Vec<(Sound, Vec<Buffer>)> {
    vec![
        // Two takes of each weapon, because these are the sounds a player hears
        // thirty times in a row.
        (
            Sound::Autocannon,
            vec![synth::autocannon(0x1101), synth::autocannon(0x1102)],
        ),
        (
            Sound::EnemyFire,
            vec![synth::machine_gun(0x2201), synth::machine_gun(0x2202)],
        ),
        (Sound::MissileLaunch, vec![synth::missile_launch(0x3301)]),
        (Sound::BladeSwing, vec![synth::blade_swing(0x4401)]),
        (
            Sound::BladeHit,
            vec![synth::blade_hit(0x5501), synth::blade_hit(0x5502)],
        ),
        // Impacts are the most repeated sound in the game and the one with the
        // most reason to vary: two sizes of hard thing, two soft ones.
        (
            Sound::ImpactHard,
            vec![
                synth::impact_hard(0x6601, 0.8),
                synth::impact_hard(0x6602, 1.5),
            ],
        ),
        (
            Sound::ImpactSoft,
            vec![synth::impact_soft(0x7701), synth::impact_soft(0x7702)],
        ),
        (Sound::Explosion, vec![synth::explosion(0x8801)]),
        (Sound::PlayerDamage, vec![synth::player_damage(0x9901)]),
        (Sound::PlayerStagger, vec![synth::player_stagger(0xaa01)]),
        (Sound::EnergyWarning, vec![synth::energy_warning()]),
        (Sound::Boost, vec![synth::boost(0xbb01)]),
        (Sound::Landing, vec![synth::landing(0xcc01)]),
        // The two feet, which the walk cycle alternates through.
        (
            Sound::Footstep,
            vec![synth::footstep(0), synth::footstep(1)],
        ),
        (Sound::LockAcquired, vec![synth::lock_acquired()]),
        (Sound::LockLost, vec![synth::lock_lost()]),
        (Sound::Telegraph, vec![synth::telegraph()]),
        (Sound::Repair, vec![synth::repair(0xdd01)]),
        (Sound::UiClick, vec![synth::ui_click()]),
        (Sound::UiConfirm, vec![synth::ui_confirm()]),
        (Sound::MissionComplete, vec![synth::mission_complete()]),
        (Sound::MissionFailed, vec![synth::mission_failed()]),
    ]
}

/// Render every sound once.
fn render_all() -> Streams {
    let mut takes: Vec<Takes> = (0..Sound::COUNT).map(|_| Takes::default()).collect();
    for (sound, buffers) in recipes() {
        takes[sound.index()] = Takes {
            streams: buffers.iter().map(|buffer| to_wav(buffer, false)).collect(),
        };
    }
    Streams {
        takes,
        engine_bed: to_wav(&synth::engine_bed(), true),
        engine_load: to_wav(&synth::engine_load(), true),
        thruster_roar: to_wav(&synth::thruster_roar(), true),
        thruster_hiss: to_wav(&synth::thruster_hiss(), true),
    }
}

fn to_wav(buffer: &Buffer, looping: bool) -> Gd<AudioStreamWav> {
    let mut stream = AudioStreamWav::new_gd();
    stream.set_format(Format::FORMAT_16_BITS);
    stream.set_mix_rate(SAMPLE_RATE as i32);
    stream.set_stereo(false);
    stream.set_data(&PackedByteArray::from(buffer.to_pcm16().as_slice()));
    if looping {
        // Without this the engine plays for one second at the start of the
        // mission and then stops for the rest of it — a silent loop is a bug
        // that sounds exactly like a mixer problem.
        stream.set_loop_mode(LoopMode::FORWARD);
        stream.set_loop_begin(0);
        stream.set_loop_end(buffer.len() as i32);
    }
    stream
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::ears;

    #[test]
    fn silence_is_quiet_in_decibels() {
        // The logarithm of zero is not a quiet sound, it is negative infinity,
        // and a mixer fed that produces a NaN and stops.
        assert_eq!(linear_to_db(0.0), -60.0);
        assert!(linear_to_db(1.0).abs() < 1e-4);
        assert!(linear_to_db(0.5) < -5.9 && linear_to_db(0.5) > -6.1);
    }

    #[test]
    fn every_sound_is_rendered() {
        // The table is indexed by discriminant, so a variant that is missing
        // from `ALL` would silently play another sound's buffer. This is the
        // test that makes the two lists impossible to disagree.
        for (index, sound) in Sound::ALL.iter().enumerate() {
            assert_eq!(sound.index(), index, "{sound:?} is out of order in ALL");
        }
        let rendered: Vec<Sound> = recipes().into_iter().map(|(sound, _)| sound).collect();
        for sound in Sound::ALL {
            assert!(
                rendered.contains(&sound),
                "{sound:?} is in the enum but not in the set"
            );
        }
        assert_eq!(rendered.len(), Sound::COUNT, "a sound is rendered twice");
    }

    #[test]
    fn every_take_is_a_sound() {
        for (sound, buffers) in recipes() {
            assert!(!buffers.is_empty(), "{sound:?} has no takes");
            for buffer in &buffers {
                assert!(!buffer.samples.is_empty(), "{sound:?} has an empty take");
                assert!(
                    buffer.peak() > 0.1,
                    "{sound:?} has a take that is almost silent"
                );
                assert!(
                    buffer.samples.iter().all(|s| s.is_finite()),
                    "{sound:?} has a non-finite sample"
                );
            }
        }
    }

    #[test]
    fn the_sounds_that_repeat_have_more_than_one_take() {
        // The claim is about the player's ear, not about the data: these are
        // the sounds that arrive in bursts, and one take is a loop.
        for sound in [
            Sound::Autocannon,
            Sound::EnemyFire,
            Sound::BladeHit,
            Sound::ImpactHard,
            Sound::ImpactSoft,
            Sound::Footstep,
        ] {
            let takes = recipes()
                .into_iter()
                .find(|(rendered, _)| *rendered == sound)
                .map(|(_, buffers)| buffers.len())
                .expect("the sound is in the set");
            assert!(takes > 1, "{sound:?} plays the same sample every time");
        }
    }

    #[test]
    fn a_turn_alternates_through_the_takes() {
        assert_eq!(take_index(0, 0), None, "a sound with no takes plays none");
        assert_eq!(take_index(2, 0), Some(0));
        assert_eq!(take_index(2, 1), Some(1));
        assert_eq!(take_index(2, 2), Some(0), "and then back to the first");
        assert_eq!(take_index(3, 7), Some(1));
    }

    #[test]
    fn a_repeated_shot_is_never_quite_the_same_shot() {
        // Small enough not to be heard as a pitch change, and never exactly
        // one, or the detune is doing nothing.
        let mut seen = std::collections::BTreeSet::new();
        for turn in 0..32 {
            let factor = detune(turn);
            assert!(
                (0.97..=1.03).contains(&factor),
                "shot {turn} is detuned to {factor}"
            );
            seen.insert((factor * 10_000.0) as i32);
        }
        assert!(seen.len() >= 6, "the detune is barely varying: {seen:?}");
        // And it is the same sequence every run, which is the whole reason it
        // is a hash rather than a random number.
        assert_eq!(detune(11), detune(11));
    }

    #[test]
    fn the_set_is_affordable_to_render_at_load() {
        // Everything in `recipes` is rendered before the first frame, and this
        // is the number that says whether that is still reasonable. It is a
        // load-time and memory claim, not a taste claim.
        let buffers: Vec<Buffer> = recipes()
            .into_iter()
            .flat_map(|(_, buffers)| buffers)
            .chain([
                synth::engine_bed(),
                synth::engine_load(),
                synth::thruster_roar(),
                synth::thruster_hiss(),
            ])
            .collect();
        let seconds: f32 = buffers
            .iter()
            .map(|buffer| buffer.len() as f32 / SAMPLE_RATE as f32)
            .sum();
        let bytes: usize = buffers.iter().map(|buffer| buffer.to_pcm16().len()).sum();
        println!(
            "{seconds:.2} s of audio, {} takes, {} kB of PCM",
            buffers.len(),
            bytes / 1024
        );
        assert!(seconds < 30.0, "{seconds} seconds of audio at load");
        assert!(
            bytes < 2_800_000,
            "{} kB of PCM held all session",
            bytes / 1024
        );
    }

    #[test]
    fn every_take_measures_as_the_family_it_belongs_to() {
        // The claim `audio.rs` makes about the set, restated where the table
        // lives: the weapon is brighter than the explosion, the machine gun is
        // brighter than the autocannon, and nothing in the HUD is as loud as a
        // weapon. If a buffer is wired to the wrong `Sound`, this fails.
        let take = |sound: Sound| -> Buffer {
            recipes()
                .into_iter()
                .find(|(rendered, _)| *rendered == sound)
                .map(|(_, mut buffers)| buffers.remove(0))
                .expect("the sound is in the set")
        };
        assert!(
            ears::centroid_hz(&take(Sound::Autocannon).samples)
                > ears::centroid_hz(&take(Sound::Explosion).samples) * 3.0
        );
        assert!(
            ears::centroid_hz(&take(Sound::EnemyFire).samples)
                > ears::centroid_hz(&take(Sound::Autocannon).samples)
        );
        assert!(
            ears::centroid_hz(&take(Sound::ImpactHard).samples)
                > ears::centroid_hz(&take(Sound::ImpactSoft).samples) * 1.8
        );
        assert!(
            take(Sound::UiClick).peak() < take(Sound::Explosion).peak() * 0.7,
            "the HUD is as loud as a weapon"
        );
    }
}
