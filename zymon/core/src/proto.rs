//! Legacy xymond wire messages. Bodies are bytes, never `String`: `ps`/`who` output is not guaranteed UTF-8.

use std::io;

/// `client[/COLLECTORID] MACHINE.OSTAG CLASS\n`. `machine` must already be commafied.
/// Only the default collector (no id) may carry a class.
pub fn client_header(collector: Option<&str>, machine: &str, os: &str, class: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(32 * 1024);
    v.extend_from_slice(b"client");
    match collector {
        Some(id) => {
            v.push(b'/');
            v.extend_from_slice(id.as_bytes());
            v.extend_from_slice(format!(" {machine}.{os}\n").as_bytes());
        }
        None => v.extend_from_slice(format!(" {machine}.{os} {class}\n").as_bytes()),
    }
    v
}

/// Writes one `[name]` section into a message buffer. The header is emitted on the first
/// byte, so a collector that produces nothing leaves no trace (legacy conditional sections).
pub struct SectionWriter<'a> {
    out: &'a mut Vec<u8>,
    name: &'a str,
    start: usize,
    started: bool,
    at_bol: bool,
}

impl<'a> SectionWriter<'a> {
    pub fn new(out: &'a mut Vec<u8>, name: &'a str) -> Self {
        let start = out.len();
        SectionWriter {
            out,
            name,
            start,
            started: false,
            at_bol: true,
        }
    }

    /// Emit the header now even if the body stays empty (sections the legacy script always sends, e.g. `[who]`).
    pub fn begin(&mut self) {
        if !self.started {
            self.out.push(b'[');
            self.out.extend_from_slice(self.name.as_bytes());
            self.out.extend_from_slice(b"]\n");
            self.started = true;
        }
    }

    /// Append `s` plus a newline.
    pub fn line(&mut self, s: impl AsRef<[u8]>) {
        let _ = io::Write::write_all(self, s.as_ref());
        let _ = io::Write::write_all(self, b"\n");
    }

    /// Terminate the last line if needed. Returns whether the section holds any data.
    pub fn finish(self) -> bool {
        if self.started && !self.at_bol {
            self.out.push(b'\n');
        }
        self.started
    }

    /// Drop everything this section wrote (collector failed half way).
    pub fn abort(self) {
        self.out.truncate(self.start);
    }
}

impl io::Write for SectionWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.begin();
        // A body line starting with '[' would be taken for a new section by xymond; indent it.
        let mut rest = buf;
        while !rest.is_empty() {
            if self.at_bol && rest[0] == b'[' {
                self.out.push(b' ');
            }
            match rest.iter().position(|&b| b == b'\n') {
                Some(i) => {
                    self.out.extend_from_slice(&rest[..=i]);
                    self.at_bol = true;
                    rest = &rest[i + 1..];
                }
                None => {
                    self.out.extend_from_slice(rest);
                    self.at_bol = false;
                    rest = &[];
                }
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color {
    Green,
    Yellow,
    Red,
    Clear,
}

impl Color {
    pub fn as_str(self) -> &'static str {
        match self {
            Color::Green => "green",
            Color::Yellow => "yellow",
            Color::Red => "red",
            Color::Clear => "clear",
        }
    }
}

/// `status[+LIFETIME] MACHINE.COLUMN COLOR first line\nbody\n`. `lifetime` is e.g. `"10m"`.
pub fn status(
    lifetime: Option<&str>,
    machine: &str,
    column: &str,
    color: Color,
    first_line: &str,
    body: &str,
) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"status");
    if let Some(l) = lifetime {
        v.push(b'+');
        v.extend_from_slice(l.as_bytes());
    }
    v.extend_from_slice(
        format!(" {machine}.{column} {} {first_line}\n", color.as_str()).as_bytes(),
    );
    v.extend_from_slice(body.as_bytes());
    if !body.is_empty() && !body.ends_with('\n') {
        v.push(b'\n');
    }
    v
}

/// `data MACHINE.DATANAME\nbody`: RRD-only data message.
pub fn data(machine: &str, name: &str, body: &str) -> Vec<u8> {
    format!("data {machine}.{name}\n{body}").into_bytes()
}

/// Several status messages in one connection: `combo\n` + messages separated by a blank line.
pub fn combo(messages: &[Vec<u8>]) -> Vec<u8> {
    let mut v = b"combo\n".to_vec();
    for (i, m) in messages.iter().enumerate() {
        if i > 0 {
            v.push(b'\n');
        }
        v.extend_from_slice(m);
        if !m.ends_with(b"\n") {
            v.push(b'\n');
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn header() {
        assert_eq!(
            client_header(None, "pi4,lan", "linux", "rpi"),
            b"client pi4,lan.linux rpi\n"
        );
        assert_eq!(
            client_header(Some("zymon"), "pi4", "linux", "ignored"),
            b"client/zymon pi4.linux\n"
        );
    }

    #[test]
    fn lazy_header() {
        let mut buf = Vec::new();
        let w = SectionWriter::new(&mut buf, "empty");
        assert!(!w.finish());
        assert!(buf.is_empty());

        let mut w = SectionWriter::new(&mut buf, "date");
        w.line("hello");
        write!(w, "no newline").unwrap();
        assert!(w.finish());
        assert_eq!(buf, b"[date]\nhello\nno newline\n");
    }

    #[test]
    fn begin_forces_empty_section() {
        let mut buf = Vec::new();
        let mut w = SectionWriter::new(&mut buf, "who");
        w.begin();
        assert!(w.finish());
        assert_eq!(buf, b"[who]\n");
    }

    #[test]
    fn abort_rolls_back() {
        let mut buf = b"[a]\nx\n".to_vec();
        let mut w = SectionWriter::new(&mut buf, "b");
        w.line("partial");
        w.abort();
        assert_eq!(buf, b"[a]\nx\n");
    }

    #[test]
    fn bracket_lines_are_indented() {
        let mut buf = Vec::new();
        let mut w = SectionWriter::new(&mut buf, "ps");
        w.write_all(b"ok\n[evil]\nmid [x]\n").unwrap();
        w.finish();
        assert_eq!(buf, b"[ps]\nok\n [evil]\nmid [x]\n");
    }

    #[test]
    fn non_utf8_passes_through() {
        let mut buf = Vec::new();
        let mut w = SectionWriter::new(&mut buf, "who");
        w.line([0xff, 0xfe, b'x']);
        w.finish();
        assert_eq!(buf, b"[who]\n\xff\xfex\n");
    }

    #[test]
    fn status_and_combo() {
        let s = status(
            Some("10m"),
            "pi4",
            "docker",
            Color::Red,
            "web down",
            "web: exited",
        );
        assert_eq!(s, b"status+10m pi4.docker red web down\nweb: exited\n");
        let c = combo(&[s.clone(), s]);
        assert_eq!(
            c,
            b"combo\nstatus+10m pi4.docker red web down\nweb: exited\n\nstatus+10m pi4.docker red web down\nweb: exited\n"
                .to_vec()
        );
    }
}
