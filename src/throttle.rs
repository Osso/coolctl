use crate::config::{Config, ProfileThresholds};

/// Throttle controller with hysteresis
pub struct Throttle {
    /// Temperature to start throttling (°C)
    throttle_start: u32,
    /// Temperature to force minimum frequency (°C)
    throttle_max: u32,
    /// Minimum frequency (kHz)
    min_freq: u64,
    /// Maximum frequency (kHz)
    max_freq: u64,
    /// Hysteresis in degrees
    hysteresis: u32,
    /// Last temperature that triggered a frequency change
    last_trigger_temp: Option<u32>,
}

impl Throttle {
    pub fn new(config: &Config, min_freq: u64, max_freq: u64) -> Self {
        Self {
            throttle_start: config.throttle_start,
            throttle_max: config.throttle_max,
            min_freq: config.min_freq.unwrap_or(min_freq),
            max_freq: config.max_freq.unwrap_or(max_freq),
            hysteresis: config.hysteresis,
            last_trigger_temp: None,
        }
    }

    /// Update thresholds (e.g., when switching profiles)
    pub fn update_thresholds(&mut self, thresholds: ProfileThresholds) {
        self.throttle_start = thresholds.throttle_start;
        self.throttle_max = thresholds.throttle_max;
        // Reset hysteresis state to force recalculation with new thresholds
        self.last_trigger_temp = None;
    }

    /// Calculate target frequency based on temperature
    /// Returns Some(freq) if frequency should change, None if within hysteresis
    pub fn calculate(&mut self, temp: u32) -> Option<u64> {
        // Check hysteresis: only change if temp moved enough from last trigger
        if let Some(last_temp) = self.last_trigger_temp {
            let diff = (temp as i32 - last_temp as i32).unsigned_abs();
            if diff < self.hysteresis {
                return None;
            }
        }

        let target = self.target_freq(temp);

        // Update last trigger temperature
        self.last_trigger_temp = Some(temp);

        Some(target)
    }

    /// Calculate target frequency without hysteresis check
    fn target_freq(&self, temp: u32) -> u64 {
        if temp < self.throttle_start {
            // Below threshold: full speed
            self.max_freq
        } else if temp >= self.throttle_max {
            // At or above max: minimum frequency
            self.min_freq
        } else {
            // Linear interpolation
            let temp_range = (self.throttle_max - self.throttle_start) as u64;
            let freq_range = self.max_freq - self.min_freq;
            let temp_above_start = (temp - self.throttle_start) as u64;

            // Linear: max_freq at throttle_start, min_freq at throttle_max
            let reduction = (freq_range * temp_above_start) / temp_range;
            self.max_freq - reduction
        }
    }

    /// Force recalculation on next call (clear hysteresis state)
    #[cfg(test)]
    pub fn reset(&mut self) {
        self.last_trigger_temp = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config {
            throttle_start: 55,
            throttle_max: 72,
            hysteresis: 2,
            ..Default::default()
        }
    }

    #[test]
    fn test_below_threshold() {
        let mut throttle = Throttle::new(&test_config(), 400_000, 5_000_000);

        // Below throttle_start: should return max_freq
        let freq = throttle.calculate(50);
        assert_eq!(freq, Some(5_000_000));
    }

    #[test]
    fn test_above_max() {
        let mut throttle = Throttle::new(&test_config(), 400_000, 5_000_000);

        // At or above throttle_max: should return min_freq
        let freq = throttle.calculate(72);
        assert_eq!(freq, Some(400_000));

        throttle.reset();
        let freq = throttle.calculate(80);
        assert_eq!(freq, Some(400_000));
    }

    #[test]
    fn test_linear_interpolation() {
        let mut throttle = Throttle::new(&test_config(), 400_000, 5_000_000);

        // Midpoint: 55 + (72-55)/2 = 63.5°C
        // Should be roughly halfway between max and min
        let freq = throttle.calculate(64).unwrap();

        // Expected: 5_000_000 - (4_600_000 * 9) / 17 ≈ 2_564_705
        let expected_min = 2_000_000;
        let expected_max = 3_000_000;
        assert!(
            freq >= expected_min && freq <= expected_max,
            "freq {} should be between {} and {}",
            freq,
            expected_min,
            expected_max
        );
    }

    #[test]
    fn test_hysteresis() {
        let mut throttle = Throttle::new(&test_config(), 400_000, 5_000_000);

        // First reading: should trigger
        let freq1 = throttle.calculate(60);
        assert!(freq1.is_some());

        // Within hysteresis: should not trigger
        let freq2 = throttle.calculate(61);
        assert!(freq2.is_none());

        // Outside hysteresis: should trigger
        let freq3 = throttle.calculate(63);
        assert!(freq3.is_some());
    }
}
