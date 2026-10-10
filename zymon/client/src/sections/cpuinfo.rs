//! zymon extra: `key: value` lines describing the hardware. Re-read every cycle on purpose (cheap, and a
//! swapped SD card reports correctly). The `Serial` line is never emitted.

use super::{Ctx, Res};
use crate::sys::common;
use zymon_core::proto::SectionWriter;

#[derive(Default, Debug, PartialEq)]
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub struct Parsed {
    pub model: Option<String>,
    pub model_name: Option<String>,
    pub implementer: Option<String>,
    pub part: Option<String>,
    pub arch: Option<String>,
    pub features: Option<String>,
    pub cores: usize,
    pub bogomips: Option<String>,
    pub hardware: Option<String>,
    pub revision: Option<String>,
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn parse(text: &str) -> Parsed {
    let mut p = Parsed::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        if v.is_empty() && k != "processor" {
            continue;
        }
        let slot = match k.as_str() {
            "processor" => {
                p.cores += 1;
                continue;
            }
            "model" => &mut p.model,
            "model name" => &mut p.model_name,
            "cpu implementer" => &mut p.implementer,
            "cpu part" => &mut p.part,
            "cpu architecture" => &mut p.arch,
            "features" | "flags" => &mut p.features,
            "bogomips" => &mut p.bogomips,
            "hardware" => &mut p.hardware,
            "revision" => &mut p.revision,
            _ => continue, // includes "serial": never reported
        };
        if slot.is_none() {
            *slot = Some(v.to_string());
        }
    }
    p
}

/// ARM implementer 0x41 part numbers -> core names.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn decode_arm(implementer: &str, part: &str) -> Option<String> {
    if !implementer.eq_ignore_ascii_case("0x41") {
        return None;
    }
    let n = match part.to_ascii_lowercase().as_str() {
        "0xb76" => "ARM1176",
        "0xc07" => "Cortex-A7",
        "0xc08" => "Cortex-A8",
        "0xc09" => "Cortex-A9",
        "0xc0f" => "Cortex-A15",
        "0xd03" => "Cortex-A53",
        "0xd04" => "Cortex-A35",
        "0xd05" => "Cortex-A55",
        "0xd07" => "Cortex-A57",
        "0xd08" => "Cortex-A72",
        "0xd09" => "Cortex-A73",
        "0xd0a" => "Cortex-A75",
        "0xd0b" => "Cortex-A76",
        "0xd0c" => "Neoverse-N1",
        "0xd41" => "Cortex-A78",
        "0xd44" => "Cortex-X1",
        _ => return None,
    };
    Some(n.to_string())
}

/// Ordered key/value pairs; keys with no data are left out.
#[cfg(not(target_os = "macos"))]
pub fn gather() -> Vec<(&'static str, String)> {
    let rd = |p: &str| {
        std::fs::read(p)
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    };
    let p = parse(&rd("/proc/cpuinfo").unwrap_or_default());
    let dt_model =
        rd("/proc/device-tree/model").map(|s| s.trim_matches(['\0', '\n', ' ']).to_string());
    let mut v: Vec<(&'static str, String)> = Vec::new();
    let mut put = |k: &'static str, val: Option<String>| {
        if let Some(x) = val.filter(|x| !x.is_empty()) {
            v.push((k, x));
        }
    };
    put("model", p.model.clone().or(dt_model));
    put(
        "cpu_model",
        p.model_name
            .clone()
            .or_else(|| decode_arm(p.implementer.as_deref()?, p.part.as_deref()?)),
    );
    put("cpu_arch", p.arch.clone());
    put("features", p.features.clone());
    put("cores", (p.cores > 0).then(|| p.cores.to_string()));
    put("bogomips", p.bogomips.clone());
    // device-tree "compatible" lists most specific first; the last entry is the SoC
    put(
        "hardware",
        p.hardware.clone().or_else(|| {
            rd("/proc/device-tree/compatible").and_then(|s| {
                s.split('\0')
                    .filter(|e| !e.is_empty())
                    .next_back()
                    .map(str::to_string)
            })
        }),
    );
    put("revision", p.revision.clone());
    put("arch", common::uname().ok().map(|u| u.machine));
    put(
        "cpu_mhz_max",
        rd("/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq")
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(|khz| (khz / 1000).to_string()),
    );
    put(
        "throttled",
        rd("/sys/devices/platform/soc/soc:firmware/get_throttled").map(|s| {
            let t = s.trim();
            if t.starts_with("0x") {
                t.to_string()
            } else {
                format!("0x{t}")
            }
        }),
    );
    v
}

