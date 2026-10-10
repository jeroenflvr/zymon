//! zymon extra: container inventory over the Docker unix socket (hand-rolled HTTP/1.1 + a tiny JSON
//! reader; no HTTP or JSON crates) and the client-side status for `docker.must_run`.

use super::{Ctx, Res};
use crate::config::Config;
use crate::sys::common;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;
use zymon_core::proto::{self, Color, SectionWriter};

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Container {
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
}

#[derive(Debug, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

struct P<'a> {
    b: &'a [u8],
    i: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> Option<()> {
        self.ws();
        (self.b.get(self.i) == Some(&c)).then(|| self.i += 1)
    }

    fn lit(&mut self, s: &str, v: Json) -> Option<Json> {
        self.b[self.i..].starts_with(s.as_bytes()).then(|| {
            self.i += s.len();
            v
        })
    }

    fn hex4(&mut self) -> Option<u32> {
        let h = self.b.get(self.i..self.i + 4)?;
        self.i += 4;
        u32::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok()
    }

    fn string(&mut self) -> Option<String> {
        self.eat(b'"')?;
        let mut out = Vec::new();
        loop {
            let c = *self.b.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    let e = *self.b.get(self.i)?;
                    self.i += 1;
                    let ch = match e {
                        b'"' | b'\\' | b'/' => e as char,
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let mut cp = self.hex4()?;
                            if (0xD800..0xDC00).contains(&cp)
                                && self.b[self.i..].starts_with(b"\\u")
                            {
                                self.i += 2;
                                let lo = self.hex4()?;
                                cp = 0x10000
                                    + ((cp - 0xD800) << 10)
                                    + (lo.wrapping_sub(0xDC00) & 0x3ff);
                            }
                            char::from_u32(cp).unwrap_or('\u{fffd}')
                        }
                        _ => return None,
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                _ => out.push(c),
            }
        }
    }

    fn value(&mut self, depth: u32) -> Option<Json> {
        if depth > 32 {
            return None;
        }
        self.ws();
        match *self.b.get(self.i)? {
            b'{' => {
                self.i += 1;
                let mut v = Vec::new();
                if self.eat(b'}').is_some() {
                    return Some(Json::Obj(v));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.eat(b':')?;
                    v.push((k, self.value(depth + 1)?));
                    if self.eat(b',').is_none() {
                        self.eat(b'}')?;
                        return Some(Json::Obj(v));
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut v = Vec::new();
                if self.eat(b']').is_some() {
                    return Some(Json::Arr(v));
                }
                loop {
                    v.push(self.value(depth + 1)?);
                    if self.eat(b',').is_none() {
                        self.eat(b']')?;
                        return Some(Json::Arr(v));
                    }
                }
            }
            b'"' => self.string().map(Json::Str),
            b't' => self.lit("true", Json::Bool(true)),
            b'f' => self.lit("false", Json::Bool(false)),
            b'n' => self.lit("null", Json::Null),
            _ => {
                let s = self.i;
                while self.i < self.b.len()
                    && matches!(
                        self.b[self.i],
                        b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'
                    )
                {
                    self.i += 1;
                }
                std::str::from_utf8(&self.b[s..self.i])
                    .ok()?
                    .parse()
                    .ok()
                    .map(Json::Num)
            }
        }
    }
}

fn parse_json(b: &[u8]) -> Option<Json> {
    let mut p = P { b, i: 0 };
    let v = p.value(0)?;
    p.ws();
    (p.i == b.len()).then_some(v)
}

impl Json {
    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(v) => v.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// `GET /containers/json?all=1` body -> containers.
pub fn parse_containers(body: &[u8]) -> Option<Vec<Container>> {
    let Json::Arr(items) = parse_json(body)? else {
        return None;
    };
    let mut out = Vec::new();
    for it in &items {
        let name = match it.get("Names") {
            Some(Json::Arr(n)) => n
                .first()
                .and_then(Json::str)
                .unwrap_or("")
                .trim_start_matches('/')
                .to_string(),
            _ => String::new(),
        };
        let field = |k: &str| it.get(k).and_then(Json::str).unwrap_or("").to_string();
        out.push(Container {
            name,
            image: field("Image"),
            state: field("State"),
            status: field("Status"),
        });
    }
    Some(out)
}

fn chunked(mut b: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let eol = b.windows(2).position(|w| w == b"\r\n")?;
        let size_s = std::str::from_utf8(&b[..eol])
            .ok()?
            .split(';')
            .next()?
            .trim();
        let size = usize::from_str_radix(size_s, 16).ok()?;
        b = &b[eol + 2..];
        if size == 0 {
            return Some(out);
        }
        out.extend_from_slice(b.get(..size)?);
        b = b.get(size + 2..)?;
    }
}

/// Split an HTTP/1.1 response, check for 200, undo chunked transfer-encoding.
pub fn http_body(raw: &[u8]) -> io::Result<Vec<u8>> {
    let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| bad("no http header"))?;
    let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
    let body = &raw[split + 4..];
    if !head.lines().next().unwrap_or("").contains(" 200") {
        return Err(bad("docker api: not 200"));
    }
    if head.contains("transfer-encoding: chunked") {
        chunked(body).ok_or_else(|| bad("bad chunked body"))
    } else {
        Ok(body.to_vec())
    }
}

