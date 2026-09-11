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

use godot::classes::{AudioStreamPlayer, AudioStreamPlayer3D, AudioStreamWav, Node3D};
use godot::prelude::*;

use ashframe_sim::types::Vec3;

use crate::palette::to_godot;
use crate::synth::{self, Buffer, Shape, Wave, SAMPLE_RATE};

/// Simultaneous positional one-shots.
///
/// The original capped its voices at twenty-eight for the same reason.
const VOICE_POOL: usize = 28;
/// Simultaneous non-positional sounds: the HUD and the results screen.
const UI_POOL: usize = 6;

/// Every sound the game can make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    FireRifle,
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
    LockAcquired,
    LockLost,
    Telegraph,
    Repair,
    UiClick,
    UiConfirm,
    MissionComplete,
    MissionFailed,
}

/// The rendered set.
struct Streams {
    fire_rifle: Gd<AudioStreamWav>,
    enemy_fire: Gd<AudioStreamWav>,
    missile_launch: Gd<AudioStreamWav>,
    blade_swing: Gd<AudioStreamWav>,
    blade_hit: Gd<AudioStreamWav>,
    impact_hard: Gd<AudioStreamWav>,
    impact_soft: Gd<AudioStreamWav>,
    explosion: Gd<AudioStreamWav>,
    player_damage: Gd<AudioStreamWav>,
    player_stagger: Gd<AudioStreamWav>,
    energy_warning: Gd<AudioStreamWav>,
    boost: Gd<AudioStreamWav>,
    landing: Gd<AudioStreamWav>,
    lock_acquired: Gd<AudioStreamWav>,
    lock_lost: Gd<AudioStreamWav>,
    telegraph: Gd<AudioStreamWav>,
    repair: Gd<AudioStreamWav>,
    ui_click: Gd<AudioStreamWav>,
    ui_confirm: Gd<AudioStreamWav>,
    mission_complete: Gd<AudioStreamWav>,
    mission_failed: Gd<AudioStreamWav>,
    engine: Gd<AudioStreamWav>,
    thruster: Gd<AudioStreamWav>,
}

/// The audio system.
pub struct Audio {
    voices: Vec<Gd<AudioStreamPlayer3D>>,
    ui: Vec<Gd<AudioStreamPlayer>>,
    engine: Gd<AudioStreamPlayer>,
    thruster: Gd<AudioStreamPlayer>,
    streams: Streams,
    next_voice: usize,
    next_ui: usize,
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

        let mut engine = AudioStreamPlayer::new_alloc();
        engine.set_bus(&StringName::from("Master"));
        parent.add_child(&engine);

        let mut thruster = AudioStreamPlayer::new_alloc();
        thruster.set_bus(&StringName::from("Master"));
        parent.add_child(&thruster);

        let streams = render_all();
        engine.set_stream(&streams.engine);
        thruster.set_stream(&streams.thruster);
        engine.set_volume_db(-12.0);
        thruster.set_volume_db(-24.0);
        engine.play();
        thruster.play();

        Self {
            voices,
            ui,
            engine,
            thruster,
            streams,
            next_voice: 0,
            next_ui: 0,
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
        let stream = self.stream_for(sound).clone();
        if self.voices.is_empty() {
            return;
        }
        let index = self.next_voice % self.voices.len();
        self.next_voice = self.next_voice.wrapping_add(1);
        let player = &mut self.voices[index];
        player.set_stream(&stream);
        player.set_position(to_godot(at));
        player.set_pitch_scale(pitch);
        player.play();
    }

    /// A one-shot with no position: the HUD and the results screen.
    pub fn play_ui(&mut self, sound: Sound) {
        let stream = self.stream_for(sound).clone();
        if self.ui.is_empty() {
            return;
        }
        let index = self.next_ui % self.ui.len();
        self.next_ui = self.next_ui.wrapping_add(1);
        let player = &mut self.ui[index];
        player.set_stream(&stream);
        player.set_pitch_scale(1.0);
        player.play();
    }

    /// Follow the mech with the sustained voices.
    ///
    /// `speed` in metres per second, `thrust` 0..1.
    pub fn drive(&mut self, speed: f32, thrust: f32, boosting: bool) {
        let norm = (speed / 48.0).clamp(0.0, 1.0);
        // Pitch rather than volume does most of the work: a mech under load
        // sounds like it is working harder, and simply making it louder reads
        // as a mixing mistake.
        let pitch = 0.72 + norm * 0.5 + if boosting { 0.22 } else { 0.0 };
        self.engine.set_pitch_scale(pitch);
        self.engine
            .set_volume_db(linear_to_db(0.35 + norm * 0.4));
        self.thruster.set_pitch_scale(0.85 + thrust * 0.6);
        self.thruster.set_volume_db(linear_to_db(thrust * 0.6));
    }

