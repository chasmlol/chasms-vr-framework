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
    // The actions of guns that don't cycle themselves (`rig::actions`).
    /// A break action's latch let go (its barrels start to fall open), and its barrels latching shut.
    BreakOpen,
    BreakClose,
    /// A swing-out barrel or cylinder unlatched, and back in and latched.
    SwingOpen,
    SwingClose,
    /// A round pushed into a chamber, and a live one pulled back out by hand.
    ShellIn,
    ShellOut,
    /// A case (or a live round) thrown out of a chamber: a break action opening, a bolt or pump
    /// at the back of its stroke.
    CaseEject,
    /// A bolt turned up out of its lugs, pulled back, pushed home and turned down.
    BoltUp,
    BoltBack,
    BoltForward,
    BoltDown,
    /// A pump action's forend pulled back and pushed forward.
    PumpBack,
    PumpForward,
    /// An air gun's pump stroke (pressure building), and its trigger without the pressure to fire.
    Pump,
    AirHiss,
    // Pockets on the game's creatures (`rig::pockets`).
    /// A hand going into a pocket (a soft brush), and finding it empty or shut.
    PocketIn,
    PocketEmpty,
    PocketShut,
    /// A finger coming on a thing in a pocket: its feel (`FEEL_*`).
    PocketFeel(u32),
    /// A thing pulled out of a pocket, and the game's answer that it didn't go unnoticed (the hand
    /// jolts and the thing is gone from it).
    PocketOut,
    PocketCaught,
    /// The pocket's creature sees the player: a slow heartbeat in the hand in it (stronger as it
    /// first notices them).
    PocketWatched,
    /// A thing let go into a pocket.
    PocketPlant,
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
            Cue::BreakOpen => "break_open",
            Cue::BreakClose => "break_close",
            Cue::SwingOpen => "swing_open",
            Cue::SwingClose => "swing_close",
            Cue::ShellIn => "shell_in",
            Cue::ShellOut => "shell_out",
            Cue::CaseEject => "case_eject",
            Cue::BoltUp => "bolt_up",
            Cue::BoltBack => "bolt_back",
            Cue::BoltForward => "bolt_forward",
            Cue::BoltDown => "bolt_down",
            Cue::PumpBack => "pump_back",
            Cue::PumpForward => "pump_forward",
            Cue::Pump => "pump",
            Cue::AirHiss => "air_hiss",
            Cue::PocketIn => "pocket_in",
            Cue::PocketEmpty => "pocket_empty",
            Cue::PocketShut => "pocket_shut",
            Cue::PocketFeel(_) => "pocket_feel",
            Cue::PocketOut => "pocket_out",
            Cue::PocketCaught => "pocket_caught",
            Cue::PocketWatched => "pocket_watched",
            Cue::PocketPlant => "pocket_plant",
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
        // The latch going is a click; shutting is the heaviest of them (metal on metal).
        Cue::BreakOpen | Cue::SwingOpen => (0.025, 180.0, 0.6),
        Cue::BreakClose | Cue::SwingClose => (0.05, 110.0, 0.95),
        Cue::ShellIn => (0.03, 140.0, 0.6),
        Cue::ShellOut => (0.02, 160.0, 0.4),
        Cue::CaseEject => (0.015, 220.0, 0.3),
        Cue::BoltUp | Cue::BoltDown => (0.02, 200.0, 0.5),
        Cue::BoltBack | Cue::PumpBack => (0.035, 170.0, 0.75),
        Cue::BoltForward | Cue::PumpForward => (0.045, 140.0, 0.85),
        Cue::Pump => (0.04, 90.0, 0.6),
        Cue::AirHiss => (0.03, 320.0, 0.25),
        // Felt more than heard: a thief's hand.
        Cue::PocketIn => (0.03, 80.0, 0.3),
        Cue::PocketEmpty => (0.05, 50.0, 0.25),
        Cue::PocketShut => (0.06, 60.0, 0.5),
        Cue::PocketFeel(kind) => super::pockets::feel(kind),
        Cue::PocketOut => (0.03, 120.0, 0.45),
        // Caught: the hardest jolt there is short of an explosion.
        Cue::PocketCaught => (0.18, 60.0, 1.0),
        Cue::PocketWatched => (0.05, 45.0, 0.3 + 0.5 * i),
        Cue::PocketPlant => (0.04, 100.0, 0.5),
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

    #[test]
    fn the_actions_cues_have_their_event_names_and_a_pulse() {
        let cues = [
            (Cue::BreakOpen, "break_open"),
            (Cue::BreakClose, "break_close"),
            (Cue::SwingOpen, "swing_open"),
            (Cue::SwingClose, "swing_close"),
            (Cue::ShellIn, "shell_in"),
            (Cue::ShellOut, "shell_out"),
            (Cue::CaseEject, "case_eject"),
            (Cue::BoltUp, "bolt_up"),
            (Cue::BoltBack, "bolt_back"),
            (Cue::BoltForward, "bolt_forward"),
            (Cue::BoltDown, "bolt_down"),
            (Cue::PumpBack, "pump_back"),
            (Cue::PumpForward, "pump_forward"),
            (Cue::Pump, "pump"),
            (Cue::AirHiss, "air_hiss"),
        ];
        for (c, n) in cues {
            assert_eq!(c.name(), n);
            assert!(haptic(c, 1.0).is_some(), "{n}");
        }
        assert!(haptic(Cue::BreakClose, 1.0).unwrap().2 > haptic(Cue::BreakOpen, 1.0).unwrap().2, "shutting is the heavier");
    }
}
