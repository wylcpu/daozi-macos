use super::{device_kind, BatteryReading, BatterySource, Device};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::time::{Duration, Instant};

fn read_devices() -> Vec<Device> {
    let Ok(output) = Command::new("/usr/sbin/system_profiler")
        .args(["SPBluetoothDataType", "-json"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let Ok(root) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return Vec::new();
    };
    let Some(rows) = root["SPBluetoothDataType"].as_array() else {
        return Vec::new();
    };
    let now = Instant::now();
    let mut devices = Vec::new();
    for row in rows {
        for (group, connected) in [("device_connected", true), ("device_not_connected", false)] {
            let Some(items) = row[group].as_array() else {
                continue;
            };
            for item in items {
                for (name, details) in item.as_object().into_iter().flatten() {
                    let address = details["device_address"].as_str().unwrap_or_default();
                    let battery = details["device_batteryLevelMain"]
                        .as_str()
                        .and_then(|v| v.trim_end_matches('%').parse::<u8>().ok())
                        .filter(|v| *v <= 100);
                    let kind = device_kind(name, true, None);
                    let reading = battery.map(|percent| {
                        BatteryReading::new(percent, BatterySource::Macos, now, None)
                    });
                    devices.push(Device {
                        id: if address.is_empty() {
                            format!("mac:{name}")
                        } else {
                            format!("mac:{}", address.replace(':', "").to_ascii_uppercase())
                        },
                        name: name.clone(),
                        battery,
                        kind,
                        reading,
                        connected,
                    });
                }
            }
        }
    }
    devices
}

pub(super) fn run(tx: SyncSender<Vec<Device>>, stop: std::sync::Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        let devices = read_devices();
        if tx.try_send(devices).is_err() && stop.load(Ordering::Relaxed) {
            break;
        }
        for _ in 0..30 {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
