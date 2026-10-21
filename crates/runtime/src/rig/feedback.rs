//! The feedback bus: every interaction event emits one cue that becomes a haptic pulse and a sound
//! on the same frame ("one event, one frame", Meta and Valve). Release mirrors grab at lower
//! amplitude; hover and zone ticks are short so they don't drown out the grab.
//!
//! A cue names the object it's about (the gun for its magazine, bolt and shots), so the sound
//! can be that object's own: the game's, from its sound pack (see `session`).
//! Each cue has a stable event name ([`Cue::name`]: `grab`, `grenade_pin`, `explosion` ...), so a
//! game's sound pack can replace the runtime's synthesized sounds cue by cue. The throwables' (the
//! research's cue set): the pin coming out is two crisp transients in both hands; arming (the
//! spoon popping, the fuse lighting) one strong one; the fuse ticks faintly in the hand holding
//! it; a throw is the soft mirror of the grab (a drop makes no throw sound); a knife sticking is
//! one short heavy transient in the hand that threw it; an explosion rumbles both hands with
//! distance.

use glam::Vec3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cue {
    Grab,
    Release,
    HolsterIn,
    HolsterOut,
    MagEject,
    MagSeat,
    SlideBack,
    SlideForward,
    LastRound,
    EmptyClick,
    Fail,
    GloveHover,
    GloveLock,
    GloveFlick,
    GloveCatch,
    Impact,
    ZoneTick,
    /// A magazine taken from the ammo pouch (the gun it's for is the object).
    Pouch,
    ShotPistol,
    ShotRifle,
    ShotShotgun,
    Melee,
    GrenadePin,
    GrenadeArm,
    GrenadeTick,
    GrenadeThrow,
    /// A grenade went off (the sound; its rumble is [`Cue::ExplosionRumble`] per hand).
    Explosion,
    ExplosionRumble,
    KnifeThrow,
    KnifeStick,
}

