use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum TurboError {
    #[error("Failed to read turbo state: {0}")]
    ReadError(#[from] std::io::Error),
    #[error("Failed to write turbo state: {0}")]
    WriteError(String),
    #[error("Turbo boost control not available")]
    NotAvailable,
}

/// Turbo boost controller
pub struct TurboController {
    /// Path to turbo control file
    path: PathBuf,
    /// Whether the file uses inverse logic (1 = disabled)
    inverse: bool,
    /// Original state to restore on shutdown
    original_state: bool,
}

impl TurboController {
    /// Detect and create turbo controller
    pub fn detect() -> Result<Self, TurboError> {
        // Intel pstate: no_turbo file (1 = turbo disabled, inverse logic)
        let intel_path = Path::new("/sys/devices/system/cpu/intel_pstate/no_turbo");
        if intel_path.exists() {
            let original = Self::read_state(intel_path, true)?;
            log::info!(
                "Detected Intel turbo control, currently {}",
                if original { "enabled" } else { "disabled" }
            );
            return Ok(Self {
                path: intel_path.to_path_buf(),
                inverse: true,
                original_state: original,
            });
        }

        // Generic cpufreq boost (1 = turbo enabled)
        let boost_path = Path::new("/sys/devices/system/cpu/cpufreq/boost");
        if boost_path.exists() {
            let original = Self::read_state(boost_path, false)?;
            log::info!(
                "Detected cpufreq boost control, currently {}",
                if original { "enabled" } else { "disabled" }
            );
            return Ok(Self {
                path: boost_path.to_path_buf(),
                inverse: false,
                original_state: original,
            });
        }

        // AMD pstate in active mode - turbo controlled by driver
        let amd_status = Path::new("/sys/devices/system/cpu/amd_pstate/status");
        if amd_status.exists() {
            if let Ok(status) = fs::read_to_string(amd_status) {
                if status.trim() == "active" {
                    log::info!("AMD pstate active mode: turbo controlled by EPP, not available for manual control");
                    return Err(TurboError::NotAvailable);
                }
            }
        }

        Err(TurboError::NotAvailable)
    }

    fn read_state(path: &Path, inverse: bool) -> Result<bool, TurboError> {
        let content = fs::read_to_string(path)?;
        let value: u8 = content.trim().parse().map_err(|_| {
            TurboError::ReadError(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Failed to parse turbo state",
            ))
        })?;
        Ok((value == 1) ^ inverse)
    }

    /// Set turbo boost state
    pub fn set_enabled(&self, enabled: bool) -> Result<(), TurboError> {
        let value = if enabled ^ self.inverse { "1" } else { "0" };
        fs::write(&self.path, value)
            .map_err(|e| TurboError::WriteError(format!("{}: {}", self.path.display(), e)))?;
        log::info!(
            "Turbo boost {}",
            if enabled { "enabled" } else { "disabled" }
        );
        Ok(())
    }

    /// Restore original turbo state
    pub fn restore(&self) -> Result<(), TurboError> {
        self.set_enabled(self.original_state)?;
        log::info!(
            "Restored turbo boost to original state: {}",
            if self.original_state {
                "enabled"
            } else {
                "disabled"
            }
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_turbo_detection() {
        // This test will pass or fail depending on system
        match TurboController::detect() {
            Ok(ctrl) => {
                println!("Turbo control available at {:?}", ctrl.path);
            }
            Err(TurboError::NotAvailable) => {
                println!("Turbo control not available on this system");
            }
            Err(e) => {
                panic!("Unexpected error: {}", e);
            }
        }
    }
}
