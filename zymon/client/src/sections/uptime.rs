use super::{Ctx, Res};
use crate::sys::common;
use zymon_core::proto::SectionWriter;

/// Reproduce `uptime(1)`. The server needs `up N days, HH:MM,` (or `N min,`) and `load average: a, b, c`
/// (BSD: `load averages: a b c`); everything else is decoration.
pub fn format_uptime(clock: &str, up_secs: u64, users: usize, load: [f64; 3], bsd: bool) -> String {
    let days = up_secs / 86400;
    let hours = (up_secs % 86400) / 3600;
    let mins = (up_secs % 3600) / 60;
    let mut s = if bsd {
        format!("{clock}  up ")
    } else {
        format!(" {clock} up ")
    };
    if days > 0 {
        s.push_str(&format!("{days} day{}, ", if days == 1 { "" } else { "s" }));
    }
    if hours > 0 {
        if bsd {
            s.push_str(&format!("{hours:2}:{mins:02}, "));
        } else {
            s.push_str(&format!("{hours:2}:{mins:02},  "));
        }
    } else if bsd {
        s.push_str(&format!("{mins} min{}, ", if mins == 1 { "" } else { "s" }));
    } else {
        s.push_str(&format!("{mins} min,  "));
    }
    let plural = if users == 1 { "" } else { "s" };
    if bsd {
        s.push_str(&format!(
            "{users} user{plural}, load averages: {:.2} {:.2} {:.2}",
            load[0], load[1], load[2]
        ));
    } else {
        s.push_str(&format!(
            "{users} user{plural},  load average: {:.2}, {:.2}, {:.2}",
            load[0], load[1], load[2]
        ));
    }
    s
}

fn uptime_secs() -> std::io::Result<u64> {
    #[cfg(target_os = "macos")]
    {
        let boot = crate::sys::macos::boottime()?;
        Ok((common::now().0 - boot).max(0) as u64)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let t = crate::sys::linux::read("/proc/uptime")?;
        t.split_whitespace()
            .next()
            .and_then(|v| v.parse::<f64>().ok())
            .map(|v| v as u64)
            .ok_or_else(|| std::io::Error::other("bad /proc/uptime"))
    }
}

fn loadavg() -> std::io::Result<[f64; 3]> {
    #[cfg(not(target_os = "macos"))]
    if let Ok(t) = crate::sys::linux::read("/proc/loadavg") {
        let v: Vec<f64> = t
            .split_whitespace()
            .take(3)
            .filter_map(|x| x.parse().ok())
            .collect();
        if v.len() == 3 {
            return Ok([v[0], v[1], v[2]]);
        }
    }
    common::loadavg().ok_or_else(|| std::io::Error::other("no load average"))
}

pub fn uptime(ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    let (now, _) = common::now();
    let bsd = cfg!(target_os = "macos");
    let clock = common::strftime(if bsd { "%H:%M" } else { "%H:%M:%S" }, now, false);
    let users = ctx.users();
    w.line(format_uptime(
        &clock,
        uptime_secs()?,
        users,
        loadavg()?,
        bsd,
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_layout() {
        let s = format_uptime(
            "15:09:34",
            86400 + 4 * 3600 + 5 * 60,
            2,
            [0.15, 0.1, 0.05],
            false,
        );
        assert_eq!(
            s,
            " 15:09:34 up 1 day,  4:05,  2 users,  load average: 0.15, 0.10, 0.05"
        );
        let s = format_uptime("15:09:34", 5 * 60, 1, [0.0, 0.0, 0.0], false);
        assert_eq!(
            s,
            " 15:09:34 up 5 min,  1 user,  load average: 0.00, 0.00, 0.00"
        );
        let s = format_uptime(
            "15:09:34",
            178 * 86400 + 9 * 3600 + 15 * 60,
            0,
            [1.0, 2.0, 3.0],
            false,
        );
        assert!(s.contains("up 178 days,  9:15,"));
    }

    #[test]
    fn bsd_layout() {
        let s = format_uptime(
            "15:01",
            3 * 86400 + 4 * 3600 + 5 * 60,
            2,
            [1.2, 1.3, 1.4],
            true,
        );
        assert_eq!(
            s,
            "15:01  up 3 days,  4:05, 2 users, load averages: 1.20 1.30 1.40"
        );
    }
}
