use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

const CPUFREQ_BASE: &str = "/sys/devices/system/cpu";

#[derive(Error, Debug)]
pub enum CpuFreqError {
    #[error("Failed to read CPU frequency info: {0}")]
    ReadError(#[from] std::io::Error),
    #[error("Failed to parse frequency value")]
    ParseError,
    #[error("No CPUs found")]
    NoCpusFound,
    #[error("Failed to write frequency: {0}")]
    WriteError(String),
}

/// CPU frequency controller
pub struct CpuFreq {
    /// Paths to scaling_max_freq for each CPU
    cpu_paths: Vec<PathBuf>,
    /// Hardware minimum frequency (kHz)
    min_freq: u64,
    /// Hardware maximum frequency (kHz)
    max_freq: u64,
    /// Last written frequency (for filtering writes)
    last_written: Option<u64>,
}

impl CpuFreq {
    /// Detect CPUs and their frequency limits
    pub fn detect() -> Result<Self, CpuFreqError> {
        let base = Path::new(CPUFREQ_BASE);
        let mut cpu_paths = Vec::new();
        let mut min_freq = u64::MAX;
        let mut max_freq = 0u64;

        for entry in fs::read_dir(base)? {
            let entry = entry?;
            let Some(cpu_info) = Self::detect_cpu(entry.path())? else {
                continue;
            };
            min_freq = min_freq.min(cpu_info.min_freq);
            max_freq = max_freq.max(cpu_info.max_freq);
            cpu_paths.push(cpu_info.scaling_max);
        }

        if cpu_paths.is_empty() {
            return Err(CpuFreqError::NoCpusFound);
        }

        cpu_paths.sort();
        log_detected_cpus(cpu_paths.len(), min_freq, max_freq);
        Ok(Self {
            cpu_paths,
            min_freq,
            max_freq,
            last_written: None,
        })
    }

    fn detect_cpu(path: PathBuf) -> Result<Option<CpuInfo>, CpuFreqError> {
        if !is_cpu_dir(&path) {
            return Ok(None);
        }

        let cpufreq_dir = path.join("cpufreq");
        let scaling_max = cpufreq_dir.join("scaling_max_freq");
        if !cpufreq_dir.exists() || !scaling_max.exists() {
            return Ok(None);
        }

        Ok(Some(CpuInfo {
            scaling_max,
            min_freq: Self::read_freq_file(&cpufreq_dir.join("cpuinfo_min_freq"))?,
            max_freq: Self::cpu_max_freq(&cpufreq_dir)?,
        }))
    }

    fn cpu_max_freq(cpufreq_dir: &Path) -> Result<u64, CpuFreqError> {
        let amd_max_path = cpufreq_dir.join("amd_pstate_max_freq");
        if amd_max_path.exists() {
            return Self::read_freq_file(&amd_max_path);
        }

        Self::read_freq_file(&cpufreq_dir.join("cpuinfo_max_freq"))
    }

    fn read_freq_file(path: &Path) -> Result<u64, CpuFreqError> {
        let content = fs::read_to_string(path)?;
        content.trim().parse().map_err(|_| CpuFreqError::ParseError)
    }

    /// Get hardware minimum frequency (kHz)
    pub fn min_freq(&self) -> u64 {
        self.min_freq
    }

    /// Get hardware maximum frequency (kHz)
    pub fn max_freq(&self) -> u64 {
        self.max_freq
    }

    /// Get number of CPUs
    pub fn cpu_count(&self) -> usize {
        self.cpu_paths.len()
    }

    /// Set maximum frequency for all CPUs (kHz)
    /// Returns true if actually written, false if filtered
    pub fn set_max_freq(&mut self, freq: u64) -> Result<bool, CpuFreqError> {
        // Clamp to valid range
        let freq = freq.clamp(self.min_freq, self.max_freq);

        // Filter: only write if changed by more than 5%
        if let Some(last) = self.last_written {
            let threshold = self.max_freq / 20; // 5%
            if (freq as i64 - last as i64).unsigned_abs() < threshold {
                return Ok(false);
            }
        }

        let freq_str = freq.to_string();
        for path in &self.cpu_paths {
            fs::write(path, &freq_str)
                .map_err(|e| CpuFreqError::WriteError(format!("{}: {}", path.display(), e)))?;
        }

        log::debug!("Set max frequency to {} kHz", freq);
        self.last_written = Some(freq);
        Ok(true)
    }

    /// Restore maximum frequency to hardware max
    pub fn restore_max(&mut self) -> Result<(), CpuFreqError> {
        let max = self.max_freq;
        self.last_written = None; // Force write
        self.set_max_freq(max)?;
        log::info!("Restored max frequency to {} kHz", max);
        Ok(())
    }
}

struct CpuInfo {
    scaling_max: PathBuf,
    min_freq: u64,
    max_freq: u64,
}

fn is_cpu_dir(path: &Path) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    let name = name.to_string_lossy();
    name.starts_with("cpu") && name[3..].chars().all(|c| c.is_ascii_digit())
}

fn log_detected_cpus(cpu_count: usize, min_freq: u64, max_freq: u64) {
    log::info!(
        "Detected {} CPUs, freq range: {} - {} kHz",
        cpu_count,
        min_freq,
        max_freq
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_detection() {
        // This test will only pass on Linux systems with cpufreq
        if let Ok(cpufreq) = CpuFreq::detect() {
            assert!(cpufreq.cpu_count() > 0);
            assert!(cpufreq.min_freq() < cpufreq.max_freq());
        }
    }
}
