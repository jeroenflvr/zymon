//! zymon extra: S.M.A.R.T. disk health through smartmontools. The `[smart]` client section has one line per disk,
//! `<name> <PASSED|FAILED|-> <temp C|-> <power-on hours|-> <model> <serial> [# problems; note]`, and the `smart`
//! status column is evaluated client-side (like `cputemp`) from the same `smartctl` capture: red when smartctl's
//! health check fails, yellow for reallocated/pending/uncorrectable sectors, media errors or NVMe wear, clear for
//! disks in standby or without SMART. `smartctl` needs raw device access: run as root, or via `sudo -n` with a
//! sudoers rule (`install.sh` offers one). Disks are never spun up (`-n standby`).

use super::{Ctx, Res};
use crate::config::Config;
use crate::sys::{common, exec};
use std::io;
use std::time::Duration;
use zymon_core::proto::{self, Color, SectionWriter};

/// NVMe `Percentage Used` (of the rated endurance) at or above which a disk turns yellow.
pub const WEAR_WARN_PCT: u64 = 90;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Disk {
    /// Device path without `/dev/`: `sda`, `nvme0`, `disk0`.
    pub name: String,
    pub model: String,
    pub serial: String,
    /// smartctl's overall verdict (`PASSED`/`FAILED!`, SCSI `OK`); None when the device reported none.
    pub health: Option<bool>,
    pub temp_c: Option<u64>,
    pub hours: Option<u64>,
    /// Each entry turns the disk yellow.
    pub problems: Vec<String>,
    /// Nothing to judge (standby, no SMART, open failed): the disk shows as clear.
    pub note: Option<String>,
}

impl Disk {
    pub fn color(&self) -> Color {
        if self.health == Some(false) {
            Color::Red
        } else if !self.problems.is_empty() {
            Color::Yellow
        } else if self.note.is_some() {
            Color::Clear
        } else {
            Color::Green
        }
    }

    fn health_str(&self) -> &'static str {
        match self.health {
            Some(true) => "PASSED",
            Some(false) => "FAILED",
            None => "-",
        }
    }

    fn tail(&self) -> String {
        let mut t: Vec<&str> = self.problems.iter().map(String::as_str).collect();
        if let Some(n) = &self.note {
            t.push(n);
        }
        t.join("; ")
    }

    /// `[smart]` section line; spaces in model/serial become `_`, missing fields `-`.
    pub fn section_line(&self) -> String {
        let field = |s: &str| {
            if s.is_empty() {
                "-".to_string()
            } else {
                s.split_whitespace().collect::<Vec<_>>().join("_")
            }
        };
        let num = |v: Option<u64>| v.map_or("-".to_string(), |n| n.to_string());
        let mut s = format!(
            "{} {} {} {} {} {}",
            self.name,
            self.health_str(),
            num(self.temp_c),
            num(self.hours),
            field(&self.model),
            field(&self.serial)
        );
        let tail = self.tail();
        if !tail.is_empty() {
            s.push_str(" # ");
            s.push_str(&tail);
        }
        s
    }

    /// Human line for the status body. No `:` or `=` so the server's NCV handler ignores it.
    fn describe(&self) -> String {
        let mut parts = vec![self.name.clone()];
        if self.health.is_some() {
            parts.push(self.health_str().to_string());
        }
        if let Some(t) = self.temp_c {
            parts.push(format!("{t} C"));
        }
        if let Some(h) = self.hours {
            parts.push(format!("{h} h"));
        }
        if !self.model.is_empty() {
            let mut m = self.model.clone();
            if !self.serial.is_empty() {
                m.push_str(&format!(" ({})", self.serial));
            }
            parts.push(m);
        }
        let mut s = parts.join(", ");
        let tail = self.tail();
        if !tail.is_empty() {
            s.push_str(" - ");
            s.push_str(&tail);
        }
        s.replace([':', '='], " ")
    }
}

/// Leading unsigned integer of a smartctl value: `"1,234"` → 1234, `"38 Celsius"` → 38, `"12345h+30m"` → 12345.
fn leading_num(s: &str) -> Option<u64> {
    let digits: String = s
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',')
        .filter(|c| *c != ',')
        .collect();
    digits.parse().ok()
}

