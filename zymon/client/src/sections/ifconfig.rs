use super::{Ctx, Res};
use std::io::Write;
use zymon_core::proto::SectionWriter;

/// Legacy layout comes from the real tool; where it is not installed the section is simply absent
/// (`[ifstat]` carries the counters the server actually uses).
pub fn ifconfig(ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    let args: &[&str] = if cfg!(target_os = "macos") {
        &["-a"]
    } else {
        &[]
    };
    let out = ctx.exec("ifconfig", args)?;
    w.write_all(&out)
}
