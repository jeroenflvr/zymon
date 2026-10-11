//! zymon extra: discrete NVIDIA GPUs through `nvidia-smi` (ships with the driver, needs no root: `/dev/nvidia*` are
//! world-accessible). The `[gpu]` client section has one line per card,
//! `gpu<N> <temp C|-> <util %|-> <mem used MiB|-> <mem total MiB|-> <power W|-> <bus id> <name> [# fault]`, and the
//! `gpu` status column is evaluated client-side (like `cputemp`) from the same capture: yellow/red on temperature
//! (`[gpu] temp_warn_c`/`temp_panic_c`) or memory use (`mem_warn_pct`/`mem_panic_pct`), red when nvidia-smi fails
//! (driver not loaded, GPU fallen off the bus) or reports a card as lost. Integrated GPUs (Jetson/Tegra report no
//! dedicated memory) are left out. Linux only.

use super::{Ctx, Res, unsupported};
use crate::config::Config;
use crate::sys::{common, exec};
use std::io;
use std::time::Duration;
use zymon_core::proto::{self, Color, SectionWriter};

/// `name` is last so a (theoretical) comma in it cannot shift the other columns.
const QUERY: &str = "--query-gpu=index,pci.bus_id,temperature.gpu,utilization.gpu,memory.used,memory.total,power.draw,name";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Card {
    pub index: String,
    pub bus_id: String,
    pub name: String,
    pub temp_c: Option<f64>,
    pub util_pct: Option<f64>,
    pub mem_used_mib: Option<f64>,
    pub mem_total_mib: Option<f64>,
    pub power_w: Option<f64>,
    /// A field nvidia-smi could not read for a reason other than "not supported" (`[GPU is lost]`, `[Unknown Error]`).
    pub fault: Option<String>,
}

impl Card {
    pub fn mem_pct(&self) -> Option<f64> {
        match (self.mem_used_mib, self.mem_total_mib) {
            (Some(u), Some(t)) if t > 0.0 => Some(u * 100.0 / t),
            _ => None,
        }
    }

    /// `[gpu]` section line; spaces in the name become `_`, missing values `-`.
    pub fn section_line(&self) -> String {
        let num = |v: Option<f64>| v.map_or("-".to_string(), |n| format!("{n}"));
        let name: String = self.name.split_whitespace().collect::<Vec<_>>().join("_");
        let mut s = format!(
            "gpu{} {} {} {} {} {} {} {}",
            self.index,
            num(self.temp_c),
            num(self.util_pct),
            num(self.mem_used_mib),
            num(self.mem_total_mib),
            num(self.power_w),
            if self.bus_id.is_empty() {
                "-"
            } else {
                &self.bus_id
            },
            if name.is_empty() { "-" } else { &name }
        );
        if let Some(f) = &self.fault {
            s.push_str(&format!(" # {f}"));
        }
        s
    }
}

/// `Ok(None)` for "this card has no such value", `Err` for a fault worth alerting on.
fn value(field: &str) -> Result<Option<f64>, String> {
    let f = field.trim();
    if let Ok(v) = f.parse::<f64>() {
        return Ok(Some(v));
    }
    match f {
        "" | "N/A" | "[N/A]" | "[Not Supported]" => Ok(None),
        _ => Err(f.trim_matches(['[', ']']).to_string()),
    }
}

/// One `--format=csv,noheader,nounits` line in `QUERY` order. None for lines that are not GPU rows.
pub fn parse_line(line: &str) -> Option<Card> {
    let f: Vec<&str> = line.split(',').map(str::trim).collect();
    if f.len() < 8 || f[0].parse::<u32>().is_err() {
        return None;
    }
    let mut faults = Vec::new();
    let mut val = |s: &str| {
        value(s).unwrap_or_else(|e| {
            if !faults.contains(&e) {
                faults.push(e);
            }
            None
        })
    };
    let card = Card {
        index: f[0].to_string(),
        bus_id: f[1].to_string(),
        temp_c: val(f[2]),
        util_pct: val(f[3]),
        mem_used_mib: val(f[4]),
        mem_total_mib: val(f[5]),
        power_w: val(f[6]),
        name: f[7..].join(", "),
        fault: None,
    };
    Some(Card {
        fault: (!faults.is_empty()).then(|| faults.join("; ")),
        ..card
    })
}

