# coolctl - CPU Thermal Throttle Daemon

## Overview

A minimal Linux daemon that adjusts CPU frequency based on temperature to maintain quiet fan operation.

## Goals

- Keep CPU temperature below threshold to prevent fan ramp-up
- Simple config-driven operation (no IPC, no CLI control)
- Single binary daemon
- Minimal dependencies

## Non-Goals

- Web UI
- TUI
- Runtime control via socket/IPC
- Fan control (EC handles this)

## Target System

- ThinkPad with AMD Ryzen AI 9 HX PRO 370
- Arch Linux
- `thinkpad_acpi` module for fan (EC-controlled)
- `amd-pstate-epp` CPU frequency driver

## Prerequisites

**1. Disable conflicting frequency controllers:**

```bash
sudo systemctl disable --now power-profiles-daemon
```

coolctl replaces power-profiles-daemon with thermal-aware throttling.

Other services to check (disable if active):
- `thermald` - Intel thermal daemon
- `tlp` - Laptop power management (may conflict)
- `auto-cpufreq` - Automatic CPU frequency scaling

**2. AMD systems: Switch amd-pstate to passive mode:**

```bash
# Check current mode
cat /sys/devices/system/cpu/amd_pstate/status

# Switch to passive (required for scaling_max_freq control)
echo passive | sudo tee /sys/devices/system/cpu/amd_pstate/status
```

Why: `amd-pstate-epp` (active mode) ignores scaling_max_freq writes.
The driver controls frequency internally based on EPP settings.
In `passive` mode, scaling_max_freq is respected like traditional cpufreq.

To make persistent, add kernel parameter: `amd_pstate=passive`

**3. Set platform profile (ThinkPad/laptops):**

```bash
# Check available profiles
cat /sys/firmware/acpi/platform_profile_choices

# Set to balanced or performance
echo balanced | sudo tee /sys/firmware/acpi/platform_profile
```

## Algorithm

Linear frequency scaling based on temperature:

```
           max_freq
              │
              ├────────┐
              │        │╲
              │        │ ╲
              │        │  ╲
              │        │   ╲
           min_freq    │    ╲───────
              │        │
              └────────┴─────────────→ Temp
                      55°C   72°C
                   (start)  (max)
```

- Below `throttle_start`: max_freq (no throttling)
- Between `throttle_start` and `throttle_max`: linear interpolation
- At/above `throttle_max`: min_freq
- Hysteresis: only change freq if temp moved ≥N degrees since last change

### Formula

```
if temp < throttle_start:
    target_freq = max_freq
elif temp >= throttle_max:
    target_freq = min_freq
else:
    ratio = (temp - throttle_start) / (throttle_max - throttle_start)
    target_freq = max_freq - ratio * (max_freq - min_freq)
```

## Configuration

File: `/etc/coolctl.toml`

```toml
# Temperature thresholds (°C) - used for "balanced" profile
throttle_start = 55   # Begin reducing frequency
throttle_max = 72     # Force minimum frequency

# Hysteresis to prevent oscillation
hysteresis = 2        # Degrees before changing freq

# Polling interval
poll_interval_ms = 1000

# Sensor selection (optional)
# sensor = "auto"                    # Auto-detect (default)
# sensor = "/sys/class/hwmon/hwmon3/temp1_input"  # Explicit path
# sensor_source = "auto"             # auto|hwmon|thermal

# Frequency limits (optional, auto-detected from CPU)
# min_freq = 400000    # kHz
# max_freq = 5100000   # kHz

# Logging
# log_level = "info"   # error|warn|info|debug
# log_file = "/var/log/coolctl.log"  # Optional file logging

# Power profile integration
enable_profiles = true

[profile_silent]
throttle_start = 40
throttle_max = 50

[profile_performance]
throttle_start = 75
throttle_max = 90
```

## Power Profiles

coolctl integrates with the Linux `platform_profile` interface to provide
automatic throttling adjustment based on system power profile.

### Profile Mapping

| Platform Profile | coolctl Profile | Behavior |
|-----------------|-----------------|----------|
| `low-power`     | silent          | Aggressive throttling for quiet fans, turbo disabled |
| `balanced`      | balanced        | Default thresholds from config |
| `performance`   | performance     | Minimal throttling, allow higher temps |

### Turbo Boost Control

In silent profile, turbo boost is automatically disabled to further reduce heat output.
Turbo is re-enabled when switching to balanced or performance profiles.