#[cfg(target_os = "macos")]
pub fn gather() -> Vec<(&'static str, String)> {
    use crate::sys::macos::{sysctl_string, sysctl_u64};
    let mut v: Vec<(&'static str, String)> = Vec::new();
    if let Some(m) = sysctl_string("hw.model") {
        v.push(("model", m));
    }
    if let Some(m) = sysctl_string("machdep.cpu.brand_string") {
        v.push(("cpu_model", m));
    }
    v.push((
        "cores",
        sysctl_u64("hw.ncpu")
            .unwrap_or(common::nproc() as u64)
            .to_string(),
    ));
    if let Some(m) = sysctl_string("hw.machine") {
        v.push(("arch", m));
    }
    if let Some(hz) = sysctl_u64("hw.cpufrequency_max") {
        v.push(("cpu_mhz_max", (hz / 1_000_000).to_string()));
    }
    v
}

pub fn cpuinfo(_ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    for (k, v) in gather() {
        w.line(format!("{k}: {v}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PI_ZERO: &str = "processor\t: 0\nmodel name\t: ARMv6-compatible processor rev 7 (v6l)\nBogoMIPS\t: 697.95\nFeatures\t: half thumb fastmult vfp edsp java tls\nCPU implementer\t: 0x41\nCPU architecture: 7\nCPU variant\t: 0x0\nCPU part\t: 0xb76\nCPU revision\t: 7\n\nHardware\t: BCM2835\nRevision\t: 9000c1\nSerial\t\t: 00000000deadbeef\nModel\t\t: Raspberry Pi Zero W Rev 1.1\n";
    const PI4: &str = "processor\t: 0\nBogoMIPS\t: 108.00\nFeatures\t: fp asimd evtstrm crc32 cpuid\nCPU implementer\t: 0x41\nCPU architecture: 8\nCPU variant\t: 0x0\nCPU part\t: 0xd08\nCPU revision\t: 3\n\nprocessor\t: 1\nBogoMIPS\t: 108.00\n\nHardware\t: BCM2835\nRevision\t: c03111\nSerial\t\t: 10000000cafe\nModel\t\t: Raspberry Pi 4 Model B Rev 1.1\n";

    #[test]
    fn pi_zero() {
        let p = parse(PI_ZERO);
        assert_eq!(p.model.as_deref(), Some("Raspberry Pi Zero W Rev 1.1"));
        assert_eq!(
            p.model_name.as_deref(),
            Some("ARMv6-compatible processor rev 7 (v6l)")
        );
        assert_eq!(p.arch.as_deref(), Some("7"));
        assert_eq!(p.cores, 1);
        assert_eq!(p.bogomips.as_deref(), Some("697.95"));
        assert_eq!(p.hardware.as_deref(), Some("BCM2835"));
        assert_eq!(p.revision.as_deref(), Some("9000c1"));
    }

    #[test]
    fn pi4_decodes_core_and_counts_cores() {
        let p = parse(PI4);
        assert_eq!(p.cores, 2);
        assert!(p.model_name.is_none());
        assert_eq!(
            decode_arm(
                p.implementer.as_deref().unwrap(),
                p.part.as_deref().unwrap()
            )
            .as_deref(),
            Some("Cortex-A72")
        );
        assert_eq!(p.revision.as_deref(), Some("c03111"));
    }

    #[test]
    fn serial_is_never_kept() {
        let dump = format!("{:?}", parse(PI_ZERO));
        assert!(!dump.contains("deadbeef"));
    }
}