/// Discrete cards only: integrated GPUs report no dedicated memory. A card with a fault is always kept.
pub fn parse(text: &str) -> Vec<Card> {
    text.lines()
        .filter_map(parse_line)
        .filter(|c| c.mem_total_mib.is_some() || c.fault.is_some())
        .collect()
}

/// The first line nvidia-smi printed, made safe for a status body (no `:`/`=`, which NCV would read as data).
fn failure_text(stdout: &[u8]) -> String {
    let s = String::from_utf8_lossy(stdout);
    let first = s.lines().map(str::trim).find(|l| !l.is_empty());
    first
        .unwrap_or("nvidia-smi failed without output")
        .replace([':', '='], " ")
}

/// `Err` carries the reason as text: a failing nvidia-smi on a GPU host is itself an alert.
pub fn query(timeout: Duration) -> io::Result<Result<Vec<Card>, String>> {
    let o = exec::run(
        "nvidia-smi",
        &[QUERY, "--format=csv,noheader,nounits"],
        timeout,
    )?;
    if !o.success {
        return Ok(Err(failure_text(&o.stdout)));
    }
    Ok(Ok(parse(&String::from_utf8_lossy(&o.stdout))))
}

/// The capture is kept in the context so the status column reuses it instead of running nvidia-smi again.
pub fn gpu(ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    if !cfg!(target_os = "linux") {
        return unsupported();
    }
    let r = query(ctx.cfg.exec_timeout())?;
    match &r {
        Ok(cards) => {
            for c in cards {
                w.line(c.section_line());
            }
        }
        Err(e) => w.line(format!("# nvidia-smi failed: {e}")),
    }
    ctx.gpu = Some(r);
    Ok(())
}

fn level(v: Option<f64>, warn: f64, panic: f64) -> Color {
    match v {
        Some(v) if v >= panic => Color::Red,
        Some(v) if v >= warn => Color::Yellow,
        _ => Color::Green,
    }
}

fn worse(a: Color, b: Color) -> Color {
    let rank = |c: Color| match c {
        Color::Clear => 0,
        Color::Green => 1,
        Color::Yellow => 2,
        Color::Red => 3,
    };
    if rank(b) > rank(a) { b } else { a }
}

/// Colour of one card plus the reasons for anything worse than green.
fn judge(cfg: &Config, c: &Card) -> (Color, Vec<String>) {
    let g = &cfg.gpu;
    let mut color = Color::Green;
    let mut why = Vec::new();
    if let Some(f) = &c.fault {
        color = Color::Red;
        why.push(f.replace([':', '='], " "));
    }
    let t = level(c.temp_c, g.temp_warn_c, g.temp_panic_c);
    if t != Color::Green {
        color = worse(color, t);
        why.push(format!("temperature {:.0} C", c.temp_c.unwrap_or(0.0)));
    }
    let m = level(c.mem_pct(), g.mem_warn_pct, g.mem_panic_pct);
    if m != Color::Green {
        color = worse(color, m);
        why.push(format!("memory {:.0} % used", c.mem_pct().unwrap_or(0.0)));
    }
    (color, why)
}

/// One human line per card, kept free of `:`/`=` so NCV ignores it.
fn describe(c: &Card) -> String {
    let mut s = format!("gpu{} {}", c.index, c.name.replace([':', '='], " "));
    if let Some(t) = c.temp_c {
        s.push_str(&format!(", {t:.0} C"));
    }
    if let Some(u) = c.util_pct {
        s.push_str(&format!(", {u:.0} % busy"));
    }
    if let (Some(u), Some(t), Some(p)) = (c.mem_used_mib, c.mem_total_mib, c.mem_pct()) {
        s.push_str(&format!(", {u:.0} of {t:.0} MiB used ({p:.0} %)"));
    }
    if let Some(w) = c.power_w {
        s.push_str(&format!(", {w:.1} W"));
    }
    s
}

