use super::{Ctx, Res, mount};
use crate::sys::common::{self, StatVfs};
use crate::sys::linux::{self, MountEnt};
use std::collections::{HashMap, HashSet};
use zymon_core::proto::SectionWriter;

/// Mounts that `df -Pl -x <nodev types> -x iso9660` (Linux) or the darwin script's
/// `nobrowse|afs|read-only` filter would list, with their statvfs data.
pub fn filesystems(ctx: &mut Ctx) -> std::io::Result<Vec<(MountEnt, StatVfs)>> {
    let mounts = mount::list(ctx)?;
    let nodev = if cfg!(target_os = "macos") {
        HashSet::new()
    } else {
        linux::nodev_types(&linux::read("/proc/filesystems").unwrap_or_default())
    };
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<(MountEnt, StatVfs)> = Vec::new();
    for mut m in mounts {
        if cfg!(target_os = "macos") {
            let o = &m.opts;
            // The sealed system root is read-only; keep it, it is the filesystem people mean by "/".
            // Remote filesystems are skipped like `df -l` does on Linux: statvfs on a slow NFS mount blocks.
            let skip = m.mountpoint != "/"
                && (m.fstype == "afs"
                    || o.contains("nobrowse")
                    || o.contains("read-only")
                    || !o.split(", ").any(|f| f == "local"));
            if skip {
                continue;
            }
        } else if nodev.contains(&m.fstype) || m.fstype == "iso9660" {
            continue;
        }
        let Some(sv) = common::statvfs(&m.mountpoint) else {
            continue;
        };
        if sv.blocks == 0 {
            continue;
        }
        // df lists a device once even when it is mounted several times (bind mounts); the shortest mount point wins.
        if m.source.starts_with('/') {
            if let Some(&i) = seen.get(&m.source) {
                if m.mountpoint.len() < out[i].0.mountpoint.len() {
                    out[i] = (m, sv);
                }
                continue;
            }
            seen.insert(m.source.clone(), out.len());
        }
        if m.source == "rootfs" {
            m.source = std::fs::canonicalize("/dev/root")
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "/dev/root".into());
        }
        out.push((m, sv));
    }
    Ok(out)
}

/// Fixed-width table: the first `left` columns left-aligned, the rest right-aligned, last column free.
pub fn table(header: &[&str], rows: &[Vec<String>], left: usize, min_first: usize) -> String {
    let n = header.len();
    let mut width: Vec<usize> = header.iter().map(|h| h.len()).collect();
    width[0] = width[0].max(min_first);
    for r in rows {
        for (i, c) in r.iter().enumerate().take(n - 1) {
            width[i] = width[i].max(c.len());
        }
    }
    let line = |cells: &[&str]| {
        let mut s = String::new();
        for (i, c) in cells.iter().enumerate() {
            if i == n - 1 {
                s.push_str(c);
            } else if i < left {
                s.push_str(&format!("{c:<w$} ", w = width[i]));
            } else {
                s.push_str(&format!("{c:>w$} ", w = width[i]));
            }
        }
        s.push('\n');
        s
    };
    let mut out = line(header);
    for r in rows {
        let cells: Vec<&str> = r.iter().map(String::as_str).collect();
        out.push_str(&line(&cells));
    }
    out
}

/// POSIX capacity: used / (used + available), rounded up.
pub fn pct(used: u64, avail: u64) -> String {
    let total = used + avail;
    if total == 0 {
        return "-".into();
    }
    format!("{}%", (used * 100).div_ceil(total))
}

/// The server splits on whitespace, so names must not contain any (the darwin script does the same).
pub fn nospace(s: &str) -> String {
    s.replace(' ', "_")
}

pub fn render(fs: &[(MountEnt, StatVfs)]) -> String {
    render_for(fs, cfg!(target_os = "macos"))
}

/// `mac` only selects the header word (`Avail` vs `Available`).
pub fn render_for(fs: &[(MountEnt, StatVfs)], mac: bool) -> String {
    let rows: Vec<Vec<String>> = fs
        .iter()
        .map(|(m, sv)| {
            let kb = |b: u64| (b * sv.frsize / 1024).to_string();
            let used = sv.blocks.saturating_sub(sv.bfree);
            vec![
                nospace(&m.source),
                kb(sv.blocks),
                kb(used),
                kb(sv.bavail),
                pct(used, sv.bavail),
                nospace(&m.mountpoint),
            ]
        })
        .collect();
    // "Mounted on" is two header words; the table treats the pair as one last column.
    let hdr = [
        "Filesystem",
        "1024-blocks",
        "Used",
        if mac { "Avail" } else { "Available" },
        "Capacity",
        "Mounted on",
    ];
    table(&hdr, &rows, 1, 14)
}

pub fn df(ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    let fs = filesystems(ctx)?;
    std::io::Write::write_all(w, render(&fs).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ent(src: &str, mp: &str) -> MountEnt {
        MountEnt {
            source: src.into(),
            mountpoint: mp.into(),
            fstype: "ext4".into(),
            opts: "rw".into(),
        }
    }

    #[test]
    fn capacity_rounds_up() {
        assert_eq!(pct(64, 36), "64%");
        assert_eq!(pct(1, 2), "34%");
        assert_eq!(pct(0, 0), "-");
    }

    #[test]
    fn layout_has_server_columns() {
        let sv = StatVfs {
            frsize: 4096,
            blocks: 1000,
            bfree: 400,
            bavail: 350,
            files: 0,
            ffree: 0,
        };
        let t = render(&[
            (ent("/dev/root", "/"), sv),
            (ent("/dev/sd a1", "/mnt/a b"), sv),
        ]);
        let lines: Vec<&str> = t.lines().collect();
        let hdr: Vec<&str> = lines[0].split_whitespace().collect();
        assert_eq!(hdr[0], "Filesystem");
        let cols: Vec<&str> = lines[1].split_whitespace().collect();
        // legacy do_disk.c reads: 2=used 4=capacity 5=mount point
        assert_eq!(cols.len(), 6);
        assert_eq!(cols[1], "4000");
        assert_eq!(cols[2], "2400");
        assert_eq!(cols[3], "1400");
        assert_eq!(cols[4], "64%");
        assert_eq!(cols[5], "/");
        assert!(lines[2].contains("/dev/sd_a1"));
        assert!(lines[2].ends_with("/mnt/a_b"));
        if !cfg!(target_os = "macos") {
            assert!(lines[0].contains("Available"));
        }
    }
}
