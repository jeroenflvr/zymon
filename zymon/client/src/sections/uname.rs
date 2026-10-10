use super::{Ctx, Res};
use crate::sys::common;
use zymon_core::proto::SectionWriter;

/// Linux: `uname -rsmn` (fixed output order); macOS: `uname -a`.
pub fn uname(_ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    let u = common::uname()?;
    if cfg!(target_os = "macos") {
        w.line(format!(
            "{} {} {} {} {}",
            u.sysname, u.nodename, u.release, u.version, u.machine
        ));
    } else {
        w.line(format!(
            "{} {} {} {}",
            u.sysname, u.nodename, u.release, u.machine
        ));
    }
    Ok(())
}