Supported turbo control interfaces:
- Intel pstate: `/sys/devices/system/cpu/intel_pstate/no_turbo`
- Generic cpufreq: `/sys/devices/system/cpu/cpufreq/boost`
- AMD pstate (passive mode): Uses cpufreq boost interface

Note: On `amd-pstate-epp` (active mode), turbo is controlled by the driver and
cannot be manually toggled.

### Profile Interface

Profile changes are detected by polling `/sys/firmware/acpi/platform_profile`.

To change profiles:
```bash
# Using powerprofilesctl (if available)
powerprofilesctl set power-saver  # → silent
powerprofilesctl set balanced     # → balanced
powerprofilesctl set performance  # → performance

# Direct sysfs write
echo low-power | sudo tee /sys/firmware/acpi/platform_profile
```

### Disabling Profile Support

Set `enable_profiles = false` in config to use fixed thresholds regardless
of platform profile.

### Measured Behavior (stress-ng, 60s per profile)

| Profile | Thresholds | Steady Temp | Fan (RPM) | Freq |
|---------|------------|-------------|-----------|------|
| Performance | 75-90°C | 74-75°C | 5300 | 5.1 GHz (full) |
| Balanced | 55-72°C | 65-66°C | 3800-4000 | 2.2-3.5 GHz |
| Silent | 45-60°C | 53-54°C | 4100* | 2.1-2.7 GHz |

*Fan RPM during silent test was elevated from prior performance test spinup.
Fan hysteresis means it takes time to spin down after high temps.

## Sensor Detection

Priority order:
1. Explicit `sensor` path in config
2. HWMon sensors (prefer `k10temp`, `coretemp`)
3. Thermal zones (`/sys/class/thermal/thermal_zone*/temp`)

### HWMon Detection

Scan `/sys/class/hwmon/*/name` for:
- `k10temp` (AMD)
- `coretemp` (Intel)
- Use `temp1_input` from matching device

### Thermal Zone Detection

Scan `/sys/class/thermal/thermal_zone*/type` for:
- Types containing "cpu", "x86_pkg", "k10temp"
- Exclude: "acpitz", "iwlwifi", "pch"

## CPU Frequency Control

Write to: `/sys/devices/system/cpu/cpu*/cpufreq/scaling_max_freq`

### Detection

- `cpuinfo_min_freq`: Hardware minimum
- `cpuinfo_max_freq`: Hardware maximum
- Apply to all CPUs (scaling_max_freq)

### Write Filtering

Only write to sysfs if:
- Frequency changed by >5% from last write, OR
- Temperature crossed a threshold boundary

## Signal Handling

| Signal | Action |
|--------|--------|
| SIGTERM | Graceful shutdown, restore max_freq |
| SIGINT | Graceful shutdown, restore max_freq |
| SIGHUP | Reload configuration |

## Logging

Levels: error, warn, info, debug

- `error`: Failed to read temp, failed to write freq
- `warn`: Sensor fallback, freq clamped
- `info`: Startup, config loaded, freq changes
- `debug`: Every poll cycle

## Exit Behavior

On shutdown:
1. Restore `scaling_max_freq` to `cpuinfo_max_freq` for all CPUs
2. Restore turbo boost to original state (if it was modified)
3. Clean exit

## File Structure

```
~/Projects/coolctl/
├── Cargo.toml
├── SPEC.md              # This file
├── src/
│   ├── main.rs          # Entry point, signal handling, main loop
│   ├── config.rs        # Config file parsing, profile definitions
│   ├── thermal.rs       # Temperature sensor reading
│   ├── cpufreq.rs       # CPU frequency control
│   ├── profile.rs       # Platform profile watcher
│   ├── throttle.rs      # Throttling algorithm
│   └── turbo.rs         # Turbo boost control
```

## Dependencies

```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
toml = "0.8"
log = "0.4"
env_logger = "0.11"
nix = { version = "0.29", features = ["signal", "fs"] }
```

## Systemd Unit

```ini
[Unit]
Description=CPU Thermal Throttle Daemon
After=local-fs.target

[Service]
Type=simple
ExecStart=/usr/local/bin/coolctl
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

## Future Considerations

- Config file watching (inotify) instead of SIGHUP
- Multiple sensor averaging
- Per-CPU frequency control
- D-Bus interface for desktop integration
