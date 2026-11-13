//! Procedurally generated audio.
//!
//! Every sound is synthesised at startup into an in-memory `AudioSource`, so the game ships with
//! no audio assets. Digs, placements, machine hum, the reactor alarm and the Geiger click are
//! all short waveforms built from simple maths.

use bevy::{
    audio::{AudioPlayer, AudioSource, PlaybackSettings, Volume},
    prelude::*,
};

/// Sample rate used for every generated sound.
pub const SAMPLE_RATE: u32 = 22_050;

/// The bank of generated sounds.
#[derive(Resource, Default)]
pub struct SoundBank {
    /// A pickaxe hitting rock.
    pub dig: Handle<AudioSource>,
    /// A block being placed.
    pub place: Handle<AudioSource>,
    /// A machine running.
    pub machine_hum: Handle<AudioSource>,
    /// The reactor alarm.
    pub alarm: Handle<AudioSource>,
    /// One Geiger counter click.
    pub geiger: Handle<AudioSource>,
    /// A UI confirmation.
    pub chime: Handle<AudioSource>,
}

/// Client audio plugin.
pub struct AudioPlugin;

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SoundBank>()
            .add_systems(Startup, build_sound_bank)
            .add_systems(Update, (play_geiger_clicks,));
    }
}

/// Builds every sound into the asset system at startup.
fn build_sound_bank(mut sources: ResMut<Assets<AudioSource>>, mut bank: ResMut<SoundBank>) {
    bank.dig = sources.add(AudioSource {
        bytes: synth_dig().into(),
    });
    bank.place = sources.add(AudioSource {
        bytes: synth_place().into(),
    });
    bank.machine_hum = sources.add(AudioSource {
        bytes: synth_hum().into(),
    });
    bank.alarm = sources.add(AudioSource {
        bytes: synth_alarm().into(),
    });
    bank.geiger = sources.add(AudioSource {
        bytes: synth_click().into(),
    });
    bank.chime = sources.add(AudioSource {
        bytes: synth_chime().into(),
    });
    info!("generated {} procedural sounds", 6);
}