impl Cue {
    /// The cue's event name, for sound packs.
    pub fn name(self) -> &'static str {
        match self {
            Cue::Grab => "grab",
            Cue::Release => "release",
            Cue::HolsterIn => "holster_in",
            Cue::HolsterOut => "holster_out",
            Cue::Pouch => "pouch",
            Cue::MagEject => "mag_eject",
            Cue::MagSeat => "mag_seat",
            Cue::SlideBack => "slide_back",
            Cue::SlideForward => "slide_forward",
            Cue::LastRound => "last_round",
            Cue::EmptyClick => "empty_click",
            Cue::Fail => "fail",
            Cue::GloveHover => "glove_hover",
            Cue::GloveLock => "glove_lock",
            Cue::GloveFlick => "glove_flick",
            Cue::GloveCatch => "glove_catch",
            Cue::Impact => "impact",
            Cue::ZoneTick => "zone_tick",
            Cue::ShotPistol => "shot_pistol",
            Cue::ShotRifle => "shot_rifle",
            Cue::ShotShotgun => "shot_shotgun",
            Cue::Melee => "melee",
            Cue::GrenadePin => "grenade_pin",
            Cue::GrenadeArm => "grenade_arm",
            Cue::GrenadeTick => "grenade_tick",
            Cue::GrenadeThrow => "grenade_throw",
            Cue::Explosion => "explosion",
            Cue::ExplosionRumble => "explosion_rumble",
            Cue::KnifeThrow => "knife_throw",
            Cue::KnifeStick => "knife_stick",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Feedback {
    pub cue: Cue,
    /// Which hand vibrates (and where the sound comes from).
    pub hand: Option<usize>,
    /// The object it happened to (the gun, for a magazine going in or out of it).
    pub object: Option<u32>,
    /// Tracking-space position of the sound.
    pub pos: Vec3,
    /// 0..1: scales loudness and haptic strength (impact speed, ...).
    pub intensity: f32,
}

/// The haptic pulse for a cue: (seconds, frequency Hz, amplitude 0..1).
pub fn haptic(cue: Cue, intensity: f32) -> Option<(f32, f32, f32)> {
    let i = intensity.clamp(0.0, 1.0);
    Some(match cue {
        Cue::Grab => (0.03, 160.0, 0.55),
        Cue::Release => (0.02, 120.0, 0.3),
        Cue::HolsterIn => (0.05, 120.0, 0.6),
        Cue::HolsterOut => (0.03, 160.0, 0.5),
        Cue::MagEject => (0.04, 100.0, 0.6),
        Cue::MagSeat => (0.06, 90.0, 0.9),
        Cue::SlideBack => (0.03, 200.0, 0.7),
        Cue::SlideForward => (0.04, 150.0, 0.8),
        Cue::LastRound => (0.02, 250.0, 0.4),
        Cue::EmptyClick => (0.015, 300.0, 0.35),
        Cue::Fail => (0.06, 60.0, 0.5),
        Cue::GloveHover => (0.01, 200.0, 0.2),
        Cue::GloveLock => (0.03, 180.0, 0.5),
        Cue::GloveFlick => (0.04, 120.0, 0.6),
        // Alyx's `vr_hand_pull_catch_vibrate_time`: 0.125 s.
        Cue::GloveCatch => (0.125, 140.0, 0.8),
        // HIGGS: impact strength grows with speed; nothing for gentle touches.
        Cue::Impact => {
            if i < 0.05 {
                return None;
            }
            (0.01, 160.0, (0.1 + 0.6 * i).min(1.0))
        }
        Cue::ZoneTick | Cue::Pouch => (0.015, 160.0, 0.35),
        Cue::ShotPistol => (0.05, 200.0, 0.9),
        Cue::ShotRifle => (0.06, 180.0, 1.0),
        Cue::ShotShotgun => (0.09, 120.0, 1.0),
        Cue::Melee => (0.05, 100.0, (0.4 + 0.6 * i).min(1.0)),
        Cue::GrenadePin => (0.03, 230.0, 0.75),
        Cue::GrenadeArm => (0.045, 150.0, 0.95),
        Cue::GrenadeTick => (0.012, 200.0, 0.2 + 0.15 * i),
        Cue::GrenadeThrow | Cue::KnifeThrow => (0.02, 120.0, 0.3),
        // The sound alone: the rumble is per hand.
        Cue::Explosion => return None,
        Cue::ExplosionRumble => {
            if i < 0.05 {
                return None;
            }
            (0.12 + 0.2 * i, 55.0, (0.3 + 0.7 * i).min(1.0))
        }
        Cue::KnifeStick => (0.03, 90.0, 0.85),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_is_softer_than_grab_and_touches_are_silent() {
        let g = haptic(Cue::Grab, 1.0).unwrap();
        let r = haptic(Cue::Release, 1.0).unwrap();
        assert!(r.2 < g.2 && r.0 <= g.0);
        assert!(haptic(Cue::Impact, 0.01).is_none());
        assert!(haptic(Cue::Impact, 1.0).unwrap().2 > haptic(Cue::Impact, 0.2).unwrap().2);
    }

    #[test]
    fn throwable_cues_have_their_event_names() {
        for (c, n) in [(Cue::GrenadePin, "grenade_pin"), (Cue::GrenadeArm, "grenade_arm"), (Cue::GrenadeThrow, "grenade_throw"), (Cue::Explosion, "explosion"), (Cue::KnifeThrow, "knife_throw"), (Cue::KnifeStick, "knife_stick")] {
            assert_eq!(c.name(), n);
        }
        assert!(haptic(Cue::Explosion, 1.0).is_none(), "the explosion's sound has no pulse of its own");
        assert!(haptic(Cue::ExplosionRumble, 1.0).unwrap().0 > haptic(Cue::ExplosionRumble, 0.2).unwrap().0);
        assert!(haptic(Cue::GrenadeTick, 1.0).unwrap().2 < haptic(Cue::GrenadeArm, 1.0).unwrap().2, "ticks are faint");
    }
}
