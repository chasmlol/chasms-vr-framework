//! Turning the runtime's action counters into the game's own input.

use rustvr_protocol::{ACTION_COUNT, Action, Intents};

/// Detects presses from [`Intents::presses`] counters, so none is lost when the game skips frames.
#[derive(Default)]
pub struct Presses {
    last: Option<[u16; 64]>,
}

impl Presses {
    /// Returns how many times each action was pressed since the previous call.
    pub fn update(&mut self, intents: &Intents) -> [u16; ACTION_COUNT] {
        let mut out = [0u16; ACTION_COUNT];
        if let Some(last) = &self.last {
            for i in 0..ACTION_COUNT {
                out[i] = intents.presses[i].wrapping_sub(last[i]);
            }
        }
        self.last = Some(intents.presses);
        out
    }

    /// Forget the counters (e.g. after reconnecting), so old presses aren't replayed.
    pub fn reset(&mut self) {
        self.last = None;
    }
}

pub fn pressed(counts: &[u16; ACTION_COUNT], a: Action) -> bool {
    counts[a as usize] > 0
}

pub fn held(intents: &Intents, a: Action) -> bool {
    intents.held & a.bit() != 0
}

/// Holds game buttons down for a short time per press, for games that poll input and would miss
/// a one-frame tap (0.12 s is enough).
pub struct Pulses {
    until: [f64; 64],
}

impl Default for Pulses {
    fn default() -> Self {
        Pulses { until: [0.0; 64] }
    }
}

impl Pulses {
    pub fn pulse(&mut self, bit: usize, now: f64, secs: f64) {
        self.until[bit] = self.until[bit].max(now + secs);
    }

    pub fn active(&self, bit: usize, now: f64) -> bool {
        self.until[bit] > now
    }

    /// Bit `i` set while pulse `i` is active.
    pub fn bits(&self, now: f64) -> u64 {
        let mut b = 0u64;
        for (i, &t) in self.until.iter().enumerate() {
            if t > now {
                b |= 1 << i;
            }
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    #[test]
    fn presses_count_across_skipped_frames() {
        let mut p = Presses::default();
        let mut i = Intents::zeroed();
        assert_eq!(p.update(&i)[Action::Reload as usize], 0, "the first sample only primes");
        i.presses[Action::Reload as usize] = 2;
        assert_eq!(p.update(&i)[Action::Reload as usize], 2);
        i.presses[Action::Reload as usize] = 1; // wrapped past u16::MAX
        i.presses[Action::Reload as usize] = i.presses[Action::Reload as usize].wrapping_add(u16::MAX);
        assert_eq!(p.update(&i)[Action::Reload as usize], u16::MAX - 1);
    }

    #[test]
    fn pulses_expire() {
        let mut p = Pulses::default();
        p.pulse(3, 1.0, 0.12);
        assert_eq!(p.bits(1.05), 1 << 3);
        assert_eq!(p.bits(1.2), 0);
    }
}
