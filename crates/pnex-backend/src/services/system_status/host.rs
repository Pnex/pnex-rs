//! Host machine facts for the platform status page (Linux only): disk
//! usage (`statvfs`), memory (`/proc/meminfo`), load, uptime and machine
//! kind (Raspberry Pi / VM / container / bare metal). Every probe is
//! best-effort: a missing file yields `None`, never an error.

use std::path::Path;

/// Machine kind, resolved by the UI to `system-machine-<kind>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineKind {
    /// `raspberry_pi` | `vm` | `container` | `bare_metal` | `unknown`.
    pub kind: &'static str,
    /// Free-form model/vendor (verbatim).
    pub model: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskUsage {
    pub path: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryUsage {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

/// Disk usage of the filesystem holding `path` (`statvfs`).
#[cfg(unix)]
pub fn disk_usage(path: &str) -> Option<DiskUsage> {
    use std::ffi::CString;
    let c_path = CString::new(path).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c_path` is a valid NUL-terminated string and `stat` a
    // properly sized out-parameter.
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if rc != 0 {
        return None;
    }
    let frsize = stat.f_frsize as u64;
    Some(DiskUsage {
        path: path.to_string(),
        total_bytes: stat.f_blocks as u64 * frsize,
        available_bytes: stat.f_bavail as u64 * frsize,
    })
}

#[cfg(not(unix))]
pub fn disk_usage(_path: &str) -> Option<DiskUsage> {
    None
}

/// Parses `/proc/meminfo` content (kB values).
pub fn parse_meminfo(text: &str) -> Option<MemoryUsage> {
    let field = |name: &str| -> Option<u64> {
        text.lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .map(|kb| kb * 1024)
    };
    Some(MemoryUsage {
        total_bytes: field("MemTotal:")?,
        available_bytes: field("MemAvailable:").or_else(|| field("MemFree:"))?,
    })
}

pub fn memory() -> Option<MemoryUsage> {
    parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)
}

/// 1-minute load average.
pub fn load1() -> Option<f64> {
    std::fs::read_to_string("/proc/loadavg")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

pub fn uptime_secs() -> Option<f64> {
    std::fs::read_to_string("/proc/uptime")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn read_trimmed(path: &str) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    // device-tree strings are NUL-terminated.
    let text = raw.trim_matches(char::from(0)).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Classifies DMI vendor/product strings as a hypervisor, if any.
pub fn vm_vendor(vendor: &str, product: &str) -> Option<&'static str> {
    let haystack = format!("{vendor} {product}").to_lowercase();
    [
        ("qemu", "QEMU/KVM"),
        ("kvm", "QEMU/KVM"),
        ("vmware", "VMware"),
        ("virtualbox", "VirtualBox"),
        ("innotek", "VirtualBox"),
        ("hyper-v", "Hyper-V"),
        ("microsoft corporation virtual", "Hyper-V"),
        ("xen", "Xen"),
        ("amazon ec2", "AWS EC2"),
        ("google compute engine", "Google Compute Engine"),
        ("digitalocean", "DigitalOcean"),
        ("hetzner", "Hetzner Cloud"),
        ("openstack", "OpenStack"),
        ("parallels", "Parallels"),
    ]
    .iter()
    .find(|(needle, _)| haystack.contains(needle))
    .map(|(_, name)| *name)
}

fn in_container() -> bool {
    if Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists() {
        return true;
    }
    std::fs::read_to_string("/proc/1/cgroup")
        .map(|c| c.contains("docker") || c.contains("kubepods") || c.contains("containerd"))
        .unwrap_or(false)
}

/// Machine kind. A container reports its host when visible (device-tree /
/// DMI are usually readable from inside Docker), otherwise `container`.
pub fn machine_kind() -> MachineKind {
    if let Some(model) = read_trimmed("/proc/device-tree/model") {
        if model.to_lowercase().contains("raspberry pi") {
            return MachineKind {
                kind: "raspberry_pi",
                model: Some(model),
            };
        }
    }
    let vendor = read_trimmed("/sys/class/dmi/id/sys_vendor").unwrap_or_default();
    let product = read_trimmed("/sys/class/dmi/id/product_name").unwrap_or_default();
    if let Some(vm) = vm_vendor(&vendor, &product) {
        return MachineKind {
            kind: "vm",
            model: Some(vm.to_string()),
        };
    }
    if in_container() {
        return MachineKind {
            kind: "container",
            model: None,
        };
    }
    let model = format!("{vendor} {product}").trim().to_string();
    if model.is_empty() {
        MachineKind {
            kind: if cfg!(target_os = "linux") {
                "bare_metal"
            } else {
                "unknown"
            },
            model: read_trimmed("/proc/device-tree/model"),
        }
    } else {
        MachineKind {
            kind: "bare_metal",
            model: Some(model),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_parses_total_and_available() {
        let text = "MemTotal:        8000000 kB\nMemFree:  100 kB\nMemAvailable:    4000000 kB\n";
        let m = parse_meminfo(text).unwrap();
        assert_eq!(m.total_bytes, 8_000_000 * 1024);
        assert_eq!(m.available_bytes, 4_000_000 * 1024);
    }

    #[test]
    fn vm_vendors_are_detected() {
        assert_eq!(
            vm_vendor("QEMU", "Standard PC (Q35 + ICH9, 2009)"),
            Some("QEMU/KVM")
        );
        assert_eq!(vm_vendor("innotek GmbH", "VirtualBox"), Some("VirtualBox"));
        assert_eq!(vm_vendor("Dell Inc.", "XPS 15"), None);
    }
}
