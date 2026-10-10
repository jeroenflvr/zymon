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
        // `vm_stat` reports free pages without the speculative ones it lists separately.
        let values: [u64; 22] = [
            v.free.saturating_sub(v.speculative),
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
