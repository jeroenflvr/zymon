use super::{Ctx, Res};
use crate::sys::common;
use zymon_core::proto::SectionWriter;

/// Exactly the three lines `logfetch --clock` prints; the server uses `epoch:` for clock-skew.
pub fn clock(_ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    let (s, us) = common::now();
    w.line(format!("epoch: {s}.{us:06}"));
    w.line(common::strftime("local: %Y-%m-%d %H:%M:%S %Z", s, false));
    w.line(common::strftime("UTC: %Y-%m-%d %H:%M:%S %Z", s, true));
    Ok(())
}
