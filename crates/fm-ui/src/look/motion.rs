// SPDX-License-Identifier: GPL-3.0-or-later
//! Animation speed. Every duration is written at its base value and passed
//! through `ms`, so one number slows or speeds them all (Kyle, Nov 2: "a
//! little bit slower overall").

/// How much longer every animation takes than its base duration.
pub const SLOWDOWN: f64 = 1.5;

/// A base duration in milliseconds at the current speed.
pub fn ms(base: u32) -> u32 {
    (base as f64 * SLOWDOWN).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_scale_by_the_slowdown() {
        assert_eq!(ms(200), 300);
        assert_eq!(ms(150), 225);
        assert_eq!(ms(0), 0);
    }
}
