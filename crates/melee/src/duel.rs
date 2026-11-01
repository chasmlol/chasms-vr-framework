//! The duel: the framework's second melee mode (`docs/DUEL.md`), beside the physics sandbox. Slow,
//! weighty and timing-based: swings wind up, release and recover; a parry is a short window opened
//! by pushing the blade fast into the blow; a parry turns the blow away and staggers its swinger,
//! and the parrier's counter (the riposte) comes before the staggered one can swing again. A blow
//! can't be stopped any other way: a blade held still in its way is beaten aside, and a hit doesn't
//! stop a swing already coming.
//!
//! Every timing and weight of it is here, for the runtime (the player's side) and a game without its
//! own (the test range's duelist), so it's tuned in one place.

/// **The parry.** The blade pushed faster than this (m/s, at its middle) opens a parry: for
/// [`PARRY_WINDOW`] seconds a blow it meets is parried. A window that met nothing closes into
/// [`PARRY_RECOVERY`], when no new one opens (waving the blade about isn't a guard).
pub const PARRY_SPEED: f32 = 1.8;
pub const PARRY_WINDOW: f64 = 0.3;
pub const PARRY_RECOVERY: f64 = 0.35;
/// A parry pushes into the blow: the blade's way at the clash at least this near the way to the
/// other blade, or against the other's own motion (the cosine).
pub const PARRY_INTO: f32 = 0.2;
/// A parrying creature's blade meets the player's this much further out than it reaches (m): its
/// guard covers the line it's put across, not only its edge. Only a blow is parried: the player's
/// blade moving at least [`BLOW_SPEED`] (m/s).
pub const PARRY_REACH: f32 = 0.12;
pub const BLOW_SPEED: f32 = 1.5;

/// **The ricochet.** A parried blow throws the parrier's blade back off it at this speed (m/s), so
/// it has the way for the counter; the parrier's weapon is that much lighter in the hand for
/// [`RIPOSTE_TIME`] ([`DRIVE_RIPOSTE`]).
pub const RICOCHET: f32 = 2.6;
pub const RIPOSTE_TIME: f64 = 0.9;

/// **Being parried.** A swing parried is knocked back off the parry at this speed (m/s), and its
/// swinger is staggered for [`STAGGER`]: the weapon heavy in the hand ([`DRIVE_STAGGER`], easing
/// back to its weight over the stagger's second half), and harmless. The counter lands before it
/// can swing again; it can still parry once [`STAGGER_PARRY`] is up.
pub const DEFLECT: f32 = 3.0;
pub const STAGGER: f64 = 0.85;
pub const STAGGER_PARRY: f64 = 0.3;

/// **Beaten.** A blade met by a blow without a parry (held in its way, too slow, or pushed outside
/// its window) is knocked aside along the blow at this speed (m/s), and the blow carries on.
pub const BEATEN: f32 = 2.0;

/// Two blades meet once in this long (s): a clash isn't judged again while they stay together.
pub const CLASH_COOLDOWN: f64 = 0.35;

/// **The hold.** In a duel a held weapon is plain physics, as a whole body: its own mass and inertia,
/// held by a spring and a damper from the controllers to its grip, and turned towards the
/// controllers' turn by another (the arms' give). The pull moves its grip as stiffly as its mass
/// settles at [`GRIP_SETTLE`] rad/s, damped to [`GRIP_DAMPING`] of critical, with at most
/// [`HOLD_FORCE_ONE`] N ([`HOLD_FORCE_TWO`] in two hands); the turn as stiffly as it settles at
/// [`SWING_SETTLE`] rad/s about its grip, damped to [`SWING_DAMPING`], with at most
/// [`HOLD_TORQUE_ONE`] N m ([`HOLD_TORQUE_TWO`] in two hands): wrists firm against a small turn and
/// only so strong against a big one. The arms bear its weight while it's held (it doesn't droop), and
/// it rests on the player's body. So it moves by its own momentum: it trails hands that set off fast,
/// and carries on past hands that stop, braked no harder than the wrists can, pulling the hands (and
/// the arms and body drawn after them) along, a long heavy blade far more than a light one; then it
/// comes back to them. The settings' *strength on a weapon* scales the most force and torque: less is
/// heavier (more trail, more carry). Checked headless with the longsword in two hands, chopped from
/// overhead (0.8 s, up to speed in 0.3 s) and the hands stopped dead: it trails 18 degrees and
/// carries on 24 degrees past them, the hands pulled 13 cm on down, and is back with them 0.8 s later;
/// chopped fast (0.3 s) it trails 87 degrees and carries on 32.
pub const GRIP_SETTLE: f32 = 5.0;
pub const GRIP_DAMPING: f32 = 0.45;
pub const SWING_SETTLE: f32 = 14.0;
pub const SWING_DAMPING: f32 = 0.8;
pub const HOLD_FORCE_ONE: f32 = 150.0;
pub const HOLD_FORCE_TWO: f32 = 250.0;
pub const HOLD_TORQUE_ONE: f32 = 2.5;
pub const HOLD_TORQUE_TWO: f32 = 3.5;
/// A held weapon stopped against something lets go further from the shoulders in a duel than the
/// sandbox's reach (its swing has carried the hands on): this much further. A second hand on it lets
/// go once it's this far from its controller (m: past the furthest a swing carries it).
pub const STUCK_SLACK: f32 = 1.6;
pub const SUPPORT_BREAK: f32 = 0.9;

