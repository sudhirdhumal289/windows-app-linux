//! Linux (GNOME-first) desktop integration.

use std::process::Command;

/// Desktop scale in percent from GNOME's `monitors.xml` or `gsettings`.
#[must_use]
pub fn desktop_scale_percent() -> Option<u32> {
    if let Some(dirs) = directories::BaseDirs::new() {
        let monitors = dirs.config_dir().join("monitors.xml");
        if let Some(scale) = std::fs::read_to_string(monitors)
            .ok()
            .and_then(|xml| parse_scale_from_monitors_xml(&xml))
        {
            return Some(scale);
        }
    }
    if let Some(factor) = gsetting("text-scaling-factor").and_then(|s| s.parse::<f64>().ok()) {
        if factor > 1.05 {
            return Some((factor * 100.0).round() as u32);
        }
    }
    gsetting("scaling-factor")
        .and_then(|s| s.trim_start_matches("uint32").trim().parse::<u32>().ok())
        .filter(|f| *f > 1)
        .map(|f| f * 100)
}

fn gsetting(key: &str) -> Option<String> {
    let output = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", key])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Scale percentage of the primary (else first) logical monitor in GNOME `monitors.xml`.
#[must_use]
pub fn parse_scale_from_monitors_xml(xml: &str) -> Option<u32> {
    let mut first = None;
    for block in xml.split("<logicalmonitor>").skip(1) {
        let block = block.split("</logicalmonitor>").next().unwrap_or_default();
        let Some(scale) = block
            .split("<scale>")
            .nth(1)
            .and_then(|rest| rest.split("</scale>").next())
            .and_then(|s| s.trim().parse::<f64>().ok())
            .map(|f| (f * 100.0).round() as u32)
            .filter(|p| (100..=500).contains(p))
        else {
            continue;
        };
        if block.contains("<primary>yes</primary>") {
            return Some(scale);
        }
        first.get_or_insert(scale);
    }
    first
}

/// A USB device attached to this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbDevice {
    /// `vvvv:pppp` in lowercase hex.
    pub id: String,
    pub name: String,
    /// Whether this user may open the device (required to redirect it).
    pub accessible: bool,
}

/// Attached USB devices that can be redirected (hubs and root hubs are skipped).
#[must_use]
pub fn list_usb_devices() -> Vec<UsbDevice> {
    list_usb_devices_in(std::path::Path::new("/sys/bus/usb/devices"), |bus, dev| {
        let node = format!("/dev/bus/usb/{bus:03}/{dev:03}");
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(node)
            .is_ok()
    })
}

fn list_usb_devices_in(
    root: &std::path::Path,
    accessible: impl Fn(u32, u32) -> bool,
) -> Vec<UsbDevice> {
    let read = |dir: &std::path::Path, name: &str| {
        std::fs::read_to_string(dir.join(name))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut devices: Vec<UsbDevice> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let dir = entry.path();
            let vendor = read(&dir, "idVendor")?;
            let product = read(&dir, "idProduct")?;
            // Class 09 = hub.
            if read(&dir, "bDeviceClass").as_deref() == Some("09") {
                return None;
            }
            let id = format!("{vendor}:{product}").to_ascii_lowercase();
            let name = read(&dir, "product")
                .or_else(|| read(&dir, "manufacturer"))
                .unwrap_or_else(|| format!("USB device {id}"));
            let bus = read(&dir, "busnum").and_then(|s| s.parse().ok());
            let dev = read(&dir, "devnum").and_then(|s| s.parse().ok());
            let accessible = matches!((bus, dev), (Some(b), Some(d)) if accessible(b, d));
            Some(UsbDevice {
                id,
                name,
                accessible,
            })
        })
        .collect();
    devices.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    devices.dedup_by(|a, b| a.id == b.id);
    devices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_usb_devices_without_hubs() {
        let root = std::env::temp_dir().join(format!("wal-usb-{}", std::process::id()));
        let write = |dev: &str, files: &[(&str, &str)]| {
            let dir = root.join(dev);
            std::fs::create_dir_all(&dir).unwrap();
            for (name, value) in files {
                std::fs::write(dir.join(name), format!("{value}\n")).unwrap();
            }
        };
        write(
            "usb1",
            &[
                ("idVendor", "1d6b"),
                ("idProduct", "0002"),
                ("bDeviceClass", "09"),
            ],
        );
        write(
            "1-2",
            &[
                ("idVendor", "046D"),
                ("idProduct", "c52b"),
                ("bDeviceClass", "00"),
                ("product", "USB Receiver"),
                ("busnum", "1"),
                ("devnum", "5"),
            ],
        );
        write("1-2:1.0", &[("bInterfaceClass", "03")]);
        write("1-3", &[("idVendor", "abcd"), ("idProduct", "0001")]);

        let devices = list_usb_devices_in(&root, |bus, dev| (bus, dev) == (1, 5));
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(
            devices,
            vec![
                UsbDevice {
                    id: "046d:c52b".into(),
                    name: "USB Receiver".into(),
                    accessible: true,
                },
                UsbDevice {
                    id: "abcd:0001".into(),
                    name: "USB device abcd:0001".into(),
                    accessible: false,
                },
            ]
        );
    }

    #[test]
    fn parses_primary_monitor_scale() {
        let xml = r#"<monitors version="2"><configuration>
            <logicalmonitor><scale>2</scale></logicalmonitor>
            <logicalmonitor><x>0</x><scale>1.5</scale><primary>yes</primary></logicalmonitor>
        </configuration></monitors>"#;
        assert_eq!(parse_scale_from_monitors_xml(xml), Some(150));
    }

    #[test]
    fn falls_back_to_first_valid_monitor() {
        let xml = "<logicalmonitor><scale>9</scale></logicalmonitor>\
                   <logicalmonitor><scale>1.25</scale></logicalmonitor>";
        assert_eq!(parse_scale_from_monitors_xml(xml), Some(125));
        assert_eq!(parse_scale_from_monitors_xml("<monitors/>"), None);
    }
}