pub fn query(socket: &str, timeout: Duration) -> io::Result<Vec<Container>> {
    let mut s = UnixStream::connect(socket)?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(timeout))?;
    s.write_all(
        b"GET /containers/json?all=1 HTTP/1.1\r\nHost: docker\r\nConnection: close\r\n\r\n",
    )?;
    let mut raw = Vec::new();
    s.take(8 << 20).read_to_end(&mut raw)?;
    let body = http_body(&raw)?;
    parse_containers(&body)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad docker json"))
}

pub fn docker(ctx: &mut Ctx, w: &mut SectionWriter) -> Res {
    match query(&ctx.cfg.docker.socket, ctx.cfg.exec_timeout()) {
        Ok(list) => {
            for c in list {
                w.line(format!("{} {} {} {}", c.name, c.image, c.state, c.status));
            }
            Ok(())
        }
        // Socket absent or not ours to read: the section is simply omitted.
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
            ) =>
        {
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// The `status+<2×interval>` message for `docker.must_run`, or None when nothing is required.
pub fn must_run_status(cfg: &Config, found: &io::Result<Vec<Container>>) -> Option<Vec<u8>> {
    let d = &cfg.docker;
    if d.must_run.is_empty() {
        return None;
    }
    let mut body = String::new();
    let mut bad = Vec::new();
    match found {
        Ok(list) => {
            for name in &d.must_run {
                let state = list
                    .iter()
                    .find(|c| &c.name == name)
                    .map(|c| c.state.as_str());
                match state {
                    Some("running") => body.push_str(&format!("&green {name} is running\n")),
                    Some(s) => {
                        body.push_str(&format!("&red {name} is {s}\n"));
                        bad.push(name.as_str());
                    }
                    None => {
                        body.push_str(&format!("&red {name} does not exist\n"));
                        bad.push(name.as_str());
                    }
                }
            }
        }
        Err(e) => {
            body.push_str(&format!("&red cannot query docker: {e}\n"));
            bad.extend(d.must_run.iter().map(String::as_str));
        }
    }
    let color = if bad.is_empty() {
        Color::Green
    } else {
        Color::Red
    };
    let (now, _) = common::now();
    let date = common::strftime("%a %b %e %H:%M:%S %Z %Y", now, false);
    let text = if bad.is_empty() {
        "Docker ok".to_string()
    } else {
        format!("Docker NOT ok: {}", bad.join(", "))
    };
    let minutes = (cfg.schedule.interval_s * 2).div_ceil(60).max(1);
    Some(proto::status(
        Some(&format!("{minutes}m")),
        &cfg.machine(),
        &d.status_column,
        color,
        &format!("{date} - {text}"),
        &body,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"[{"Id":"abc","Names":["/web"],"Image":"nginx:latest","ImageID":"sha256:x","Command":"nginx","Created":1700000000,"Ports":[{"IP":"0.0.0.0","PrivatePort":80,"Type":"tcp"}],"Labels":{},"State":"running","Status":"Up 2 hours (healthy)","Mounts":[]},{"Names":["/db \u00e9"],"Image":"postgres","State":"exited","Status":"Exited (0) 3 days ago"}]"#;

    #[test]
    fn parses_container_list() {
        let c = parse_containers(JSON.as_bytes()).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(
            c[0],
            Container {
                name: "web".into(),
                image: "nginx:latest".into(),
                state: "running".into(),
                status: "Up 2 hours (healthy)".into()
            }
        );
        assert_eq!(c[1].name, "db é");
        assert_eq!(c[1].state, "exited");
        assert_eq!(parse_containers(b"[]").unwrap(), vec![]);
        assert!(parse_containers(b"{\"message\":\"x\"}").is_none());
        assert!(parse_containers(b"[{").is_none());
    }

    #[test]
    fn http_plain_and_chunked() {
        let plain =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n[]";
        assert_eq!(http_body(plain).unwrap(), b"[]");
        let ch = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n[{}\r\n2\r\n]x\r\n0\r\n\r\n";
        assert_eq!(http_body(ch).unwrap(), b"[{}]x");
        assert!(http_body(b"HTTP/1.1 500 Err\r\n\r\n").is_err());
        assert!(http_body(b"junk").is_err());
    }

    #[test]
    fn must_run_colors() {
        let mut cfg = Config::default();
        cfg.identity.hostname = "pi4.lan".into();
        cfg.docker.must_run = vec!["web".into(), "db".into()];
        let list = Ok(vec![
            Container {
                name: "web".into(),
                image: "i".into(),
                state: "running".into(),
                status: "Up".into(),
            },
            Container {
                name: "db".into(),
                image: "i".into(),
                state: "exited".into(),
                status: "Exited".into(),
            },
        ]);
        let m = String::from_utf8(must_run_status(&cfg, &list).unwrap()).unwrap();
        assert!(m.starts_with("status+10m pi4,lan.docker red "), "{m}");
        assert!(m.contains("&green web is running\n&red db is exited\n"));
        let ok = Ok(vec![Container {
            name: "web".into(),
            image: "i".into(),
            state: "running".into(),
            status: "Up".into(),
        }]);
        cfg.docker.must_run = vec!["web".into()];
        assert!(
            String::from_utf8(must_run_status(&cfg, &ok).unwrap())
                .unwrap()
                .contains(" green ")
        );
        cfg.docker.must_run.clear();
        assert!(must_run_status(&cfg, &ok).is_none());
    }
}
