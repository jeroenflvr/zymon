//! `[top]` is display-only (appended to the cpu status). `top` itself is never executed: the header
//! comes from /proc (Linux) or mach (macOS), the process list from the `[ps]` capture.

use super::uptime::format_uptime;
use super::{Ctx, Res, ps};
use crate::state::CpuTimes;
use crate::sys::common;
use std::io::Write;
use zymon_core::proto::SectionWriter;

/// Percent of the interval spent in us, sy, ni, id, wa, hi, si, st. `prev` all-zero = since boot.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn cpu_percent(prev: &CpuTimes, cur: &CpuTimes) -> [f64; 8] {
    // /proc/stat order is user nice system idle iowait irq softirq steal
    let d: Vec<u64> = (0..8).map(|i| cur[i].saturating_sub(prev[i])).collect();
    let total: u64 = d.iter().sum();
    let p = |v: u64| {
        if total == 0 {
            0.0
        } else {
            v as f64 * 100.0 / total as f64
        }
    };
    [
        p(d[0]),
        p(d[2]),
        p(d[1]),
        p(d[3]),
        p(d[4]),
        p(d[5]),
        p(d[6]),
        p(d[7]),
    ]
}

fn top_rows(rows: &[ps::Row], n: usize) -> Vec<&ps::Row> {
    let mut v: Vec<&ps::Row> = rows.iter().collect();
    v.sort_by(|a, b| {
        b.pcpu
            .partial_cmp(&a.pcpu)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.pid.len().cmp(&b.pid.len()))
            .then_with(|| a.pid.cmp(&b.pid))
    });
    v.truncate(n);
    v
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn task_counts(rows: &[ps::Row]) -> (usize, usize, usize, usize, usize) {
    let (mut run, mut sleep, mut stop, mut zomb) = (0, 0, 0, 0);
    for r in rows {
        match r.state.chars().next() {
            Some('R') => run += 1,
            Some('T') | Some('t') => stop += 1,
            Some('Z') => zomb += 1,
            _ => sleep += 1,
        }
    }
    (rows.len(), run, sleep, stop, zomb)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn render_linux(
    clock: &str,
    up_secs: u64,
    users: usize,
    load: [f64; 3],
    cpu: [f64; 8],
    meminfo: &str,
    rows: Option<&[ps::Row]>,
) -> String {
    use crate::sys::linux::meminfo_kb;
    let mut s = format!(
        "top -{}\n",
        format_uptime(clock, up_secs, users, load, false)
    );
    if let Some(rows) = rows {
        let (t, r, sl, st, z) = task_counts(rows);
        s.push_str(&format!(
            "Tasks: {t:3} total, {r:3} running, {sl:3} sleeping, {st:3} stopped, {z:3} zombie\n"
        ));
    }
    s.push_str(&format!(
        "%Cpu(s): {:4.1} us, {:4.1} sy, {:4.1} ni, {:4.1} id, {:4.1} wa, {:4.1} hi, {:4.1} si, {:4.1} st\n",
        cpu[0], cpu[1], cpu[2], cpu[3], cpu[4], cpu[5], cpu[6], cpu[7]
    ));
    let g = |k: &str| meminfo_kb(meminfo, k).unwrap_or(0);
    let mib = |kb: u64| kb as f64 / 1024.0;
    let total = g("MemTotal");
    let free = g("MemFree");
    let bc = g("Buffers") + g("Cached") + g("SReclaimable");
    let used = total.saturating_sub(free + bc);
    s.push_str(&format!(
        "MiB Mem : {:8.1} total, {:8.1} free, {:8.1} used, {:8.1} buff/cache\n",
        mib(total),
        mib(free),
        mib(used),
        mib(bc)
    ));
    let st = g("SwapTotal");
    let sf = g("SwapFree");
    let avail = meminfo_kb(meminfo, "MemAvailable").unwrap_or(free + bc);
    s.push_str(&format!(
        "MiB Swap: {:8.1} total, {:8.1} free, {:8.1} used. {:8.1} avail Mem \n",
        mib(st),
        mib(sf),
        mib(st.saturating_sub(sf)),
        mib(avail)
    ));
    if let Some(rows) = rows {
        s.push_str(
            "\n    PID USER      PR  NI    VIRT    RES    SHR S  %CPU  %MEM     TIME+ COMMAND\n",
        );
        for r in top_rows(rows, 10) {
            s.push_str(&format!(
                "{:>7} {:<9.9} {:>3} {:>3} {:>7} {:>6} {:>6} {} {:>5.1} {:>5} {:>9} {}\n",
                r.pid,
                r.user,
                r.pri,
                "-",
                r.vsz,
                r.rss,
                "-",
                r.state.chars().next().unwrap_or('?'),
                r.pcpu,
                r.pmem,
                r.time,
                r.cmd
            ));
        }
    }
    s
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
fn size_str(bytes: u64) -> String {
    const G: u64 = 1 << 30;
    if bytes >= G {
        format!("{:.0}G", bytes as f64 / G as f64)
    } else {
        format!("{}M", bytes >> 20)
    }
}

/// macOS: the `Processes:`/`Load Avg:`/`CPU usage:`/`PhysMem:` lines of `top -l`, then the busiest processes.
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub fn render_mac(
    load: [f64; 3],
    cpu: [f64; 3],
    mem: (u64, u64, u64),
    rows: Option<&[ps::Row]>,
) -> String {
    let mut s = String::new();
    if let Some(rows) = rows {
        let (t, r, sl, _, _) = task_counts(rows);
        s.push_str(&format!(
            "Processes: {t} total, {r} running, {sl} sleeping\n"
        ));
    }
    s.push_str(&format!(
        "Load Avg: {:.2}, {:.2}, {:.2}\n",
        load[0], load[1], load[2]
    ));
    s.push_str(&format!(
        "CPU usage: {:.1}% user, {:.1}% sys, {:.1}% idle\n",
        cpu[0], cpu[1], cpu[2]
    ));
    let (used, wired, unused) = mem;
    s.push_str(&format!(
        "PhysMem: {} used ({} wired), {} unused.\n",
        size_str(used),
        size_str(wired),
        size_str(unused)
    ));
    if let Some(rows) = rows {
        s.push_str("\nPID    COMMAND          %CPU TIME\n");
        for r in top_rows(rows, 10) {
            let name = r.cmd.rsplit('/').next().unwrap_or(&r.cmd);
            s.push_str(&format!(
                "{:<6} {:<16.16} {:<4.1} {}\n",
                r.pid, name, r.pcpu, r.time
            ));
        }
    }
    s
}

pub fn top(ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    let rows = ctx.ps.as_deref().map(ps::parse);
    #[cfg(target_os = "macos")]
    {
        use crate::sys::macos;
        let zero = [0u64; 8];
        let cur = ctx.cur.as_ref().map(|s| s.cpu).unwrap_or(zero);
        let prev = ctx.prev.as_ref().map(|s| s.cpu).unwrap_or(zero);
        // macOS ticks are filled as user nice system idle, the /proc/stat order
        let p = cpu_percent(&prev, &cur);
        let vm = macos::vm_stats()?;
        let page = vm.page_size;
        let memsize = macos::sysctl_u64("hw.memsize").unwrap_or(0);
        let unused = vm.free.saturating_sub(vm.speculative) * page;
        let wired = vm.wired * page;
        let load = common::loadavg().unwrap_or([0.0; 3]);
        w.write_all(
            render_mac(
                load,
                [p[0] + p[2], p[1], p[3]],
                (memsize.saturating_sub(unused), wired, unused),
                rows.as_deref(),
            )
            .as_bytes(),
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        use crate::sys::linux::read;
        let (now, _) = common::now();
        let clock = common::strftime("%H:%M:%S", now, false);
        let up = read("/proc/uptime")?
            .split_whitespace()
            .next()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0) as u64;
        let load = common::loadavg().unwrap_or([0.0; 3]);
        let zero = [0u64; 8];
        let cur = ctx.cur.as_ref().map(|s| s.cpu).unwrap_or(zero);
        let prev = ctx.prev.as_ref().map(|s| s.cpu).unwrap_or(zero);
        let users = ctx.users();
        let mem = read("/proc/meminfo")?;
        w.write_all(
            render_linux(
                &clock,
                up,
                users,
                load,
                cpu_percent(&prev, &cur),
                &mem,
                rows.as_deref(),
            )
            .as_bytes(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_delta() {
        let prev = [100, 0, 50, 800, 10, 0, 0, 0];
        let cur = [110, 0, 60, 880, 10, 0, 0, 0];
        let p = cpu_percent(&prev, &cur);
        assert!(
            (p[0] - 10.0).abs() < 1e-9 && (p[1] - 10.0).abs() < 1e-9 && (p[3] - 80.0).abs() < 1e-9
        );
        assert_eq!(cpu_percent(&cur, &cur), [0.0; 8]);
    }

    #[test]
    fn linux_header_lines() {
        let mi = "MemTotal: 1048576 kB\nMemFree: 524288 kB\nBuffers: 0 kB\nCached: 262144 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\nMemAvailable: 700000 kB\n";
        let cap = b"  PID  PPID USER STARTED S PRI %CPU TIME %MEM RSZ VSZ CMD\n 1 0 root Oct09 S 19 0.1 00:00:03 0.5 4000 16000 /sbin/init\n 2 1 pi 10:00 R 19 9.0 00:00:03 0.5 4000 16000 busy --loop\n";
        let rows = ps::parse(cap);
        let t = render_linux(
            "12:00:00",
            3600,
            1,
            [0.1, 0.2, 0.3],
            [1.0; 8],
            mi,
            Some(&rows),
        );
        let l: Vec<&str> = t.lines().collect();
        assert!(l[0].starts_with("top - 12:00:00 up  1:00,"));
        assert!(l[1].starts_with("Tasks:"));
        assert!(l[2].starts_with("%Cpu(s):"));
        assert!(l[3].starts_with("MiB Mem :"));
        assert!(l[4].starts_with("MiB Swap:"));
        assert!(l[7].ends_with("busy --loop"), "busiest first: {}", l[7]);
    }

    #[test]
    fn mac_header_lines() {
        let t = render_mac(
            [1.0, 2.0, 3.0],
            [5.0, 3.0, 92.0],
            (8 << 30, 2 << 30, 512 << 20),
            None,
        );
        assert!(t.starts_with("Load Avg: 1.00, 2.00, 3.00\nCPU usage: 5.0% user, 3.0% sys, 92.0% idle\nPhysMem: 8G used (2G wired), 512M unused."));
    }
}
