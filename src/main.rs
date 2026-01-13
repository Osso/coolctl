mod config;
mod cpufreq;
mod profile;
mod thermal;
mod throttle;
mod turbo;

use config::{Config, PowerProfile};
use cpufreq::CpuFreq;
use nix::libc;
use nix::sys::signal::{self, SigHandler, Signal};
use profile::ProfileWatcher;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use thermal::ThermalSensor;
use throttle::Throttle;
use turbo::TurboController;

const CONFIG_PATH: &str = "/etc/coolctl.toml";

static SHOULD_EXIT: AtomicBool = AtomicBool::new(false);
static SHOULD_RELOAD: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_sigterm(_: libc::c_int) {
    SHOULD_EXIT.store(true, Ordering::SeqCst);
}

extern "C" fn handle_sighup(_: libc::c_int) {
    SHOULD_RELOAD.store(true, Ordering::SeqCst);
}

fn setup_signal_handlers() {
    unsafe {
        signal::signal(Signal::SIGTERM, SigHandler::Handler(handle_sigterm))
            .expect("Failed to set SIGTERM handler");
        signal::signal(Signal::SIGINT, SigHandler::Handler(handle_sigterm))
            .expect("Failed to set SIGINT handler");
        signal::signal(Signal::SIGHUP, SigHandler::Handler(handle_sighup))
            .expect("Failed to set SIGHUP handler");
    }
}

fn init_logging(config: &Config) {
    let level = match config.log_level.to_lowercase().as_str() {
        "error" => log::LevelFilter::Error,
        "warn" => log::LevelFilter::Warn,
        "debug" => log::LevelFilter::Debug,
        "trace" => log::LevelFilter::Trace,
        _ => log::LevelFilter::Info,
    };

    env_logger::Builder::new()
        .filter_level(level)
        .format_timestamp_secs()
        .init();
}

/// Apply turbo boost state based on profile (disabled in silent mode)
fn apply_turbo_for_profile(turbo: Option<&TurboController>, profile: PowerProfile) {
    let Some(turbo) = turbo else { return };

    let should_enable = profile != PowerProfile::Silent;
    if let Err(e) = turbo.set_enabled(should_enable) {
        log::error!("Failed to set turbo state: {}", e);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Load config
    let mut config = Config::load_or_default(CONFIG_PATH);
    init_logging(&config);

    log::info!("coolctl starting");
    log::info!(
        "Config: throttle {}°C - {}°C, hysteresis {}°C",
        config.throttle_start,
        config.throttle_max,
        config.hysteresis
    );

    // Setup signal handlers
    setup_signal_handlers();

    // Setup profile watcher
    let mut profile_watcher = ProfileWatcher::new();
    let profiles_enabled = config.enable_profiles && profile_watcher.is_available();
    if profiles_enabled {
        log::info!(
            "Profile switching enabled, current: {}",
            profile_watcher.current().name()
        );
    } else if config.enable_profiles {
        log::info!("Profile switching enabled but platform_profile not available");
    }

    // Detect temperature sensor
    let sensor = if let Some(ref path) = config.sensor {
        ThermalSensor::from_path(path)?
    } else {
        ThermalSensor::detect(config.sensor_source)?
    };
    log::info!("Using sensor: {:?}", sensor.path());

    // Detect CPU frequency control
    let mut cpufreq = CpuFreq::detect()?;
    log::info!(
        "Detected {} CPUs, freq range: {} - {} MHz",
        cpufreq.cpu_count(),
        cpufreq.min_freq() / 1000,
        cpufreq.max_freq() / 1000
    );

    // Detect turbo boost control (optional)
    let turbo = match TurboController::detect() {
        Ok(ctrl) => Some(ctrl),
        Err(e) => {
            log::info!("Turbo boost control not available: {}", e);
            None
        }
    };

    // Initialize throttle controller
    let mut throttle = Throttle::new(&config, cpufreq.min_freq(), cpufreq.max_freq());

    // Apply initial profile thresholds if profiles enabled
    if profiles_enabled {
        let current_profile = profile_watcher.current();
        let thresholds = config.thresholds_for(current_profile);
        throttle.update_thresholds(thresholds);
        apply_turbo_for_profile(turbo.as_ref(), current_profile);
        log::info!(
            "Profile {}: throttle {}°C - {}°C",
            current_profile.name(),
            thresholds.throttle_start,
            thresholds.throttle_max
        );
    }

    let poll_interval = Duration::from_millis(config.poll_interval_ms);

    // Main loop
    while !SHOULD_EXIT.load(Ordering::SeqCst) {
        // Check for config reload
        if SHOULD_RELOAD.swap(false, Ordering::SeqCst) {
            log::info!("Reloading configuration");
            config = Config::load_or_default(CONFIG_PATH);
            throttle = Throttle::new(&config, cpufreq.min_freq(), cpufreq.max_freq());
            // Re-apply profile thresholds after reload
            if profiles_enabled {
                let thresholds = config.thresholds_for(profile_watcher.current());
                throttle.update_thresholds(thresholds);
            }
        }

        // Check for profile changes
        if profiles_enabled {
            if let Some(new_profile) = profile_watcher.poll() {
                let thresholds = config.thresholds_for(new_profile);
                throttle.update_thresholds(thresholds);
                apply_turbo_for_profile(turbo.as_ref(), new_profile);
                log::info!(
                    "Profile changed to {}: throttle {}°C - {}°C",
                    new_profile.name(),
                    thresholds.throttle_start,
                    thresholds.throttle_max
                );
            }
        }

        // Read temperature
        match sensor.read_temp() {
            Ok(temp) => {
                // Calculate target frequency
                if let Some(target_freq) = throttle.calculate(temp) {
                    match cpufreq.set_max_freq(target_freq) {
                        Ok(true) => {
                            log::info!(
                                "Temp: {}°C -> MaxFreq: {} MHz",
                                temp,
                                target_freq / 1000
                            );
                        }
                        Ok(false) => {
                            // Filtered, no change needed
                            log::debug!("Temp: {}°C (no freq change needed)", temp);
                        }
                        Err(e) => {
                            log::error!("Failed to set frequency: {}", e);
                        }
                    }
                } else {
                    log::debug!("Temp: {}°C (within hysteresis)", temp);
                }
            }
            Err(e) => {
                log::error!("Failed to read temperature: {}", e);
            }
        }

        thread::sleep(poll_interval);
    }

    // Graceful shutdown: restore max frequency and turbo state
    log::info!("Shutting down, restoring settings");
    if let Err(e) = cpufreq.restore_max() {
        log::error!("Failed to restore max frequency: {}", e);
    }
    if let Some(ref turbo) = turbo {
        if let Err(e) = turbo.restore() {
            log::error!("Failed to restore turbo state: {}", e);
        }
    }

    log::info!("coolctl stopped");
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