    /// Silence the sustained voices, for the briefing and the results screen.
    pub fn hush(&mut self) {
        self.engine.set_volume_db(-60.0);
        self.thruster.set_volume_db(-60.0);
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

    fn stream_for(&self, sound: Sound) -> &Gd<AudioStreamWav> {
        match sound {
            Sound::FireRifle => &self.streams.fire_rifle,
            Sound::EnemyFire => &self.streams.enemy_fire,
            Sound::MissileLaunch => &self.streams.missile_launch,
            Sound::BladeSwing => &self.streams.blade_swing,
            Sound::BladeHit => &self.streams.blade_hit,
            Sound::ImpactHard => &self.streams.impact_hard,
            Sound::ImpactSoft => &self.streams.impact_soft,
            Sound::Explosion => &self.streams.explosion,
            Sound::PlayerDamage => &self.streams.player_damage,
            Sound::PlayerStagger => &self.streams.player_stagger,
            Sound::EnergyWarning => &self.streams.energy_warning,
            Sound::Boost => &self.streams.boost,
            Sound::Landing => &self.streams.landing,
            Sound::LockAcquired => &self.streams.lock_acquired,
            Sound::LockLost => &self.streams.lock_lost,
            Sound::Telegraph => &self.streams.telegraph,
            Sound::Repair => &self.streams.repair,
            Sound::UiClick => &self.streams.ui_click,
            Sound::UiConfirm => &self.streams.ui_confirm,
            Sound::MissionComplete => &self.streams.mission_complete,
            Sound::MissionFailed => &self.streams.mission_failed,
        }
    }
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

/// Render every sound once.
fn render_all() -> Streams {
    Streams {
        // A shot: a bright burst swept down, over a short square thump.
        fire_rifle: stream({
            let mut mix = synth::noise_burst(0.11, 0.34, 2600.0, 320.0, Shape::Band, 1.4);
            mix.mix_at(&synth::tone(Wave::Square, 180.0, 60.0, 0.07, 0.16), 0.0, 1.0);
            mix
        }),
        enemy_fire: stream({
            let mut mix = synth::noise_burst(0.09, 0.14, 1400.0, 260.0, Shape::Band, 1.2);
            mix.mix_at(&synth::tone(Wave::Saw, 120.0, 50.0, 0.06, 0.07), 0.0, 1.0);
            mix
        }),
        // The launch is the whoosh; the note under it is the motor lighting.
        missile_launch: stream({
            let mut mix = synth::noise_burst(0.5, 0.22, 300.0, 2400.0, Shape::Band, 0.8);
            mix.mix_at(&synth::tone(Wave::Saw, 90.0, 260.0, 0.3, 0.09), 0.0, 1.0);
            mix
        }),
        blade_swing: stream({
            let mut mix = synth::noise_burst(0.26, 0.24, 380.0, 2600.0, Shape::Band, 2.2);
            mix.mix_at(&synth::tone(Wave::Sine, 420.0, 1500.0, 0.22, 0.1), 0.0, 1.0);
            mix
        }),
        // A metallic clang is a few detuned partials with a fast decay, not a
        // single note: a struck plate rings at several unrelated frequencies.
        blade_hit: stream({
            let mut mix = Buffer::silence(0.34);
            for (freq, gain) in [(340.0, 0.20), (517.0, 0.14), (790.0, 0.10), (1230.0, 0.07)] {
                mix.mix_at(
                    &synth::tone(Wave::Triangle, freq, freq * 0.72, 0.34, gain),
                    0.0,
                    1.0,
                );
            }
            mix.mix_at(
                &synth::noise_burst(0.3, 0.3, 3200.0, 500.0, Shape::Band, 1.1),
                0.0,
                1.0,
            );
            mix
        }),
        // Hard surfaces ring; soft ones thud. The simulation tags every prop
        // with what it is made of, so the impact can pick the right one.
        impact_hard: stream({
            let mut mix = synth::tone(Wave::Triangle, 620.0, 240.0, 0.16, 0.14);
            mix.mix_at(
                &synth::noise_burst(0.14, 0.2, 2400.0, 700.0, Shape::Band, 1.6),
                0.0,
                1.0,
            );
            mix
        }),
        impact_soft: stream({
            let mut mix = synth::noise_burst(0.16, 0.22, 900.0, 180.0, Shape::Low, 0.9);
            mix.mix_at(&synth::tone(Wave::Sine, 150.0, 70.0, 0.14, 0.12), 0.0, 1.0);
            mix
        }),
        explosion: stream({
            let mut mix = synth::tone(Wave::Sine, 150.0, 32.0, 0.85, 0.4);
            mix.mix_at(
                &synth::noise_burst(0.9, 0.36, 1500.0, 90.0, Shape::Low, 0.7),
                0.0,
                1.0,
            );
            mix.mix_at(
                &synth::noise_burst(0.35, 0.2, 3200.0, 400.0, Shape::Band, 0.8),
                0.0,
                1.0,
            );
            mix
        }),
        player_damage: stream({
            let mut mix = synth::tone(Wave::Saw, 200.0, 80.0, 0.2, 0.22);
            mix.mix_at(
                &synth::noise_burst(0.2, 0.24, 700.0, 120.0, Shape::Low, 0.8),
                0.0,
                1.0,
            );
            mix
        }),
        player_stagger: stream({
            let mut mix = synth::tone(Wave::Saw, 300.0, 60.0, 0.7, 0.26);
            mix.mix_at(
                &synth::noise_burst(0.6, 0.2, 1200.0, 140.0, Shape::Band, 0.9),
                0.0,
                1.0,
            );
            mix
        }),
        energy_warning: stream(synth::tone(Wave::Square, 720.0, 480.0, 0.16, 0.1)),
        boost: stream(synth::noise_burst(0.3, 0.2, 700.0, 2800.0, Shape::Band, 0.9)),
        landing: stream({
            let mut mix = synth::tone(Wave::Sine, 120.0, 40.0, 0.28, 0.3);
            mix.mix_at(
                &synth::noise_burst(0.22, 0.21, 600.0, 120.0, Shape::Low, 0.8),
                0.0,
                1.0,
            );
            mix
        }),
        // Two rising blips, which is the shortest way to say "acquired".
        lock_acquired: stream({
            let mut mix = synth::tone(Wave::Square, 880.0, 1320.0, 0.07, 0.08);
            mix.mix_at(
                &synth::tone(Wave::Square, 1320.0, 1760.0, 0.06, 0.06),
                0.07,
                1.0,
            );
            mix
        }),
        lock_lost: stream(synth::tone(Wave::Square, 700.0, 380.0, 0.1, 0.06)),
        telegraph: stream(synth::tone(Wave::Saw, 160.0, 220.0, 0.5, 0.07)),
        repair: stream({
            let mut mix = synth::tone(Wave::Sine, 320.0, 720.0, 0.5, 0.12);
            mix.mix_at(&synth::tone(Wave::Sine, 480.0, 960.0, 0.4, 0.08), 0.12, 1.0);
            mix
        }),
        ui_click: stream(synth::tone(Wave::Square, 900.0, 700.0, 0.05, 0.05)),
        ui_confirm: stream({
            let mut mix = synth::tone(Wave::Square, 520.0, 780.0, 0.09, 0.07);
            mix.mix_at(
                &synth::tone(Wave::Square, 780.0, 1180.0, 0.12, 0.06),
                0.08,
                1.0,
            );
            mix
        }),
        mission_complete: stream({
            let mut mix = Buffer::silence(0.9);
            for (i, freq) in [523.0, 659.0, 784.0, 1046.0].into_iter().enumerate() {
                mix.mix_at(
                    &synth::tone(Wave::Triangle, freq, freq, 0.5, 0.14),
                    i as f32 * 0.14,
                    1.0,
                );
            }
            mix
        }),
        mission_failed: stream({
            let mut mix = Buffer::silence(1.1);
            for (i, freq) in [392.0, 330.0, 262.0, 196.0].into_iter().enumerate() {
                mix.mix_at(
                    &synth::tone(Wave::Saw, freq, freq * 0.98, 0.6, 0.14),
                    i as f32 * 0.18,
                    1.0,
                );
            }
            mix
        }),
        // 0.5 s is exactly 21 cycles of 42 Hz, so the loop closes on itself.
        engine: loop_stream({
            let mut mix = Buffer::silence(0.5);
            for harmonic in 1..=6 {
                let freq = 42.0 * harmonic as f32;
                mix.mix_at(
                    &synth::tone(Wave::Saw, freq, freq, 0.5, 0.16 / harmonic as f32),
                    0.0,
                    1.0,
                );
            }
            mix
        }),
        thruster: loop_stream(synth::noise_burst(0.6, 0.5, 620.0, 620.0, Shape::Band, 0.9)),
    }
}

/// A one-shot: normalised and faded at both ends so it cannot click.
fn stream(mut buffer: Buffer) -> Gd<AudioStreamWav> {
    buffer.normalise(0.85);
    buffer.fade_edges(0.004);
    to_wav(&buffer)
}

/// A sustained loop: cross-faded at the seam and quieter, because it plays
/// under everything else.
fn loop_stream(mut buffer: Buffer) -> Gd<AudioStreamWav> {
    buffer.normalise(0.6);
    synth::make_loopable(&mut buffer, 0.02);
    to_wav(&buffer)
}

fn to_wav(buffer: &Buffer) -> Gd<AudioStreamWav> {
    let mut stream = AudioStreamWav::new_gd();
    stream.set_format(godot::classes::audio_stream_wav::Format::FORMAT_16_BITS);
    stream.set_mix_rate(SAMPLE_RATE as i32);
    stream.set_stereo(false);
    stream.set_data(&PackedByteArray::from(buffer.to_pcm16().as_slice()));
    stream
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_quiet_in_decibels() {
        // The logarithm of zero is not a quiet sound, it is negative infinity,
        // and a mixer fed that produces a NaN and stops.
        assert_eq!(linear_to_db(0.0), -60.0);
        assert!(linear_to_db(1.0).abs() < 1e-4);
        assert!(linear_to_db(0.5) < -5.9 && linear_to_db(0.5) > -6.1);
    }
}