/// Wraps raw f32 samples as a WAV file in memory, which `AudioSource` can decode.
fn wav_from_samples(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

/// A short noise burst with a fast decay: a pickaxe on rock.
fn synth_dig() -> Vec<u8> {
    let len = SAMPLE_RATE as usize / 8;
    let mut rng = SimpleRng::new(0x51CE);
    let mut samples = Vec::with_capacity(len);
    for i in 0..len {
        let t = i as f32 / len as f32;
        let envelope = (1.0 - t).powi(3);
        // Noise plus a low thump.
        let noise = rng.next_bipolar() * 0.6;
        let thump = (t * 60.0 * std::f32::consts::TAU).sin() * 0.4;
        samples.push((noise + thump) * envelope * 0.7);
    }
    wav_from_samples(&samples)
}

/// A soft wooden click: placing a block.
fn synth_place() -> Vec<u8> {
    let len = SAMPLE_RATE as usize / 12;
    let mut samples = Vec::with_capacity(len);
    for i in 0..len {
        let t = i as f32 / len as f32;
        let envelope = (1.0 - t).powi(2);
        let tone = (t * 220.0 * std::f32::consts::TAU).sin() * 0.5;
        let click = (t * 900.0 * std::f32::consts::TAU).sin() * 0.2 * (1.0 - t).powi(6);
        samples.push((tone + click) * envelope * 0.5);
    }
    wav_from_samples(&samples)
}

/// A low steady hum with two detuned harmonics: a machine running.
fn synth_hum() -> Vec<u8> {
    let len = SAMPLE_RATE as usize;
    let mut samples = Vec::with_capacity(len);
    for i in 0..len {
        let t = i as f32 / SAMPLE_RATE as f32;
        let a = (t * 70.0 * std::f32::consts::TAU).sin() * 0.3;
        let b = (t * 105.5 * std::f32::consts::TAU).sin() * 0.15;
        let c = (t * 141.0 * std::f32::consts::TAU).sin() * 0.08;
        // Fade the ends so looping does not click.
        let fade = (t * 8.0).min(1.0) * ((1.0 - t) * 8.0).min(1.0);
        samples.push((a + b + c) * fade * 0.4);
    }
    wav_from_samples(&samples)
}

/// A two-tone rising alarm.
fn synth_alarm() -> Vec<u8> {
    let len = SAMPLE_RATE as usize / 2;
    let mut samples = Vec::with_capacity(len);
    for i in 0..len {
        let t = i as f32 / len as f32;
        let frequency = if t < 0.5 { 520.0 } else { 700.0 };
        let envelope = (1.0 - (t * 2.0 - 1.0).abs()).max(0.0).powi(2);
        samples.push((t * frequency * std::f32::consts::TAU).sin() * envelope * 0.5);
    }
    wav_from_samples(&samples)
}

/// A very short bright click: the Geiger counter.
fn synth_click() -> Vec<u8> {
    let len = SAMPLE_RATE as usize / 100;
    let mut samples = Vec::with_capacity(len);
    let mut rng = SimpleRng::new(0x9E1);
    for i in 0..len {
        let t = i as f32 / len as f32;
        let envelope = (1.0 - t).powi(8);
        samples.push(rng.next_bipolar() * envelope * 0.6);
    }
    wav_from_samples(&samples)
}

/// A pleasant two-note chime for confirmations.
fn synth_chime() -> Vec<u8> {
    let len = SAMPLE_RATE as usize / 4;
    let mut samples = Vec::with_capacity(len);
    for i in 0..len {
        let t = i as f32 / len as f32;
        let envelope = (1.0 - t).powi(3);
        let frequency = if t < 0.5 { 660.0 } else { 880.0 };
        samples.push((t * frequency * std::f32::consts::TAU).sin() * envelope * 0.4);
    }
    wav_from_samples(&samples)
}

/// A tiny xorshift PRNG so audio generation is deterministic and dependency-free.
struct SimpleRng {
    state: u32,
}

impl SimpleRng {
    fn new(seed: u32) -> Self {
        Self { state: seed | 1 }
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    fn next_bipolar(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1 << 23) as f32 - 1.0
    }
}

/// Plays a Geiger click whose rate rises with the player's radiation dose.
///
/// This is how players find uranium: the counter clicks faster as they approach it.
fn play_geiger_clicks(
    time: Res<Time>,
    bank: Res<SoundBank>,
    mut commands: Commands,
    hazards: Option<Res<ClientHazards>>,
    mut timer: Local<f32>,
) {
    let dose_rate = hazards.map_or(0.0, |h| h.ambient_dose_rate);
    if dose_rate <= 0.0 {
        return;
    }
    // Clicks per second scale with the dose rate, capped so it stays listenable.
    let clicks_per_second = (dose_rate * 8.0).clamp(0.5, 14.0);
    *timer += time.delta_secs();
    if *timer >= 1.0 / clicks_per_second {
        *timer = 0.0;
        commands.spawn((
            AudioPlayer(bank.geiger.clone()),
            PlaybackSettings::DESPAWN.with_volume(Volume::Linear(0.35)),
        ));
    }
}

/// The hazard state the HUD uses for the Geiger counter and radiation grain.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct ClientHazards {
    /// Radiation dose.
    pub dose: f32,
    /// Ambient dose rate, used to drive the Geiger click rate.
    pub ambient_dose_rate: f32,
    /// Body heat.
    pub heat: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_headers_are_well_formed() {
        let samples = vec![0.0f32; 100];
        let wav = wav_from_samples(&samples);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[36..40], b"data");
        // 44-byte header plus two bytes per sample.
        assert_eq!(wav.len(), 44 + samples.len() * 2);
    }

    #[test]
    fn every_sound_is_non_silent_and_bounded() {
        for (name, wav) in [
            ("dig", synth_dig()),
            ("place", synth_place()),
            ("hum", synth_hum()),
            ("alarm", synth_alarm()),
            ("click", synth_click()),
            ("chime", synth_chime()),
        ] {
            assert!(wav.len() > 44, "{name} is empty");
            // Decode the samples back out and check they are audible but not clipped.
            let samples: Vec<i16> = wav[44..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| i16::from_le_bytes(*c))
                .collect();
            let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
            assert!(peak > 1000, "{name} is too quiet (peak {peak})");
            assert!(peak < 32_700, "{name} clips (peak {peak})");
        }
    }

    #[test]
    fn sounds_are_deterministic() {
        assert_eq!(synth_dig(), synth_dig());
        assert_eq!(synth_click(), synth_click());
    }
}
