//! zymon extra: CPU temperature. The `[cputemp]` client section lists every sensor; the `cputemp` status column
//! is evaluated client-side (like `docker.must_run`) against `[cputemp] warn_c` / `panic_c`.
//! Linux only: macOS exposes no CPU temperature without root.

use super::{Ctx, Res, unsupported};
use crate::config::Config;
use crate::sys::common;
use zymon_core::proto::{self, Color, SectionWriter};

#[derive(Debug, Clone, PartialEq)]
pub struct Zone {
    pub name: String,
    pub celsius: f64,
}

/// sysfs reports millidegrees.
pub fn parse_milli(s: &str) -> Option<f64> {
    let v: f64 = s.trim().parse().ok()?;
    // -274 C is "no sensor" on several drivers; ignore garbage
    (v > -200_000.0 && v < 300_000.0).then_some(v / 1000.0)
}

/// How well a sensor name describes the processor: `Some(0)` for the package/die sensors (`x86_pkg_temp`,
/// `coretemp`, AMD `k10temp`/`Tctl`), `Some(1)` for SoC-style names (`cpu-thermal`, `soc`, `TCPU`), `Some(2)` for
/// `acpitz`, which on many x86 boards is a fixed bogus value (91 C) and only counts when nothing better exists.
/// `None` for batteries, wifi chips, disks, PCH and the like.
pub fn cpu_rank(name: &str) -> Option<u8> {
    let n = name.to_ascii_lowercase();
    let has = |keys: &[&str]| keys.iter().any(|k| n.contains(k));
    if has(&["pkg", "package", "coretemp", "k10temp", "tctl"]) {
        Some(0)
    } else if has(&["cpu", "soc", "core"]) {
        Some(1)
    } else if has(&["acpitz"]) {
        Some(2)
    } else {
        None
    }
}

/// Zones that describe the processor rather than a battery, wifi chip or disk.
pub fn is_cpu_zone(name: &str) -> bool {
    cpu_rank(name).is_some()
}

/// The sensors the status is computed from: every sensor of the best (lowest) rank present, so on a dual-socket
/// x86 both `x86_pkg_temp` zones count while `acpitz` is ignored; all sensors when none looks like a CPU.
pub fn cpu_sensors(zones: &[Zone]) -> Vec<&Zone> {
    let best = zones.iter().filter_map(|z| cpu_rank(&z.name)).min();
    zones.iter().filter(|z| cpu_rank(&z.name) == best).collect()
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
/// Make names unique and wire-friendly: `cpu-thermal`, `cpu-thermal_1`.
fn unique(zones: Vec<(String, f64)>) -> Vec<Zone> {
    let mut out: Vec<Zone> = Vec::new();
    for (name, celsius) in zones {
        let base: String = name.split_whitespace().collect::<Vec<_>>().join("_");
        let mut n = base.clone();
        let mut i = 1;
        while out.iter().any(|z| z.name == n) {
            n = format!("{base}_{i}");
            i += 1;
        }
        out.push(Zone { name: n, celsius });
    }
    out
}

/// thermal zones first, hwmon as the fallback (`temp1_input` of each chip).
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn read_zones_from(sys: &str) -> Vec<Zone> {
    let rd = |p: &str| std::fs::read_to_string(p).ok();
    let mut found: Vec<(String, f64)> = Vec::new();
    let mut dirs = list_dirs(&format!("{sys}/class/thermal"), "thermal_zone");
    dirs.sort_by_key(|d| {
        d.trim_start_matches(|c: char| !c.is_ascii_digit())
            .parse::<u32>()
            .unwrap_or(0)
    });
    for d in dirs {
        let base = format!("{sys}/class/thermal/{d}");
        if let Some(t) = rd(&format!("{base}/temp")).and_then(|s| parse_milli(&s)) {
            let ty = rd(&format!("{base}/type"))
                .map(|s| s.trim().to_string())
                .unwrap_or(d.clone());
            found.push((ty, t));
        }
    }
    if found.is_empty() {
        let mut dirs = list_dirs(&format!("{sys}/class/hwmon"), "hwmon");
        dirs.sort();
        for d in dirs {
            let base = format!("{sys}/class/hwmon/{d}");
            if let Some(t) = rd(&format!("{base}/temp1_input")).and_then(|s| parse_milli(&s)) {
                let name = rd(&format!("{base}/name"))
                    .map(|s| s.trim().to_string())
                    .unwrap_or(d.clone());
                found.push((name, t));
            }
        }
    }
    unique(found)
}

pub fn read_zones() -> Vec<Zone> {
    if cfg!(target_os = "linux") {
        read_zones_from("/sys")
    } else {
        Vec::new()
    }
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn list_dirs(root: &str, prefix: &str) -> Vec<String> {
    std::fs::read_dir(root)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with(prefix))
                .collect()
        })
        .unwrap_or_default()
}