fn hex_or_dec(s: &str) -> Option<u64> {
    let s = s.trim();
    match s.strip_prefix("0x") {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

/// One `smartctl --scan-open` line → `["/dev/sda", "-d", "sat"]`; None for comments and blank lines.
pub fn parse_scan_line(line: &str) -> Option<Vec<String>> {
    let spec = line.split('#').next()?.trim();
    (!spec.is_empty()).then(|| spec.split_whitespace().map(str::to_string).collect())
}

pub fn dev_name(dev: &str) -> String {
    dev.strip_prefix("/dev/").unwrap_or(dev).replace(' ', "_")
}

/// One attribute row of the ATA table (default `smartctl -A` layout, 10+ columns).
fn ata_attribute(d: &mut Disk, line: &str) {
    let f: Vec<&str> = line.split_whitespace().collect();
    if f.len() < 10 {
        return;
    }
    let Ok(id) = f[0].parse::<u32>() else {
        return;
    };
    let raw = leading_num(f[9]);
    let mut count = |what: &str| {
        if let Some(n) = raw.filter(|&n| n > 0) {
            d.problems.push(format!("{n} {what}"));
        }
    };
    match id {
        5 => count("reallocated sectors"),
        187 => count("reported uncorrectable errors"),
        197 => count("pending sectors"),
        198 => count("offline uncorrectable sectors"),
        9 => d.hours = raw,
        // 190 Airflow_Temperature_Cel is the fallback for drives without 194
        194 => d.temp_c = raw.filter(|t| (1..=150).contains(t)),
        190 if d.temp_c.is_none() => d.temp_c = raw.filter(|t| (1..=150).contains(t)),
        _ => {}
    }
    match f[8] {
        "-" => {}
        "FAILING_NOW" => d.problems.push(format!("{} failing now", f[1])),
        _ => d.problems.push(format!("{} failed in the past", f[1])),
    }
}

/// Parse `smartctl -i -H -A -n standby <dev>` text output (ATA, NVMe and SCSI layouts).
pub fn parse_report(name: &str, text: &str) -> Disk {
    let mut d = Disk {
        name: name.to_string(),
        ..Default::default()
    };
    let mut vendor = String::new();
    let mut in_table = false;
    let mut saw_data = false;
    for raw in text.lines() {
        let l = raw.trim();
        if in_table {
            if l.is_empty() {
                in_table = false;
            } else {
                saw_data = true;
                ata_attribute(&mut d, l);
            }
            continue;
        }
        if l.starts_with("ID# ATTRIBUTE_NAME") {
            in_table = true;
            continue;
        }
        if l.starts_with("Device is in ") && l.contains(" mode") {
            d.note = Some("standby".into());
            continue;
        }
        if let Some(rest) = l.strip_prefix("Smartctl open device:") {
            let why = rest.rsplit("failed: ").next().unwrap_or(rest).trim();
            d.note = Some(format!("cannot open ({why})"));
            continue;
        }
        if let Some(v) = l.strip_prefix("number of hours powered up = ") {
            d.hours = leading_num(v);
            saw_data = true;
            continue;
        }
        let Some((k, v)) = l.split_once(':') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "Device Model" | "Model Number" => d.model = v.into(),
            "Vendor" => vendor = v.into(),
            "Product" => {
                d.model = if vendor.is_empty() {
                    v.into()
                } else {
                    format!("{vendor} {v}")
                }
            }
            "Serial Number" => d.serial = v.into(),
            "SMART support is" => {
                if v.starts_with("Unavailable") {
                    d.note = Some("SMART not available".into());
                } else if v.starts_with("Disabled") {
                    d.note = Some("SMART disabled".into());
                }
            }
            "SMART overall-health self-assessment test result" => {
                d.health = Some(v.starts_with("PASSED"));
                saw_data = true;
            }
            "SMART Health Status" => {
                d.health = Some(v == "OK");
                saw_data = true;
            }
            "Critical Warning" => {
                saw_data = true;
                if let Some(w) = hex_or_dec(v).filter(|&w| w != 0) {
                    d.problems.push(format!("critical warning 0x{w:02x}"));
                }
            }
            "Temperature" | "Current Drive Temperature" => d.temp_c = leading_num(v),
            "Percentage Used" => {
                if let Some(p) = leading_num(v).filter(|&p| p >= WEAR_WARN_PCT) {
                    d.problems.push(format!("{p}% of rated endurance used"));
                }
            }
            "Media and Data Integrity Errors" => {
                if let Some(n) = leading_num(v).filter(|&n| n > 0) {
                    d.problems.push(format!("{n} media errors"));
                }
            }
            "Elements in grown defect list" => {
                if let Some(n) = leading_num(v).filter(|&n| n > 0) {
                    d.problems.push(format!("{n} grown defects"));
                }
            }
            "Power On Hours" => d.hours = leading_num(v),
            _ => {}
        }
    }
    if !saw_data && d.note.is_none() {
        d.note = Some("no SMART data".into());
    }
    d
}

/// `smartctl` directly as root, else through `sudo -n` (a NOPASSWD sudoers rule is expected).
fn smartctl(args: &[&str], timeout: Duration) -> io::Result<exec::Output> {
    if common::euid() == 0 {
        return exec::run("smartctl", args, timeout);
    }
    let path = exec::which("smartctl")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "smartctl: not found"))?;
    let path = path.to_string_lossy();
    let mut v = vec!["-n", &*path];
    v.extend_from_slice(args);
    exec::run("sudo", &v, timeout)
}

