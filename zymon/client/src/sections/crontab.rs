//! zymon extra: informational only, no client-side alerting.

use super::{Ctx, Res};
use zymon_core::proto::SectionWriter;

/// Comments and blank lines removed; environment assignments and jobs kept.
pub fn strip(text: &str) -> String {
    let mut s = String::new();
    for l in text.lines() {
        let t = l.trim();
        if !t.is_empty() && !t.starts_with('#') {
            s.push_str(t);
            s.push('\n');
        }
    }
    s
}

pub fn crontab(ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    let mut files = vec!["/etc/crontab".to_string()];
    if let Ok(rd) = std::fs::read_dir("/etc/cron.d") {
        let mut v: Vec<String> = rd
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .map(|e| e.path().to_string_lossy().into_owned())
            .collect();
        v.sort();
        files.extend(v);
    }
    for f in files {
        if let Ok(b) = std::fs::read(&f) {
            w.line(format!("# file: {f}"));
            let body = strip(&String::from_utf8_lossy(&b));
            if !body.is_empty() {
                w.line(body.trim_end());
            }
        }
    }
    if let Ok(o) = ctx.exec("crontab", &["-l"]) {
        let t = strip(&String::from_utf8_lossy(&o));
        if !t.is_empty() {
            w.line("# file: crontab -l");
            w.line(t.trim_end());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_comments_and_blanks() {
        let t = "# m h dom\nSHELL=/bin/sh\n\n  17 *\t* * *\troot    cd / && run-parts\n   # indented comment\n";
        assert_eq!(
            strip(t),
            "SHELL=/bin/sh\n17 *\t* * *\troot    cd / && run-parts\n"
        );
    }
}
