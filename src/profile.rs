use crate::config::PowerProfile;
use std::fs;
use std::path::Path;

const PLATFORM_PROFILE_PATH: &str = "/sys/firmware/acpi/platform_profile";

/// Platform profile watcher
pub struct ProfileWatcher {
    current: PowerProfile,
    available: bool,
}

impl ProfileWatcher {
    pub fn new() -> Self {
        let (current, available) = Self::read_profile();
        Self { current, available }
    }

    /// Check if platform profile interface is available
    pub fn is_available(&self) -> bool {
        self.available
    }

    /// Get current profile
    pub fn current(&self) -> PowerProfile {
        self.current
    }

    /// Check for profile change, returns Some(new_profile) if changed
    pub fn poll(&mut self) -> Option<PowerProfile> {
        if !self.available {
            return None;
        }

        let (new_profile, _) = Self::read_profile();
        if new_profile != self.current {
            self.current = new_profile;
            Some(new_profile)
        } else {
            None
        }
    }

    fn read_profile() -> (PowerProfile, bool) {
        let path = Path::new(PLATFORM_PROFILE_PATH);
        if !path.exists() {
            return (PowerProfile::default(), false);
        }

        match fs::read_to_string(path) {
            Ok(content) => (PowerProfile::from_platform_str(&content), true),
            Err(_) => (PowerProfile::default(), false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_from_platform_str() {
        assert_eq!(
            PowerProfile::from_platform_str("low-power\n"),
            PowerProfile::Silent
        );
        assert_eq!(
            PowerProfile::from_platform_str("balanced"),
            PowerProfile::Balanced
        );
        assert_eq!(
            PowerProfile::from_platform_str("performance\n"),
            PowerProfile::Performance
        );
        // Unknown defaults to balanced
        assert_eq!(
            PowerProfile::from_platform_str("unknown"),
            PowerProfile::Balanced
        );
    }
}