/// Device specs to query: `[smart] devices` from the config, else `smartctl --scan-open`.
fn device_specs(cfg: &Config, timeout: Duration) -> io::Result<Vec<Vec<String>>> {
    if !cfg.smart.devices.is_empty() {
        return Ok(cfg
            .smart
            .devices
            .iter()
            .filter_map(|s| parse_scan_line(s))
            .collect());
    }
    let o = smartctl(&["--scan-open"], timeout)?;
    if !o.success && o.stdout.is_empty() {
        return Err(io::Error::other(
            "smartctl --scan-open failed (needs root or a sudoers rule)",
        ));
    }
    Ok(String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter_map(parse_scan_line)
        .collect())
}

/// Run smartctl once per disk. A non-zero exit is normal (bit flags: standby, failing disk), so only the
/// output counts.
pub fn query(cfg: &Config) -> io::Result<Vec<Disk>> {
    let t = cfg.exec_timeout();
    let mut out = Vec::new();
    for spec in device_specs(cfg, t)? {
        let name = dev_name(&spec[0]);
        let mut args = vec!["-i", "-H", "-A", "-n", "standby"];
        args.extend(spec[1..].iter().map(String::as_str));
        args.push(&spec[0]);
        out.push(match smartctl(&args, t) {
            Ok(o) => parse_report(&name, &String::from_utf8_lossy(&o.stdout)),
            Err(e) => Disk {
                name,
                note: Some(format!("smartctl {e}").replace([':', '='], " ")),
                ..Default::default()
            },
        });
    }
    Ok(out)
}

/// The capture is kept in the context so the status column reuses it instead of running smartctl again.
pub fn smart(ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    let disks = query(ctx.cfg)?;
    for d in &disks {
        w.line(d.section_line());
    }
    ctx.smart = Some(disks);
    Ok(())
}

fn rank(c: Color) -> u8 {
    match c {
        Color::Clear => 0,
        Color::Green => 1,
        Color::Yellow => 2,
        Color::Red => 3,
    }
}