pub fn cputemp(_ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    if !cfg!(target_os = "linux") {
        return unsupported();
    }
    for z in read_zones() {
        w.line(format!("{}: {:.1}", z.name, z.celsius));
    }
    Ok(())
}

fn color_for(c: f64, warn: f64, panic: f64) -> Color {
    if c >= panic {
        Color::Red
    } else if c >= warn {
        Color::Yellow
    } else {
        Color::Green
    }
}

/// Raspberry Pi firmware throttling bits that are active right now (bits 0-3 of get_throttled).
pub fn throttle_now(flags: u32) -> Vec<&'static str> {
    [
        (0x1, "under-voltage"),
        (0x2, "arm frequency capped"),
        (0x4, "throttled"),
        (0x8, "soft temperature limit"),
    ]
    .iter()
    .filter(|(b, _)| flags & b != 0)
    .map(|(_, n)| *n)
    .collect()
}

fn read_throttled() -> Option<u32> {
    let s = std::fs::read_to_string("/sys/devices/platform/soc/soc:firmware/get_throttled").ok()?;
    let t = s.trim();
    u32::from_str_radix(t.trim_start_matches("0x"), 16).ok()
}

/// The status keeps the layout of the original hand-made `cputemp` test so the existing server-side graph keeps
/// working: the first line is just the date, the body (a blank line before and between the lines) is
///
/// ```text
///
/// CPU temperature 42.4 C
///
/// temp : 42.4
///
/// Yellow from 70 C, red from 80 C
/// ```
///
/// where `temp : <value>` is the line the server's NCV handler turns into the `temp` data set. The value and the
/// colour follow the hottest CPU sensor (see [`cpu_sensors`]); with more than one CPU sensor (dual-socket x86) a
/// `tempN : <value>` line per sensor follows, in sensor order. The colour is raised to yellow while the Pi firmware
/// reports throttling. The other sensors stay in the `[cputemp]` client section, out of the NCV body.
pub fn status_message(cfg: &Config, zones: &[Zone], throttled: Option<u32>) -> Option<Vec<u8>> {
    let c = &cfg.cputemp;
    let sensors = cpu_sensors(zones);
    let hottest = sensors.iter().max_by(|a, b| {
        a.celsius
            .partial_cmp(&b.celsius)
            .unwrap_or(std::cmp::Ordering::Equal)
    })?;
    let mut color = color_for(hottest.celsius, c.warn_c, c.panic_c);

    let mut body = format!(
        "\nCPU temperature {t:.1} C\n\ntemp : {t:.1}\n",
        t = hottest.celsius
    );
    if sensors.len() > 1 {
        body.push('\n');
        for (i, z) in sensors.iter().enumerate() {
            body.push_str(&format!("temp{i} : {:.1}\n", z.celsius));
        }
    }
    body.push_str(&format!(
        "\nYellow from {:.0} C, red from {:.0} C\n",
        c.warn_c, c.panic_c
    ));
    if let Some(flags) = throttled {
        let now = throttle_now(flags);
        if !now.is_empty() {
            if color == Color::Green {
                color = Color::Yellow;
            }
            // not a `name : number` line, so NCV ignores it
            body.push_str(&format!(
                "\n&yellow firmware: {} (get_throttled 0x{flags:x})\n",
                now.join(", ")
            ));
        }
    }

    let (now, _) = common::now();
    let date = common::strftime("%a %d %b %H:%M:%S %Z %Y", now, false);
    let minutes = (cfg.schedule.interval_s * 2).div_ceil(60).max(1);
    Some(proto::status(
        Some(&format!("{minutes}m")),
        &cfg.machine(),
        &c.status_column,
        color,
        &date,
        &body,
    ))
}

