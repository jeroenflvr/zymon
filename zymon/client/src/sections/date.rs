use super::{Ctx, Res};
use crate::sys::common;
use zymon_core::proto::SectionWriter;

pub fn date(_ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    let (now, _) = common::now();
    w.line(common::strftime("%a %b %e %H:%M:%S %Z %Y", now, false));
    Ok(())
}
