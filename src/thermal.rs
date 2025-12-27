use crate::config::SensorSource;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ThermalError {
    #[error("Failed to read temperature: {0}")]
    ReadError(#[from] std::io::Error),
    #[error("Failed to parse temperature value")]
    ParseError,
    #[error("No suitable temperature sensor found")]
    NoSensorFound,
}

/// Temperature sensor abstraction
pub struct ThermalSensor {
    path: PathBuf,
}

impl ThermalSensor {
    /// Create sensor from explicit path
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self, ThermalError> {
        let path = path.as_ref().to_path_buf();
        if !path.exists() {
            return Err(ThermalError::NoSensorFound);
        }
        Ok(Self { path })
    }

    /// Auto-detect the best temperature sensor
    pub fn detect(source: SensorSource) -> Result<Self, ThermalError> {
        match source {
            SensorSource::Auto => {
                // Try hwmon first, fall back to thermal zones
                Self::detect_hwmon().or_else(|_| Self::detect_thermal_zone())
            }
            SensorSource::Hwmon => Self::detect_hwmon(),
            SensorSource::Thermal => Self::detect_thermal_zone(),
        }
    }

    /// Detect hwmon sensor (k10temp for AMD, coretemp for Intel)
    fn detect_hwmon() -> Result<Self, ThermalError> {
        let hwmon_base = Path::new("/sys/class/hwmon");

        if !hwmon_base.exists() {
            return Err(ThermalError::NoSensorFound);
        }

        // Priority order for hwmon drivers
        let preferred_drivers = ["k10temp", "coretemp", "zenpower"];

        for driver in &preferred_drivers {
            if let Some(path) = Self::find_hwmon_by_name(hwmon_base, driver) {
                log::info!("Detected hwmon sensor: {} at {:?}", driver, path);
                return Ok(Self { path });
            }
        }

        // Fall back to any hwmon with temp1_input
        for entry in fs::read_dir(hwmon_base).map_err(ThermalError::ReadError)? {
            let entry = entry.map_err(ThermalError::ReadError)?;
            let temp_path = entry.path().join("temp1_input");
            if temp_path.exists() {
                log::info!("Using fallback hwmon sensor: {:?}", temp_path);
                return Ok(Self { path: temp_path });
            }
        }

        Err(ThermalError::NoSensorFound)
    }

    fn find_hwmon_by_name(hwmon_base: &Path, driver_name: &str) -> Option<PathBuf> {
        let entries = fs::read_dir(hwmon_base).ok()?;

        for entry in entries.flatten() {
            let name_path = entry.path().join("name");
            if let Ok(name) = fs::read_to_string(&name_path) {
                if name.trim() == driver_name {
                    let temp_path = entry.path().join("temp1_input");
                    if temp_path.exists() {
                        return Some(temp_path);
                    }
                }
            }
        }
        None
    }

    /// Detect thermal zone sensor
    fn detect_thermal_zone() -> Result<Self, ThermalError> {
        let thermal_base = Path::new("/sys/class/thermal");

        if !thermal_base.exists() {
            return Err(ThermalError::NoSensorFound);
        }

        // Preferred zone types for CPU temperature
        let preferred_types = ["x86_pkg_temp", "cpu", "k10temp", "acpitz"];
        // Types to avoid
        let excluded_types = ["iwlwifi", "pch", "int340"];

        let mut zones: Vec<(PathBuf, String)> = Vec::new();

        for entry in fs::read_dir(thermal_base).map_err(ThermalError::ReadError)? {
            let entry = entry.map_err(ThermalError::ReadError)?;
            let path = entry.path();

            if !path.file_name().map_or(false, |n| {
                n.to_string_lossy().starts_with("thermal_zone")
            }) {
                continue;
            }

            let type_path = path.join("type");
            let temp_path = path.join("temp");

            if !temp_path.exists() {
                continue;
            }

            if let Ok(zone_type) = fs::read_to_string(&type_path) {
                let zone_type = zone_type.trim().to_lowercase();

                // Skip excluded types
                if excluded_types.iter().any(|t| zone_type.contains(t)) {
                    continue;
                }

                zones.push((temp_path, zone_type));
            }
        }

        // Sort by preference
        zones.sort_by(|a, b| {
            let a_pref = preferred_types
                .iter()
                .position(|t| a.1.contains(t))
                .unwrap_or(usize::MAX);
            let b_pref = preferred_types
                .iter()
                .position(|t| b.1.contains(t))
                .unwrap_or(usize::MAX);
            a_pref.cmp(&b_pref)
        });

        if let Some((path, zone_type)) = zones.into_iter().next() {
            log::info!("Detected thermal zone: {} at {:?}", zone_type, path);
            return Ok(Self { path });
        }

        Err(ThermalError::NoSensorFound)
    }

    /// Read current temperature in degrees Celsius
    pub fn read_temp(&self) -> Result<u32, ThermalError> {
        let content = fs::read_to_string(&self.path)?;
        let millidegrees: i64 = content.trim().parse().map_err(|_| ThermalError::ParseError)?;
        // sysfs reports temperature in millidegrees Celsius
        Ok((millidegrees / 1000) as u32)
    }

    /// Get the sensor path
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sensor_detection() {
        // This test will only pass on systems with temperature sensors
        if let Ok(sensor) = ThermalSensor::detect(SensorSource::Auto) {
            let temp = sensor.read_temp();
            assert!(temp.is_ok());
            let temp = temp.unwrap();
            // Sanity check: temp should be between 0 and 120
            assert!(temp < 120, "Temperature {} seems too high", temp);
        }
    }
}
