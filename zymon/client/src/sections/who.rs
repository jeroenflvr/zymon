use super::{Ctx, Res};
use std::io::Write;
use zymon_core::proto::SectionWriter;

pub fn who(ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    let out = ctx.who().to_vec();
    // The legacy script always sends the header, even with nobody logged in.
    w.begin();
    w.write_all(&out)
}
