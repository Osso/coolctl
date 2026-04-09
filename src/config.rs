use serde::Deserialize;
use std::fs;
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    ReadError(#[from] std::io::Error),
    #[error("Failed to parse config: {0}")]
    ParseError(#[from] toml::de::Error),
    #[error("Invalid config: {0}")]
    ValidationError(String),
}

/// Temperature thresholds for a profile
#[derive(Debug, Deserialize, Clone, Copy)]
pub struct ProfileThresholds {
    pub throttle_start: u32,
    pub throttle_max: u32,
}

/// Power profile names matching platform_profile values
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PowerProfile {
    Silent, // low-power
    #[default]
    Balanced, // balanced
    Performance, // performance
}

impl PowerProfile {
    pub fn from_platform_str(s: &str) -> Self {
        match s.trim() {
            "low-power" => PowerProfile::Silent,
            "balanced" => PowerProfile::Balanced,
            "performance" => PowerProfile::Performance,
            _ => PowerProfile::Balanced,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            PowerProfile::Silent => "silent",
            PowerProfile::Balanced => "balanced",
            PowerProfile::Performance => "performance",
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    /// Temperature to begin throttling (°C) - used for balanced profile
    pub throttle_start: u32,
    /// Temperature to force minimum frequency (°C) - used for balanced profile
    pub throttle_max: u32,
    /// Hysteresis in degrees before changing frequency
    pub hysteresis: u32,
    /// Polling interval in milliseconds
    pub poll_interval_ms: u64,
    /// Explicit sensor path (None = auto-detect)
    pub sensor: Option<String>,
    /// Sensor source preference: auto, hwmon, thermal
    pub sensor_source: SensorSource,
    /// Minimum frequency in kHz (None = auto-detect)
    pub min_freq: Option<u64>,
    /// Maximum frequency in kHz (None = auto-detect)
    pub max_freq: Option<u64>,
    /// Log level: error, warn, info, debug
    pub log_level: String,
    /// Optional log file path
    pub log_file: Option<String>,
    /// Enable profile switching based on platform_profile
    pub enable_profiles: bool,
    /// Silent profile thresholds (aggressive throttling for quiet fans)
    pub profile_silent: ProfileThresholds,
    /// Performance profile thresholds (minimal throttling, allow higher temps)
    pub profile_performance: ProfileThresholds,
}

#[derive(Debug, Deserialize, Clone, Copy, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SensorSource {
    #[default]
    Auto,
    Hwmon,
    Thermal,
}

impl Default for ProfileThresholds {
    fn default() -> Self {
        Self {
            throttle_start: 55,
            throttle_max: 72,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            throttle_start: 55,
            throttle_max: 72,
            hysteresis: 2,
            poll_interval_ms: 1000,
            sensor: None,
            sensor_source: SensorSource::Auto,
            min_freq: None,
            max_freq: None,
            log_level: "info".to_string(),
            log_file: None,
            enable_profiles: true,
            profile_silent: ProfileThresholds {
                throttle_start: 40,
                throttle_max: 50,
            },
            profile_performance: ProfileThresholds {
                throttle_start: 75,
                throttle_max: 90,
            },
        }
    }
}

impl Config {
    /// Get thresholds for a given profile
    pub fn thresholds_for(&self, profile: PowerProfile) -> ProfileThresholds {
        match profile {
            PowerProfile::Silent => self.profile_silent,
            PowerProfile::Balanced => ProfileThresholds {
                throttle_start: self.throttle_start,
                throttle_max: self.throttle_max,
            },
            PowerProfile::Performance => self.profile_performance,
        }
    }

    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let content = fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    pub fn load_or_default<P: AsRef<Path>>(path: P) -> Self {
        match Self::load(path) {
            Ok(config) => config,
            Err(e) => {
                log::warn!("Failed to load config, using defaults: {}", e);
                Self::default()
            }
        }
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.throttle_start >= self.throttle_max {
            return Err(ConfigError::ValidationError(
                "throttle_start must be less than throttle_max".to_string(),
            ));
        }
        if self.throttle_max > 110 {
            return Err(ConfigError::ValidationError(
                "throttle_max should not exceed 110°C".to_string(),
            ));
        }
        if self.poll_interval_ms < 100 {
            return Err(ConfigError::ValidationError(
                "poll_interval_ms should be at least 100ms".to_string(),
            ));
        }
        // Validate profile thresholds
        if self.profile_silent.throttle_start >= self.profile_silent.throttle_max {
            return Err(ConfigError::ValidationError(
                "profile_silent: throttle_start must be less than throttle_max".to_string(),
            ));
        }
        if self.profile_performance.throttle_start >= self.profile_performance.throttle_max {
            return Err(ConfigError::ValidationError(
                "profile_performance: throttle_start must be less than throttle_max".to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.throttle_start, 55);
        assert_eq!(config.throttle_max, 72);
        assert_eq!(config.hysteresis, 2);
    }

    #[test]
    fn test_parse_config() {
        let toml = r#"
            throttle_start = 60
            throttle_max = 75
            hysteresis = 3
        "#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.throttle_start, 60);
        assert_eq!(config.throttle_max, 75);
        assert_eq!(config.hysteresis, 3);
    }

    #[test]
    fn test_validation_fails_on_invalid_thresholds() {
        let config = Config {
            throttle_start: 80,
            throttle_max: 70,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }
}
