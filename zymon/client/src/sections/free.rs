use super::{Ctx, Res, unsupported};
use crate::sys::linux::{meminfo_kb, read};
use zymon_core::proto::SectionWriter;

/// procps-ng `free` (no flags, kB). The server (linux.c) needs "\nMem:" with six numbers and a header
/// ending in "available\n"; it divides by 1024 itself.
pub fn render(meminfo: &str) -> String {
    let g = |k: &str| meminfo_kb(meminfo, k).unwrap_or(0);
    let total = g("MemTotal");
    let free = g("MemFree");
    let buffcache = g("Buffers") + g("Cached") + g("SReclaimable");
    let shared = g("Shmem");
    let used = total.saturating_sub(free + buffcache);
    let available = meminfo_kb(meminfo, "MemAvailable").unwrap_or(free + buffcache);
    let swap_total = g("SwapTotal");
    let swap_free = g("SwapFree");
    let mut s = format!(
        "{:<7}{:>11} {:>11} {:>11} {:>11} {:>11} {:>11}\n",
        "", "total", "used", "free", "shared", "buff/cache", "available"
    );
    s.push_str(&format!(
        "{:<7}{total:>11} {used:>11} {free:>11} {shared:>11} {buffcache:>11} {available:>11}\n",
        "Mem:"
    ));
    s.push_str(&format!(
        "{:<7}{swap_total:>11} {:>11} {swap_free:>11}\n",
        "Swap:",
        swap_total.saturating_sub(swap_free)
    ));
    s
}

pub fn free(_ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    if !cfg!(target_os = "linux") {
        return unsupported();
    }
    let t = read("/proc/meminfo")?;
    std::io::Write::write_all(w, render(&t).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEMINFO: &str = "MemTotal:         949440 kB\nMemFree:           93408 kB\nMemAvailable:     520000 kB\nBuffers:           50000 kB\nCached:           300000 kB\nShmem:              6000 kB\nSwapTotal:        102396 kB\nSwapFree:         100000 kB\nSReclaimable:      20000 kB\n";

    #[test]
    fn new_style_layout() {
        let t = render(MEMINFO);
        assert!(t.lines().next().unwrap().ends_with("available"));
        let mem = t.lines().find(|l| l.starts_with("Mem:")).unwrap();
        let n: Vec<u64> = mem
            .split_whitespace()
            .skip(1)
            .map(|v| v.parse().unwrap())
            .collect();
        // total used free shared buff/cache available
        assert_eq!(
            n,
            vec![949440, 949440 - 93408 - 370000, 93408, 6000, 370000, 520000]
        );
        let swap = t.lines().find(|l| l.starts_with("Swap:")).unwrap();
        let n: Vec<u64> = swap
            .split_whitespace()
            .skip(1)
            .map(|v| v.parse().unwrap())
            .collect();
        assert_eq!(n, vec![102396, 2396, 100000]);
    }

    /// The exact checks of xymond/client/linux.c: "available\n" present, "\nMem: %ld*6", "\nSwap: %ld*3".
    #[test]
    fn legacy_server_accepts() {
        let t = render(MEMINFO);
        assert!(t.contains("available\n"));
        assert!(!t.contains("\n-/+ buffers/cache:"));
        let p = &t[t.find("\nMem:").unwrap() + 1..];
        let nums: Vec<i64> = p
            .split_whitespace()
            .skip(1)
            .take(6)
            .filter_map(|v| v.parse().ok())
            .collect();
        assert_eq!(nums.len(), 6);
        assert!(t.contains("\nSwap:"));
    }
}
