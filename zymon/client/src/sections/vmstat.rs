use super::{Ctx, Res, unsupported};
use crate::state::Snap;
use crate::sys::linux::{meminfo_kb, read};
use zymon_core::proto::SectionWriter;

/// One `vmstat`-layout line (17 numeric columns, `r b swpd free buff cache si so bi bo in cs us sy id wa st`)
/// computed from the counter deltas between two snapshots; rates are per second.
pub fn render(prev: &Snap, cur: &Snap, meminfo: &str, page_kb: u64) -> String {
    let dt = cur
        .at
        .saturating_duration_since(prev.at)
        .as_secs_f64()
        .max(0.001);
    let rate = |a: u64, b: u64| (b.saturating_sub(a) as f64 / dt).round() as u64;
    let g = |k: &str| meminfo_kb(meminfo, k).unwrap_or(0);
    let d: Vec<u64> = (0..8)
        .map(|i| cur.cpu[i].saturating_sub(prev.cpu[i]))
        .collect();
    let total: u64 = d.iter().sum();
    let pc = |v: u64| {
        if total == 0 {
            0
        } else {
            (v as f64 * 100.0 / total as f64).round() as u64
        }
    };
    // user nice system idle iowait irq softirq steal
    let (us, sy, id, wa, st) = (
        pc(d[0] + d[1]),
        pc(d[2] + d[5] + d[6]),
        pc(d[3]),
        pc(d[4]),
        pc(d[7]),
    );
    let swpd = g("SwapTotal").saturating_sub(g("SwapFree"));
    let mut s = String::from(
        "procs -----------memory---------- ---swap-- -----io---- -system-- ------cpu-----\n",
    );
    s.push_str(
        " r  b   swpd   free   buff  cache   si   so    bi    bo   in   cs us sy id wa st\n",
    );
    s.push_str(&format!(
        " {:2} {:2} {:6} {:6} {:6} {:6} {:4} {:4} {:5} {:5} {:4} {:4} {:2} {:2} {:2} {:2} {:2}\n",
        cur.procs_running.saturating_sub(1),
        cur.procs_blocked,
        swpd,
        g("MemFree"),
        g("Buffers"),
        g("Cached") + g("SReclaimable"),
        rate(prev.pswpin, cur.pswpin) * page_kb,
        rate(prev.pswpout, cur.pswpout) * page_kb,
        rate(prev.pgpgin, cur.pgpgin),
        rate(prev.pgpgout, cur.pgpgout),
        rate(prev.intr, cur.intr),
        rate(prev.ctxt, cur.ctxt),
        us,
        sy,
        id,
        wa,
        st
    ));
    s
}

/// No 5-minute background `vmstat`: the previous cycle's counters are the baseline, so the first
/// cycle after start emits nothing.
pub fn vmstat(ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    if !cfg!(target_os = "linux") {
        return unsupported();
    }
    let (Some(prev), Some(cur)) = (&ctx.prev, &ctx.cur) else {
        return Ok(());
    };
    let mem = read("/proc/meminfo")?;
    let page_kb = (crate::sys::common::page_size() / 1024).max(1);
    std::io::Write::write_all(w, render(prev, cur, &mem, page_kb).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn snap(at: Instant, cpu: [u64; 8], ctxt: u64, intr: u64, pgin: u64, pgout: u64) -> Snap {
        Snap {
            at,
            cpu,
            ctxt,
            intr,
            procs_running: 3,
            procs_blocked: 1,
            pgpgin: pgin,
            pgpgout: pgout,
            pswpin: 0,
            pswpout: 0,
        }
    }

    #[test]
    fn delta_math() {
        let t0 = Instant::now();
        let a = snap(t0, [100, 0, 50, 800, 10, 0, 0, 0], 1000, 5000, 100, 200);
        let b = snap(
            t0 + Duration::from_secs(10),
            [110, 0, 60, 870, 10, 0, 0, 0],
            1500,
            5100,
            200,
            400,
        );
        let mi = "MemFree: 100 kB\nBuffers: 10 kB\nCached: 20 kB\nSReclaimable: 5 kB\nSwapTotal: 50 kB\nSwapFree: 40 kB\n";
        let out = render(&a, &b, mi, 4);
        let last = out.lines().last().unwrap();
        let n: Vec<u64> = last
            .split_whitespace()
            .map(|v| v.parse().unwrap())
            .collect();
        // r b swpd free buff cache si so bi bo in cs us sy id wa st
        assert_eq!(
            n,
            vec![
                2, 1, 10, 100, 10, 25, 0, 0, 10, 20, 10, 50, 11, 11, 78, 0, 0
            ]
        );
    }

    /// do_vmstat.c reads the LAST line of the section as 17 whitespace separated numbers.
    #[test]
    fn last_line_is_17_numbers() {
        let t0 = Instant::now();
        let a = snap(t0, [1; 8], 1, 1, 1, 1);
        let b = snap(t0 + Duration::from_secs(1), [2; 8], 2, 2, 2, 2);
        let out = render(&a, &b, "", 4);
        assert!(out.ends_with('\n'));
        let last = out.lines().last().unwrap();
        assert_eq!(
            last.split_whitespace()
                .filter_map(|v| v.parse::<u64>().ok())
                .count(),
            17
        );
    }
}
