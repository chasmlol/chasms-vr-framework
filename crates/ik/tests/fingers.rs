//! Finger curls through the public API.

use rustvr_ik::{FingerInput, FingerSolver, HoldShape};

fn settle(f: &mut FingerSolver, input: &FingerInput, hold: HoldShape) -> rustvr_protocol::Fingers {
    let mut out = f.update(0.0, input, hold);
    for _ in 0..60 {
        out = f.update(1.0 / 90.0, input, hold);
    }
    out
}

#[test]
fn holds_wrap_the_fingers_whatever_the_grip_button_says() {
    let open = FingerInput::default();
    let squeezed = FingerInput { squeeze: 1.0, touch_thumb: true, ..Default::default() };
    let shapes = [HoldShape::PistolGrip, HoldShape::Foregrip, HoldShape::Magazine, HoldShape::Small, HoldShape::Large];
    let mut middle = Vec::new();
    for hold in shapes {
        let mut f = FingerSolver::new();
        let a = settle(&mut f, &open, hold);
        let b = settle(&mut f, &squeezed, hold);
        // Middle to pinky follow the object, not the grip button.
        assert!(a.curl[2..].iter().zip(&b.curl[2..]).all(|(x, y)| (x - y).abs() < 1e-4), "{hold:?}");
        assert!(a.curl.iter().all(|c| (0.0..=1.0).contains(c)) && (0.0..=1.0).contains(&a.splay));
        middle.push(a.curl[2]);
    }
    // A large object opens the hand more than a pistol grip.
    assert!(middle[4] < middle[0]);
}

#[test]
fn the_trigger_finger_follows_the_trigger_on_a_gun() {
    let mut f = FingerSolver::new();
    let mut last = -1.0;
    for pull in [0.0, 0.3, 0.6, 1.0] {
        let input = FingerInput { trigger: pull, squeeze: 1.0, touch_trigger: true, ..Default::default() };
        let index = settle(&mut f, &input, HoldShape::PistolGrip).curl[1];
        assert!(index > last, "index curls as the trigger is pulled");
        last = index;
    }
}
