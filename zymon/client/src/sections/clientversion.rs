use super::{Ctx, Res};
use zymon_core::proto::SectionWriter;

pub const VERSION: &str = concat!("zymon-client/", env!("CARGO_PKG_VERSION"));

pub fn clientversion(_ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    w.line(VERSION);
    Ok(())
}
