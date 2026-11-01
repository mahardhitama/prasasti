pub const STEPS: [f64; 12] = [0.5, 0.67, 0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0];

/// Neighbouring rung, starting from the rung nearest `current`; the ends hold.
pub fn step(current: f64, direction: i32) -> f64 {
    let mut closest = 0;
    for i in 1..STEPS.len() {
        if (STEPS[i] - current).abs() < (STEPS[closest] - current).abs() {
            closest = i;
        }
    }
    let i = (closest as i64 + direction as i64).clamp(0, STEPS.len() as i64 - 1);
    STEPS[i as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_the_ladder() {
        assert_eq!(step(1.0, 1), 1.1);
        assert_eq!(step(1.0, -1), 0.9);
        assert_eq!(step(3.0, 1), 3.0);
        assert_eq!(step(0.5, -1), 0.5);
        assert_eq!(step(1.04, 1), 1.1); // starts from the nearest rung
    }

    #[test]
    fn huge_direction_and_nan_hold() {
        assert_eq!(step(1.0, i32::MAX), 3.0);
        assert_eq!(step(1.0, i32::MIN), 0.5);
        assert!(STEPS.contains(&step(f64::NAN, 1)));
    }
}