/// `status+<2×interval> MACHINE.gpu`: one `&color` line per card, then `gpu<N>_temp`, `gpu<N>_util` and
/// `gpu<N>_mem` (percent of memory used) `name : value` lines for NCV graphing. None when there is no discrete GPU.
pub fn status_message(cfg: &Config, capture: &Result<Vec<Card>, String>) -> Option<Vec<u8>> {
    let g = &cfg.gpu;
    let mut body = String::from("\n");
    let (color, text) = match capture {
        Err(e) => {
            body.push_str(&format!("&red nvidia-smi failed - {e}\n"));
            (Color::Red, "GPU NOT ok - nvidia-smi failed".to_string())
        }
        Ok(cards) if cards.is_empty() => return None,
        Ok(cards) => {
            let mut color = Color::Green;
            let mut bad = Vec::new();
            for c in cards {
                let (cc, why) = judge(cfg, c);
                color = worse(color, cc);
                let mut line = format!("&{} {}", cc.as_str(), describe(c));
                if !why.is_empty() {
                    bad.push(format!("gpu{}", c.index));
                    line.push_str(&format!(" - {}", why.join(", ")));
                }
                body.push_str(&line);
                body.push('\n');
            }
            body.push_str(&format!(
                "\nYellow from {:.0} C or {:.0} % memory used, red from {:.0} C or {:.0} % memory used\n",
                g.temp_warn_c, g.mem_warn_pct, g.temp_panic_c, g.mem_panic_pct
            ));
            let mut ncv = String::new();
            for c in cards {
                let i = &c.index;
                if let Some(t) = c.temp_c {
                    ncv.push_str(&format!("gpu{i}_temp : {t}\n"));
                }
                if let Some(u) = c.util_pct {
                    ncv.push_str(&format!("gpu{i}_util : {u}\n"));
                }
                if let Some(p) = c.mem_pct() {
                    ncv.push_str(&format!("gpu{i}_mem : {p:.1}\n"));
                }
            }
            if !ncv.is_empty() {
                body.push('\n');
                body.push_str(&ncv);
            }
            let text = if bad.is_empty() {
                "GPU ok".to_string()
            } else {
                format!("GPU NOT ok - {}", bad.join(", "))
            };
            (color, text)
        }
    };
    let (now, _) = common::now();
    let date = common::strftime("%a %b %e %H:%M:%S %Z %Y", now, false);
    let minutes = (cfg.schedule.interval_s * 2).div_ceil(60).max(1);
    Some(proto::status(
        Some(&format!("{minutes}m")),
        &cfg.machine(),
        &g.status_column,
        color,
        &format!("{date} - {text}"),
        &body,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMI: &str = "0, 00000000:01:00.0, 45, 3, 1234, 10240, 25.43, NVIDIA GeForce RTX 3080\n\
                       1, 00000000:02:00.0, 86, 100, 23000, 24576, 310.2, NVIDIA RTX A5000\n";

    fn cfg() -> Config {
        let mut c = Config::default();
        c.identity.hostname = "gpu.lan".into();
        c
    }

    fn text(v: Option<Vec<u8>>) -> String {
        String::from_utf8(v.unwrap()).unwrap()
    }

    #[test]
    fn parses_cards() {
        let v = parse(SMI);
        assert_eq!(v.len(), 2);
        assert_eq!(
            v[0],
            Card {
                index: "0".into(),
                bus_id: "00000000:01:00.0".into(),
                name: "NVIDIA GeForce RTX 3080".into(),
                temp_c: Some(45.0),
                util_pct: Some(3.0),
                mem_used_mib: Some(1234.0),
                mem_total_mib: Some(10240.0),
                power_w: Some(25.43),
                fault: None,
            }
        );
        assert_eq!(
            v[0].section_line(),
            "gpu0 45 3 1234 10240 25.43 00000000:01:00.0 NVIDIA_GeForce_RTX_3080"
        );
    }

    #[test]
    fn not_supported_integrated_and_lost() {
        let v = parse(
            "0, 00000000:01:00.0, 40, [N/A], 100, 4096, [Not Supported], Tesla, K80\n\
             1, [N/A], 35, 10, [N/A], [N/A], [N/A], Orin (nvgpu)\n\
             2, 00000000:03:00.0, [GPU is lost], [GPU is lost], [GPU is lost], [GPU is lost], [GPU is lost], [GPU is lost]\n\
             No devices were found\n",
        );
        assert_eq!(v.len(), 2, "integrated Orin is dropped: {v:?}");
        assert_eq!(v[0].name, "Tesla, K80");
        assert_eq!((v[0].util_pct, v[0].power_w), (None, None));
        assert_eq!(v[0].fault, None);
        assert_eq!(v[1].fault.as_deref(), Some("GPU is lost"));
        assert!(v[1].section_line().ends_with(" # GPU is lost"));
        assert!(parse("No devices were found\n").is_empty());
    }

    #[test]
    fn thresholds_pick_the_colour() {
        let c = cfg();
        let v = parse(SMI);
        assert!(text(status_message(&c, &Ok(v[..1].to_vec()))).contains(".gpu green "));
        // card 1: 86 C (yellow) and 93.6 % memory (yellow)
        let m = text(status_message(&c, &Ok(v.clone())));
        assert!(m.contains(".gpu yellow "), "{m}");
        assert!(m.contains("GPU NOT ok - gpu1"), "{m}");
        assert!(
            m.contains("&yellow gpu1 NVIDIA RTX A5000, 86 C, 100 % busy, 23000 of 24576 MiB used (94 %), 310.2 W - temperature 86 C, memory 94 % used"),
            "{m}"
        );
        let mut hot = v[0].clone();
        hot.temp_c = Some(95.0);
        assert!(text(status_message(&c, &Ok(vec![hot]))).contains(".gpu red "));
    }

    #[test]
    fn failures_are_red_and_no_gpu_no_status() {
        let c = cfg();
        let e = failure_text(
            b"NVIDIA-SMI has failed because it couldn't communicate with the NVIDIA driver.\n",
        );
        let m = text(status_message(&c, &Err(e)));
        assert!(m.starts_with("status+10m gpu,lan.gpu red "), "{m}");
        assert!(m.contains("&red nvidia-smi failed - NVIDIA-SMI has failed"));
        let lost = parse("0, 00000000:01:00.0, [GPU is lost], 0, 0, 0, 0, X\n");
        assert!(text(status_message(&c, &Ok(lost))).contains(".gpu red "));
        assert!(status_message(&c, &Ok(Vec::new())).is_none());
    }

    /// The server's NCV handler reads `name : number` (and `name = number`) lines: only the gauges qualify.
    #[test]
    fn ncv_sees_only_the_gauges() {
        let m = text(status_message(&cfg(), &Ok(parse(SMI))));
        let ncv: Vec<(&str, f64)> = m
            .lines()
            .skip(1)
            .filter_map(|l| {
                let (k, v) = l.split_once([':', '='])?;
                let v = v.trim();
                let end = v
                    .find(|ch: char| !(ch.is_ascii_digit() || "+-.eE".contains(ch)))
                    .unwrap_or(v.len());
                Some((k.trim(), v[..end].parse().ok()?))
            })
            .collect();
        assert_eq!(
            ncv,
            vec![
                ("gpu0_temp", 45.0),
                ("gpu0_util", 3.0),
                ("gpu0_mem", 12.1),
                ("gpu1_temp", 86.0),
                ("gpu1_util", 100.0),
                ("gpu1_mem", 93.6),
            ]
        );
    }
}