/// `status+<2×interval> MACHINE.smart`: one `&color` line per disk, then `<name>_temp : <C>` lines for NCV
/// graphing. None when there is no disk at all.
pub fn status_message(cfg: &Config, disks: &[Disk]) -> Option<Vec<u8>> {
    if disks.is_empty() {
        return None;
    }
    let mut color = Color::Clear;
    let mut bad = Vec::new();
    let mut body = String::from("\n");
    for d in disks {
        let c = d.color();
        if rank(c) > rank(color) {
            color = c;
        }
        if matches!(c, Color::Red | Color::Yellow) {
            bad.push(d.name.as_str());
        }
        body.push_str(&format!("&{} {}\n", c.as_str(), d.describe()));
    }
    let temps: Vec<String> = disks
        .iter()
        .filter_map(|d| d.temp_c.map(|t| format!("{}_temp : {t}\n", d.name)))
        .collect();
    if !temps.is_empty() {
        body.push('\n');
        body.extend(temps);
    }
    let text = if bad.is_empty() {
        "SMART ok".to_string()
    } else {
        format!("SMART NOT ok: {}", bad.join(", "))
    };
    let (now, _) = common::now();
    let date = common::strftime("%a %b %e %H:%M:%S %Z %Y", now, false);
    let minutes = (cfg.schedule.interval_s * 2).div_ceil(60).max(1);
    Some(proto::status(
        Some(&format!("{minutes}m")),
        &cfg.machine(),
        &cfg.smart.status_column,
        color,
        &format!("{date} - {text}"),
        &body,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ATA: &str = "smartctl 7.3 2022-02-28 r5338 [x86_64-linux-6.1.0] (local build)
Copyright (C) 2002-22, Bruce Allen, Christian Franke, www.smartmontools.org

=== START OF INFORMATION SECTION ===
Model Family:     Western Digital Red
Device Model:     WDC WD40EFRX-68N32N0
Serial Number:    WD-WCC7K1234567
Firmware Version: 82.00A82
User Capacity:    4,000,787,030,016 bytes [4.00 TB]
SMART support is: Available - device has SMART capability.
SMART support is: Enabled

=== START OF READ SMART DATA SECTION ===
SMART overall-health self-assessment test result: PASSED

SMART Attributes Data Structure revision number: 16
Vendor Specific SMART Attributes with Thresholds:
ID# ATTRIBUTE_NAME          FLAG     VALUE WORST THRESH TYPE      UPDATED  WHEN_FAILED RAW_VALUE
  1 Raw_Read_Error_Rate     0x002f   200   200   051    Pre-fail  Always       -       0
  5 Reallocated_Sector_Ct   0x0033   200   200   140    Pre-fail  Always       -       0
  9 Power_On_Hours          0x0032   083   083   000    Old_age   Always       -       12345
190 Airflow_Temperature_Cel 0x0022   066   050   045    Old_age   Always       -       34 (Min/Max 20/45)
194 Temperature_Celsius     0x0022   112   098   000    Old_age   Always       -       35
197 Current_Pending_Sector  0x0032   200   200   000    Old_age   Always       -       0
198 Offline_Uncorrectable   0x0030   100   253   000    Old_age   Offline      -       0

";

    const NVME: &str = "=== START OF INFORMATION SECTION ===
Model Number:                       Samsung SSD 980 PRO 1TB
Serial Number:                      S5GXNX0R123456A
Firmware Version:                   5B2QGXA7
Total NVM Capacity:                 1,000,204,886,016 [1.00 TB]

=== START OF SMART DATA SECTION ===
SMART overall-health self-assessment test result: FAILED!
- available spare has fallen below threshold

SMART/Health Information (NVMe Log 0x02)
Critical Warning:                   0x01
Temperature:                        38 Celsius
Available Spare:                    5%
Available Spare Threshold:          10%
Percentage Used:                    93%
Data Units Read:                    12,345,678 [6.32 TB]
Power On Hours:                     1,234
Media and Data Integrity Errors:    2
Temperature Sensor 1:               38 Celsius
Temperature Sensor 2:               44 Celsius
";

    const SCSI: &str = "=== START OF INFORMATION SECTION ===
Vendor:               SEAGATE
Product:              ST4000NM0023
Serial number:        Z1Z12345
Device type:          disk

=== START OF READ SMART DATA SECTION ===
SMART Health Status: OK

Current Drive Temperature:     31 C
Drive Trip Temperature:        60 C

Elements in grown defect list: 3

  number of hours powered up = 40123.45
";

    fn cfg() -> Config {
        let mut c = Config::default();
        c.identity.hostname = "nas.lan".into();
        c
    }

    fn text(v: Option<Vec<u8>>) -> String {
        String::from_utf8(v.unwrap()).unwrap()
    }

    #[test]
    fn ata_report() {
        let d = parse_report("sda", ATA);
        assert_eq!(d.model, "WDC WD40EFRX-68N32N0");
        assert_eq!(d.serial, "WD-WCC7K1234567");
        assert_eq!(d.health, Some(true));
        assert_eq!(d.temp_c, Some(35), "194 beats 190");
        assert_eq!(d.hours, Some(12345));
        assert!(d.problems.is_empty() && d.note.is_none());
        assert_eq!(d.color(), Color::Green);
        assert_eq!(
            d.section_line(),
            "sda PASSED 35 12345 WDC_WD40EFRX-68N32N0 WD-WCC7K1234567"
        );
    }

    #[test]
    fn ata_bad_sectors_and_failed_attributes() {
        let t = ATA
            .replace(
                "197 Current_Pending_Sector  0x0032   200   200   000    Old_age   Always       -       0",
                "197 Current_Pending_Sector  0x0032   200   200   000    Old_age   Always       -       8",
            )
            .replace(
                "  5 Reallocated_Sector_Ct   0x0033   200   200   140    Pre-fail  Always       -       0",
                "  5 Reallocated_Sector_Ct   0x0033   100   100   140    Pre-fail  Always   In_the_past 16",
            );
        let d = parse_report("sda", &t);
        assert_eq!(
            d.problems,
            vec![
                "16 reallocated sectors",
                "Reallocated_Sector_Ct failed in the past",
                "8 pending sectors"
            ]
        );
        assert_eq!(d.color(), Color::Yellow);
        assert!(d.section_line().ends_with(
            " # 16 reallocated sectors; Reallocated_Sector_Ct failed in the past; 8 pending sectors"
        ));
    }

    #[test]
    fn nvme_report() {
        let d = parse_report("nvme0", NVME);
        assert_eq!(d.model, "Samsung SSD 980 PRO 1TB");
        assert_eq!(d.health, Some(false));
        assert_eq!(d.temp_c, Some(38));
        assert_eq!(d.hours, Some(1234));
        assert_eq!(
            d.problems,
            vec![
                "critical warning 0x01",
                "93% of rated endurance used",
                "2 media errors"
            ]
        );
        assert_eq!(d.color(), Color::Red);
    }

    #[test]
    fn scsi_report() {
        let d = parse_report("sdb", SCSI);
        assert_eq!(d.model, "SEAGATE ST4000NM0023");
        assert_eq!(d.health, Some(true));
        assert_eq!(d.temp_c, Some(31));
        assert_eq!(d.hours, Some(40123));
        assert_eq!(d.problems, vec!["3 grown defects"]);
        assert_eq!(d.color(), Color::Yellow);
    }

    #[test]
    fn standby_unavailable_and_garbage() {
        let d = parse_report(
            "sdc",
            "smartctl 7.3\n\nDevice is in STANDBY mode, exit(2)\n",
        );
        assert_eq!(d.note.as_deref(), Some("standby"));
        assert_eq!(d.color(), Color::Clear);
        assert_eq!(d.section_line(), "sdc - - - - - # standby");

        let d = parse_report(
            "sdd",
            "Device Model:     USB Stick\nSMART support is: Unavailable - device lacks SMART capability.\n",
        );
        assert_eq!(d.note.as_deref(), Some("SMART not available"));

        let d = parse_report(
            "sde",
            "Smartctl open device: /dev/sde failed: Permission denied\n",
        );
        assert_eq!(d.note.as_deref(), Some("cannot open (Permission denied)"));

        let d = parse_report("sdf", "");
        assert_eq!(d.note.as_deref(), Some("no SMART data"));
        assert_eq!(d.color(), Color::Clear);
    }

    #[test]
    fn scan_lines_and_names() {
        assert_eq!(
            parse_scan_line("/dev/sda -d sat # /dev/sda [SAT], ATA device"),
            Some(vec!["/dev/sda".to_string(), "-d".into(), "sat".into()])
        );
        assert_eq!(
            parse_scan_line("/dev/nvme0 -d nvme # /dev/nvme0, NVMe device").unwrap()[0],
            "/dev/nvme0"
        );
        assert_eq!(
            parse_scan_line("# /dev/sdb, SCSI device open failed: Permission denied"),
            None
        );
        assert_eq!(parse_scan_line("   "), None);
        assert_eq!(dev_name("/dev/sda"), "sda");
        assert_eq!(dev_name("disk0"), "disk0");
        assert_eq!(leading_num("1,234 [6 TB]"), Some(1234));
        assert_eq!(leading_num("12345h+30m+10.000s"), Some(12345));
        assert_eq!(leading_num("-"), None);
        assert_eq!(hex_or_dec("0x04"), Some(4));
        assert_eq!(hex_or_dec("7"), Some(7));
    }

    #[test]
    fn status_colour_follows_the_worst_disk() {
        let c = cfg();
        let ok = parse_report("sda", ATA);
        let sleeping = parse_report("sdc", "Device is in STANDBY mode, exit(2)\n");
        let m = text(status_message(&c, &[ok.clone(), sleeping.clone()]));
        let (first, body) = m.split_once('\n').unwrap();
        assert!(
            first.starts_with("status+10m nas,lan.smart green "),
            "{first}"
        );
        assert!(first.ends_with(" - SMART ok"));
        assert!(body.contains(
            "\n&green sda, PASSED, 35 C, 12345 h, WDC WD40EFRX-68N32N0 (WD-WCC7K1234567)\n"
        ));
        assert!(body.contains("&clear sdc - standby\n"));
        assert!(body.ends_with("\nsda_temp : 35\n"), "{body}");

        let m = text(status_message(
            &c,
            &[ok, parse_report("sdb", SCSI), parse_report("nvme0", NVME)],
        ));
        assert!(m.starts_with("status+10m nas,lan.smart red "), "{m}");
        assert!(m.contains(" - SMART NOT ok: sdb, nvme0\n"));
        assert!(m.contains(
            "&yellow sdb, PASSED, 31 C, 40123 h, SEAGATE ST4000NM0023 - 3 grown defects\n"
        ));
        assert!(m.contains("&red nvme0, FAILED, 38 C, 1234 h, Samsung SSD 980 PRO 1TB (S5GXNX0R123456A) - critical warning 0x01; 93% of rated endurance used; 2 media errors\n"));

        // all disks asleep: a clear column; no disk: no column
        assert!(text(status_message(&c, &[sleeping])).contains(" clear "));
        assert!(status_message(&c, &[]).is_none());
    }

    /// The server's NCV handler reads `name : number` lines: only the temperatures qualify.
    #[test]
    fn ncv_sees_only_temperatures() {
        let m = text(status_message(
            &cfg(),
            &[
                parse_report("sda", ATA),
                parse_report("nvme0", NVME),
                parse_report("sdb", SCSI),
            ],
        ));
        let ncv: Vec<(&str, f64)> = m
            .lines()
            .skip(1)
            .filter_map(|l| {
                let (k, v) = l.split_once([':', '='])?;
                Some((k.trim(), v.trim().parse().ok()?))
            })
            .collect();
        assert_eq!(
            ncv,
            vec![("sda_temp", 35.0), ("nvme0_temp", 38.0), ("sdb_temp", 31.0)]
        );
    }

    #[test]
    fn explicit_devices_from_config() {
        let mut c = cfg();
        c.smart.devices = vec!["/dev/sda".into(), "/dev/sdb -d sat".into(), " ".into()];
        let specs = device_specs(&c, Duration::from_secs(1)).unwrap();
        assert_eq!(
            specs,
            vec![
                vec!["/dev/sda".to_string()],
                vec!["/dev/sdb".to_string(), "-d".into(), "sat".into()]
            ]
        );
    }
}