/// **The body.** A duel's held weapons rest on the player's body rather than going through it: their
/// head (this round, m), their torso and the line of their shoulders.
pub const BODY_HEAD: f32 = 0.1;
pub const BODY_TORSO: f32 = 0.13;
pub const BODY_SHOULDERS: f32 = 0.06;
/// Of the hands' strength: a duel's as they are, staggered (the weapon heavy, barely swung), and in a
/// riposte (lighter).
pub const DRIVE: f32 = 1.0;
pub const DRIVE_STAGGER: f32 = 0.25;
pub const DRIVE_RIPOSTE: f32 = 1.25;
/// Walking with a melee weapon in hand goes this much slower (of the stick's own speed).
pub const MOVE_DRAWN: f32 = 0.55;

/// **A creature's swing** (the range's duelist; a game may keep its own): its wind-up (it can be
/// read), its release (it hurts what it meets), its recovery (it can't swing or parry). A riposte
/// winds up in [`RIPOSTE_WINDUP`]. A swing that met nothing recovers longer ([`MISS_RECOVERY`]).
pub const WINDUP: f64 = 0.8;
pub const RELEASE: f64 = 0.65;
pub const RECOVERY: f64 = 0.45;
pub const MISS_RECOVERY: f64 = 0.7;
pub const RIPOSTE_WINDUP: f64 = 0.28;
/// Between its swings it waits this long at least, and this much longer at most (s).
pub const PAUSE: (f64, f64) = (0.6, 1.6);
/// A hit on it while it winds up holds its wind-up back this long (s: a flinch). A hit while it
/// releases does nothing to the swing: only a parry stops one.
pub const FLINCH: f64 = 0.15;

/// **A creature's eyes.** It sees the player's blade this late (s: a person's reaction), give or
/// take [`REACTION_SPREAD`]; it judges when the blade reaches it from how it moves then, and opens
/// its parry so the blow lands [`PARRY_LEAD`] into the window. A blade that slows (a drag) arrives
/// after its window has closed; one that speeds up (an accel) arrives before it has seen it coming.
pub const REACTION: f64 = 0.22;
pub const REACTION_SPREAD: f64 = 0.05;
pub const PARRY_LEAD: f64 = 0.1;
/// A blade further than this from its chest (m), or coming slower than [`THREAT_SPEED`] (m/s), isn't
/// a blow to parry yet; one due within [`THREAT_TIME`] (s) is.
pub const THREAT_REACH: f32 = 1.6;
pub const THREAT_SPEED: f32 = 1.2;
pub const THREAT_TIME: f64 = 0.6;

/// **Spacing.** A duelist keeps the player this far off (m: its blade's reach), steps in to swing
/// from no further than [`SWING_RANGE`], and walks this fast (m/s), slow with its weapon out.
pub const SPACING: f32 = 1.5;
pub const SWING_RANGE: f32 = 1.75;
pub const WALK: f32 = 0.6;

/// How long a staggered weapon takes to come back to its weight: none at first, all of it by the
/// stagger's end (0..1 of the way from [`DRIVE_STAGGER`] to [`DRIVE`]).
pub fn stagger_ease(since: f64) -> f32 {
    let half = STAGGER * 0.5;
    (((since - half) / half) as f32).clamp(0.0, 1.0)
}

/// The share of a duel hand's strength it has now, staggered since
/// `staggered` seconds and in a riposte for `riposte` more (each None when not).
pub fn drive(staggered: Option<f64>, riposte: Option<f64>) -> f32 {
    match (staggered, riposte) {
        (Some(s), _) if s < STAGGER => DRIVE_STAGGER + (DRIVE - DRIVE_STAGGER) * stagger_ease(s),
        (_, Some(_)) => DRIVE_RIPOSTE,
        _ => DRIVE,
    }
}