/// Read the sensors now and build the status (None when this host has no usable sensor).
pub fn current_status(cfg: &Config) -> Option<Vec<u8>> {
    status_message(cfg, &read_zones(), read_throttled())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        let mut c = Config::default();
        c.identity.hostname = "pi.lan".into();
        c
    }

    fn z(n: &str, c: f64) -> Zone {
        Zone {
            name: n.into(),
            celsius: c,
        }
    }

    fn text(v: Option<Vec<u8>>) -> String {
        String::from_utf8(v.unwrap()).unwrap()
    }

    #[test]
    fn millidegrees() {
        assert_eq!(parse_milli("48312\n"), Some(48.312));
        assert_eq!(parse_milli("-274000"), None);
        assert_eq!(parse_milli("-5000"), Some(-5.0));
        assert_eq!(parse_milli("junk"), None);
        assert_eq!(parse_milli("999999999"), None);
    }

    #[test]
    fn matches_the_existing_graph_layout() {
        let m = text(status_message(&cfg(), &[z("cpu-thermal", 42.4)], None));
        let (first, body) = m.split_once('\n').unwrap();
        // first line: header + date only, like the original test
        assert!(
            first.starts_with("status+10m pi,lan.cputemp green "),
            "{first}"
        );
        assert!(!first.contains("CPU temperature"));
        assert_eq!(
            body,
            "\nCPU temperature 42.4 C\n\ntemp : 42.4\n\nYellow from 70 C, red from 80 C\n"
        );
    }

    /// The server's NCV handler reads `name : number` lines: exactly one data set, `temp`.
    #[test]
    fn ncv_sees_a_single_temp_value() {
        let m = text(status_message(
            &cfg(),
            &[z("cpu-thermal", 42.4), z("gpu", 44.0)],
            Some(0x5),
        ));
        let ncv: Vec<(&str, f64)> = m
            .lines()
            .skip(1)
            .filter_map(|l| {
                let (k, v) = l.split_once(':')?;
                Some((k.trim(), v.trim().parse().ok()?))
            })
            .collect();
        assert_eq!(ncv, vec![("temp", 42.4)]);
    }

    #[test]
    fn thresholds_pick_the_colour() {
        let c = cfg();
        assert!(text(status_message(&c, &[z("cpu-thermal", 52.0)], None)).contains(" green "));
        assert!(text(status_message(&c, &[z("cpu-thermal", 70.0)], None)).contains(" yellow "));
        assert!(text(status_message(&c, &[z("cpu-thermal", 80.0)], None)).contains(" red "));
    }

    #[test]
    fn only_cpu_sensors_decide() {
        let c = cfg();
        let m = text(status_message(
            &c,
            &[z("cpu-thermal", 50.0), z("iwlwifi_1", 95.0)],
            None,
        ));
        assert!(m.contains(" green ") && m.contains("temp : 50.0\n"), "{m}");
        // no CPU-looking sensor: all of them count
        assert!(text(status_message(&c, &[z("battery", 85.0)], None)).contains(" red "));
    }

    /// Intel laptop (lmde): acpitz and B0D4 sit at a fixed bogus 91 C; the package sensor is the truth.
    #[test]
    fn package_sensor_beats_acpitz() {
        let c = cfg();
        let zones = [
            z("acpitz", 91.0),
            z("SEN1", 61.0),
            z("INT3400_Thermal", 20.0),
            z("pch_cometlake", 54.0),
            z("B0D4", 91.0),
            z("iwlwifi_1", 60.0),
            z("x86_pkg_temp", 72.0),
        ];
        let m = text(status_message(&c, &zones, None));
        assert!(m.contains(" yellow ") && m.contains("temp : 72.0\n"), "{m}");
        assert!(!m.contains("temp0"), "{m}");
        // acpitz still counts when it is the only CPU-looking sensor
        let m = text(status_message(
            &c,
            &[z("acpitz", 45.0), z("nvme", 60.0)],
            None,
        ));
        assert!(m.contains(" green ") && m.contains("temp : 45.0\n"), "{m}");
        assert_eq!(
            cpu_sensors(&[
                z("cpu-thermal", 1.0),
                z("acpitz", 2.0),
                z("x86_pkg_temp", 3.0)
            ])
            .iter()
            .map(|z| z.name.as_str())
            .collect::<Vec<_>>(),
            ["x86_pkg_temp"]
        );
    }

    /// Dual-socket x86 (biggie): both x86_pkg_temp zones count, the hottest sets `temp`, each gets a `tempN` line.
    #[test]
    fn all_packages_are_reported() {
        let zones = [
            z("x86_pkg_temp", 54.0),
            z("acpitz", 91.0),
            z("x86_pkg_temp_1", 58.5),
        ];
        let m = text(status_message(&cfg(), &zones, None));
        let (_, body) = m.split_once('\n').unwrap();
        assert_eq!(
            body,
            "\nCPU temperature 58.5 C\n\ntemp : 58.5\n\ntemp0 : 54.0\ntemp1 : 58.5\n\nYellow from 70 C, red from 80 C\n"
        );
    }

    #[test]
    fn pi_throttling_raises_green_to_yellow() {
        let c = cfg();
        let m = text(status_message(&c, &[z("cpu-thermal", 55.0)], Some(0x50005)));
        assert!(m.contains(" yellow "), "{m}");
        assert!(m.contains("&yellow firmware: under-voltage, throttled (get_throttled 0x50005)"));
        // past-only flags (bits 16+) do not matter
        assert!(
            text(status_message(&c, &[z("cpu-thermal", 55.0)], Some(0x50000))).contains(" green ")
        );
        assert_eq!(
            throttle_now(0xf),
            vec![
                "under-voltage",
                "arm frequency capped",
                "throttled",
                "soft temperature limit"
            ]
        );
    }

    fn put(root: &std::path::Path, rel: &str, content: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    #[test]
    fn reads_sysfs_thermal_then_hwmon() {
        let root = std::env::temp_dir().join(format!("zymon-sys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        put(&root, "class/thermal/thermal_zone0/type", "cpu-thermal\n");
        put(&root, "class/thermal/thermal_zone0/temp", "48312\n");
        put(&root, "class/thermal/thermal_zone10/type", "gpu\n");
        put(&root, "class/thermal/thermal_zone10/temp", "50000\n");
        put(&root, "class/thermal/thermal_zone2/temp", "garbage");
        put(&root, "class/hwmon/hwmon0/name", "coretemp\n");
        put(&root, "class/hwmon/hwmon0/temp1_input", "99000\n");
        let z = read_zones_from(root.to_str().unwrap());
        assert_eq!(
            z,
            vec![
                Zone {
                    name: "cpu-thermal".into(),
                    celsius: 48.312
                },
                Zone {
                    name: "gpu".into(),
                    celsius: 50.0
                }
            ]
        );

        // no thermal zones: hwmon takes over
        std::fs::remove_dir_all(root.join("class/thermal")).unwrap();
        let z = read_zones_from(root.to_str().unwrap());
        assert_eq!(
            z,
            vec![Zone {
                name: "coretemp".into(),
                celsius: 99.0
            }]
        );
        assert!(read_zones_from("/nonexistent").is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_sensor_no_status() {
        assert!(status_message(&cfg(), &[], None).is_none());
    }

    #[test]
    fn duplicate_names_are_made_unique() {
        let v = unique(vec![
            ("x86_pkg_temp".into(), 40.0),
            ("x86_pkg_temp".into(), 41.0),
            ("a b".into(), 1.0),
        ]);
        assert_eq!(
            v.iter().map(|z| z.name.as_str()).collect::<Vec<_>>(),
            ["x86_pkg_temp", "x86_pkg_temp_1", "a_b"]
        );
    }
}
