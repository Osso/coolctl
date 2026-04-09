mod config;
mod cpufreq;
mod profile;
mod thermal;
mod throttle;
mod turbo;

use config::{Config, PowerProfile, ProfileThresholds};
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
    let mut config = Config::load_or_default(CONFIG_PATH);
    init_logging(&config);
    log::info!("coolctl starting");
    log_config(&config);
    setup_signal_handlers();
    let mut profile_watcher = ProfileWatcher::new();
    let profiles_enabled = log_profile_support(&config, &profile_watcher);
    let sensor = detect_sensor(&config)?;
    log::info!("Using sensor: {:?}", sensor.path());
    let mut cpufreq = CpuFreq::detect()?;
    log_cpufreq(&cpufreq);
    let turbo = detect_turbo();
    let mut throttle = Throttle::new(&config, cpufreq.min_freq(), cpufreq.max_freq());
    apply_current_profile(
        &config,
        profiles_enabled,
        &profile_watcher,
        &mut throttle,
        turbo.as_ref(),
    );
    run_loop(
        &mut config,
        profiles_enabled,
        &sensor,
        &mut profile_watcher,
        &mut throttle,
        &mut cpufreq,
        turbo.as_ref(),
    );
    restore_settings(&mut cpufreq, turbo.as_ref());
    Ok(())
}

fn log_config(config: &Config) {
    log::info!(
        "Config: throttle {}°C - {}°C, hysteresis {}°C",
        config.throttle_start,
        config.throttle_max,
        config.hysteresis
    );
}

fn log_profile_support(config: &Config, profile_watcher: &ProfileWatcher) -> bool {
    let profiles_enabled = config.enable_profiles && profile_watcher.is_available();
    if profiles_enabled {
        log::info!(
            "Profile switching enabled, current: {}",
            profile_watcher.current().name()
        );
    } else if config.enable_profiles {
        log::info!("Profile switching enabled but platform_profile not available");
    }
    profiles_enabled
}

fn detect_sensor(config: &Config) -> Result<ThermalSensor, Box<dyn std::error::Error>> {
    match &config.sensor {
        Some(path) => Ok(ThermalSensor::from_path(path)?),
        None => Ok(ThermalSensor::detect(config.sensor_source)?),
    }
}

fn log_cpufreq(cpufreq: &CpuFreq) {
    log::info!(
        "Detected {} CPUs, freq range: {} - {} MHz",
        cpufreq.cpu_count(),
        cpufreq.min_freq() / 1000,
        cpufreq.max_freq() / 1000
    );
}

fn detect_turbo() -> Option<TurboController> {
    match TurboController::detect() {
        Ok(controller) => Some(controller),
        Err(error) => {
            log::info!("Turbo boost control not available: {}", error);
            None
        }
    }
}

fn apply_current_profile(
    config: &Config,
    profiles_enabled: bool,
    profile_watcher: &ProfileWatcher,
    throttle: &mut Throttle,
    turbo: Option<&TurboController>,
) {
    if !profiles_enabled {
        return;
    }

    let current_profile = profile_watcher.current();
    let thresholds = config.thresholds_for(current_profile);
    throttle.update_thresholds(thresholds);
    apply_turbo_for_profile(turbo, current_profile);
    log_profile_thresholds("Profile", current_profile, thresholds);
}

fn poll_interval(config: &Config) -> Duration {
    Duration::from_millis(config.poll_interval_ms)
}

fn run_loop(
    config: &mut Config,
    profiles_enabled: bool,
    sensor: &ThermalSensor,
    profile_watcher: &mut ProfileWatcher,
    throttle: &mut Throttle,
    cpufreq: &mut CpuFreq,
    turbo: Option<&TurboController>,
) {
    let poll_interval = poll_interval(config);
    while !SHOULD_EXIT.load(Ordering::SeqCst) {
        if SHOULD_RELOAD.swap(false, Ordering::SeqCst) {
            reload_config(config, throttle, cpufreq, profiles_enabled, profile_watcher);
        }
        handle_profile_change(profiles_enabled, profile_watcher, config, throttle, turbo);
        handle_temperature(sensor, throttle, cpufreq);
        thread::sleep(poll_interval);
    }
}

fn reload_config(
    config: &mut Config,
    throttle: &mut Throttle,
    cpufreq: &CpuFreq,
    profiles_enabled: bool,
    profile_watcher: &ProfileWatcher,
) {
    log::info!("Reloading configuration");
    *config = Config::load_or_default(CONFIG_PATH);
    *throttle = Throttle::new(config, cpufreq.min_freq(), cpufreq.max_freq());
    if profiles_enabled {
        let thresholds = config.thresholds_for(profile_watcher.current());
        throttle.update_thresholds(thresholds);
    }
}

fn handle_profile_change(
    profiles_enabled: bool,
    profile_watcher: &mut ProfileWatcher,
    config: &Config,
    throttle: &mut Throttle,
    turbo: Option<&TurboController>,
) {
    if !profiles_enabled {
        return;
    }

    let Some(new_profile) = profile_watcher.poll() else {
        return;
    };
    let thresholds = config.thresholds_for(new_profile);
    throttle.update_thresholds(thresholds);
    apply_turbo_for_profile(turbo, new_profile);
    log_profile_thresholds("Profile changed to", new_profile, thresholds);
}

fn log_profile_thresholds(prefix: &str, profile: PowerProfile, thresholds: ProfileThresholds) {
    log::info!(
        "{} {}: throttle {}°C - {}°C",
        prefix,
        profile.name(),
        thresholds.throttle_start,
        thresholds.throttle_max
    );
}

fn handle_temperature(sensor: &ThermalSensor, throttle: &mut Throttle, cpufreq: &mut CpuFreq) {
    let temp = match sensor.read_temp() {
        Ok(temp) => temp,
        Err(error) => {
            log::error!("Failed to read temperature: {}", error);
            return;
        }
    };

    let Some(target_freq) = throttle.calculate(temp) else {
        log::debug!("Temp: {}°C (within hysteresis)", temp);
        return;
    };

    match cpufreq.set_max_freq(target_freq) {
        Ok(true) => log::info!("Temp: {}°C -> MaxFreq: {} MHz", temp, target_freq / 1000),
        Ok(false) => log::debug!("Temp: {}°C (no freq change needed)", temp),
        Err(error) => log::error!("Failed to set frequency: {}", error),
    }
}

fn restore_settings(cpufreq: &mut CpuFreq, turbo: Option<&TurboController>) {
    log::info!("Shutting down, restoring settings");
    if let Err(error) = cpufreq.restore_max() {
        log::error!("Failed to restore max frequency: {}", error);
    }
    if let Some(turbo) = turbo {
        if let Err(error) = turbo.restore() {
            log::error!("Failed to restore turbo state: {}", error);
        }
    }
    log::info!("coolctl stopped");
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
