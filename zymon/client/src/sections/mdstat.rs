use super::{Ctx, Res, unsupported};
use zymon_core::proto::SectionWriter;

/// Raw /proc/mdstat, only if it exists.
pub fn mdstat(_ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    if !cfg!(target_os = "linux") {
        return unsupported();
    }
    match std::fs::read("/proc/mdstat") {
        Ok(b) => std::io::Write::write_all(w, &b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
