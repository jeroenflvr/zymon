use super::{Ctx, Res, unsupported};
use zymon_core::proto::SectionWriter;

/// Release files tried after os-release, in the legacy script's order.
const RELEASE_FILES: &[&str] = &[
    "/etc/redhat-release",
    "/etc/gentoo-release",
    "/etc/SuSE-release",
    "/etc/SUSE-release",
    "/etc/slackware-version",
    "/etc/mandrake-release",
    "/etc/fedora-release",
    "/etc/arch-release",
];

pub fn pretty_name(os_release: &str) -> Option<String> {
    os_release.lines().find_map(|l| {
        let v = l.strip_prefix("PRETTY_NAME=")?;
        Some(v.trim().trim_matches(['"', '\'']).to_string())
    })
}

pub fn osversion(_ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    if !cfg!(target_os = "linux") {
        return unsupported();
    }
    for p in ["/etc/os-release", "/usr/lib/os-release"] {
        if let Ok(t) = std::fs::read_to_string(p)
            && let Some(n) = pretty_name(&t)
            && !n.is_empty()
        {
            w.line(n);
            return Ok(());
        }
    }
    for p in RELEASE_FILES {
        if let Ok(t) = std::fs::read_to_string(p)
            && let Some(l) = t.lines().next()
        {
            w.line(l);
            return Ok(());
        }
    }
    if let Ok(t) = std::fs::read_to_string("/etc/debian_version")
        && let Some(l) = t.lines().next()
    {
        w.line(format!("Debian {l}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty() {
        let t = "NAME=\"Debian GNU/Linux\"\nPRETTY_NAME=\"Debian GNU/Linux 12 (bookworm)\"\nID=debian\n";
        assert_eq!(
            pretty_name(t).as_deref(),
            Some("Debian GNU/Linux 12 (bookworm)")
        );
        assert_eq!(pretty_name("ID=x\n"), None);
    }
}
