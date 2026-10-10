use super::df::{filesystems, nospace, pct, table};
use super::{Ctx, Res};
use crate::sys::common::StatVfs;
use crate::sys::linux::MountEnt;
use zymon_core::proto::SectionWriter;

/// Linux: `df -Pi` layout. macOS: the darwin script's 5-column rewrite (`itotal iused ifree %iused Mounted`,
/// only filesystems with used inodes).
pub fn render(fs: &[(MountEnt, StatVfs)]) -> String {
    render_for(fs, cfg!(target_os = "macos"))
}

pub fn render_for(fs: &[(MountEnt, StatVfs)], mac: bool) -> String {
    let mut rows = Vec::new();
    for (m, sv) in fs {
        let used = sv.files.saturating_sub(sv.ffree);
        if mac && used == 0 {
            continue;
        }
        let total = if mac { used + sv.ffree } else { sv.files };
        rows.push(vec![
            nospace(&m.source),
            total.to_string(),
            used.to_string(),
            sv.ffree.to_string(),
            pct(used, sv.ffree),
            nospace(&m.mountpoint),
        ]);
    }
    if mac {
        table(
            &[
                "Filesystem",
                "itotal",
                "iused",
                "ifree",
                "%iused",
                "Mounted",
            ],
            &rows,
            1,
            20,
        )
    } else {
        table(
            &[
                "Filesystem",
                "Inodes",
                "IUsed",
                "IFree",
                "IUse%",
                "Mounted on",
            ],
            &rows,
            1,
            14,
        )
    }
}

pub fn inode(ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    let fs = filesystems(ctx)?;
    std::io::Write::write_all(w, render(&fs).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_has_server_columns() {
        let m = MountEnt {
            source: "/dev/root".into(),
            mountpoint: "/".into(),
            fstype: "ext4".into(),
            opts: "rw".into(),
        };
        let sv = StatVfs {
            frsize: 4096,
            blocks: 10,
            bfree: 5,
            bavail: 5,
            files: 1000,
            ffree: 900,
        };
        let t = render(&[(m, sv)]);
        let lines: Vec<&str> = t.lines().collect();
        let h: Vec<&str> = lines[0].split_whitespace().collect();
        let want_free = if cfg!(target_os = "macos") {
            "ifree"
        } else {
            "IFree"
        };
        let want_pct = if cfg!(target_os = "macos") {
            "%iused"
        } else {
            "IUse%"
        };
        assert!(h.contains(&want_free) && h.contains(&want_pct) && h.contains(&"Mounted"));
        let c: Vec<&str> = lines[1].split_whitespace().collect();
        assert_eq!(c[2], "100");
        assert_eq!(c[3], "900");
        assert_eq!(c[4], "10%");
    }
}
