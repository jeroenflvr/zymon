use super::{Ctx, Res};
use crate::sys::common;
use zymon_core::proto::SectionWriter;

pub fn nproc(_ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    w.line(common::nproc().to_string());
    Ok(())
}
