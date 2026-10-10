use super::{Ctx, Res};
use crate::sys::linux::{self, MountEnt};
use std::io::Write;
use zymon_core::proto::SectionWriter;

/// macOS `mount` line: `/dev/disk3s1 on /System/Volumes/Data (apfs, local, journaled)`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn parse_mount_cmd(text: &str) -> Vec<MountEnt> {
    text.lines()
        .filter_map(|l| {
            let (source, rest) = l.split_once(" on ")?;
            let (mp, opts) = rest.rsplit_once(" (")?;
            let opts = opts.strip_suffix(')')?;
            let (fstype, flags) = opts.split_once(", ").unwrap_or((opts, ""));
            Some(MountEnt {
                source: source.to_string(),
                mountpoint: mp.to_string(),
                fstype: fstype.to_string(),
                opts: flags.to_string(),
            })
        })
        .collect()
}

/// All mounts, from /proc/self/mounts or the `mount` command.
pub fn list(ctx: &mut Ctx) -> std::io::Result<Vec<MountEnt>> {
    if cfg!(target_os = "macos") {
        let out = ctx.exec("mount", &[])?;
        Ok(parse_mount_cmd(&String::from_utf8_lossy(&out)))
    } else {
        Ok(linux::parse_mounts(&linux::read("/proc/self/mounts")?))
    }
}

/// `mount(8)` layout for the Linux section.
pub fn render_linux(mounts: &[MountEnt]) -> String {
    let mut s = String::new();
    for m in mounts {
        s.push_str(&format!(
            "{} on {} type {} ({})\n",
            m.source, m.mountpoint, m.fstype, m.opts
        ));
    }
    s
}

pub fn mount(ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    if cfg!(target_os = "macos") {
        let out = ctx.exec("mount", &[])?;
        w.write_all(&out)
    } else {
        w.write_all(render_linux(&list(ctx)?).as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_mount_parse() {
        let t = "/dev/disk3s1s1 on / (apfs, sealed, local, read-only, journaled)\n/dev/disk3s5 on /System/Volumes/Data (apfs, local, journaled, protected)\nmap auto_home on /System/Volumes/Data/home (autofs, automounted, nobrowse)\n";
        let m = parse_mount_cmd(t);
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].mountpoint, "/");
        assert!(m[0].opts.contains("read-only"));
        assert_eq!(m[2].fstype, "autofs");
        assert_eq!(m[2].source, "map auto_home");
    }

    #[test]
    fn linux_mount_layout() {
        let m = linux::parse_mounts("/dev/root / ext4 rw,noatime 0 0\n");
        assert_eq!(render_linux(&m), "/dev/root on / type ext4 (rw,noatime)\n");
    }
}
