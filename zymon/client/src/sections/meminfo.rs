use super::{Ctx, Res};
use zymon_core::proto::SectionWriter;

/// Row labels in `vm_stat` order.
pub const LABELS: [&str; 22] = [
    "Pages free",
    "Pages active",
    "Pages inactive",
    "Pages speculative",
    "Pages throttled",
    "Pages wired down",
    "Pages purgeable",
    "\"Translation faults\"",
    "Pages copy-on-write",
    "Pages zero filled",
    "Pages reactivated",
    "Pages purged",
    "File-backed pages",
    "Anonymous pages",
    "Pages stored in compressor",
    "Pages occupied by compressor",
    "Decompressions",
    "Compressions",
    "Pageins",
    "Pageouts",
    "Swapins",
    "Swapouts",
];

/// darwin.c takes physical memory as free+active+inactive+wired, but those four leave out speculative,
/// compressor and throttled pages, so a 16 GB Mac shows up as ~8 GB. Report "Pages free" as everything the
/// other three do not account for, which makes the server's total equal the installed RAM.
/// (Falls back to the plain `vm_stat` value when the RAM size is unknown.)
#[cfg(target_os = "macos")]
fn reported_free(total_pages: u64, v: &crate::sys::macos::VmStats) -> u64 {
    if total_pages == 0 {
        return v.free.saturating_sub(v.speculative);
    }
    adjusted_free(total_pages, v.active, v.inactive, v.wired)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn adjusted_free(total_pages: u64, active: u64, inactive: u64, wired: u64) -> u64 {
    total_pages.saturating_sub(active + inactive + wired)
}

/// `vm_stat` layout. The server reads the page size from the first line and `Pages free:`,
/// `Pages active:`, `Pages inactive:`, `Pages wired down:` (darwin.c).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn render(page_size: u64, rows: &[(&str, u64)]) -> String {
    let mut s = format!("Mach Virtual Memory Statistics: (page size of {page_size} bytes)\n");
    for (name, v) in rows {
        s.push_str(&format!("{:<35}{:>17}.\n", format!("{name}:"), v));
    }
    s
}

pub fn meminfo(_ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    #[cfg(target_os = "macos")]
    {
        let v = crate::sys::macos::vm_stats()?;
        let memsize = crate::sys::macos::sysctl_u64("hw.memsize").unwrap_or(0);
        let free = reported_free(memsize / v.page_size.max(1), &v);
        let values: [u64; 22] = [
            free,
            v.active,
            v.inactive,
            v.speculative,
            v.throttled,
            v.wired,
            v.purgeable,
            v.faults,
            v.cow,
            v.zero_fill,
            v.reactivated,
            v.purged,
            v.file_backed,
            v.anonymous,
            v.compressor_stored,
            v.compressor_pages,
            v.decompressions,
            v.compressions,
            v.pageins,
            v.pageouts,
            v.swapins,
            v.swapouts,
        ];
        let rows: Vec<(&str, u64)> = LABELS.iter().copied().zip(values).collect();
        std::io::Write::write_all(w, render(v.page_size, &rows).as_bytes())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = w;
        super::unsupported()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_total_equals_installed_ram() {
        // 16 GB, 16 KiB pages: darwin.c sums free+active+inactive+wired
        let total = 16u64 << 30 >> 14;
        let free = adjusted_free(total, 210_684, 208_720, 143_006);
        assert_eq!(free + 210_684 + 208_720 + 143_006, total);
        assert_eq!(adjusted_free(10, 8, 8, 8), 0);
    }

    #[test]
    fn darwin_c_checks() {
        let t = render(
            16384,
            &[
                ("Pages free", 3925),
                ("Pages active", 278863),
                ("Pages inactive", 273560),
                ("Pages wired down", 137603),
            ],
        );
        let p = t.find("page size of").unwrap();
        let ps: u64 = t[p + 13..]
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(ps, 16384);
        for k in [
            "\nPages free:",
            "\nPages active:",
            "\nPages inactive:",
            "\nPages wired down:",
        ] {
            let at = t.find(k).unwrap_or_else(|| panic!("{k}"));
            let v: u64 = t[at..][t[at..].find(':').unwrap() + 1..]
                .split('.')
                .next()
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            assert!(v > 0);
        }
    }
}
