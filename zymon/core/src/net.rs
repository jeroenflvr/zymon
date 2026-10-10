//! Blocking TCP sender: write the whole message, `shutdown(Write)`, optionally read the reply to EOF.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::thread::sleep;
use std::time::Duration;

pub const DEFAULT_PORT: u16 = 1984;
/// A logfetch.cfg-style reply is tiny; cap it so a broken peer cannot eat the Pi's RAM.
const MAX_REPLY: u64 = 1 << 20;

#[derive(Clone, Debug)]
pub struct NetOpts {
    pub connect_timeout: Duration,
    pub read_timeout: Duration,
    /// Attempts per server (connect + write), with `backoff` between them.
    pub retries: u32,
    pub backoff: Duration,
}

impl Default for NetOpts {
    fn default() -> Self {
        NetOpts {
            connect_timeout: Duration::from_secs(10),
            read_timeout: Duration::from_secs(15),
            retries: 3,
            backoff: Duration::from_secs(1),
        }
    }
}

#[derive(Debug)]
pub enum NetError {
    BadAddress(String),
    Resolve(String, io::Error),
    Connect(String, io::Error),
    Write(String, io::Error),
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetError::BadAddress(a) => write!(f, "bad server address '{a}'"),
            NetError::Resolve(a, e) => write!(f, "cannot resolve {a}: {e}"),
            NetError::Connect(a, e) => write!(f, "cannot connect to {a}: {e}"),
            NetError::Write(a, e) => write!(f, "cannot send to {a}: {e}"),
        }
    }
}

impl std::error::Error for NetError {}

/// `host`, `host:port`, `[v6]:port` or a bare IPv6 literal.
pub fn split_host_port(s: &str) -> Option<(&str, u16)> {
    if s.is_empty() {
        return None;
    }
    if let Some(rest) = s.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        return match tail {
            "" => Some((host, DEFAULT_PORT)),
            _ => Some((host, tail.strip_prefix(':')?.parse().ok()?)),
        };
    }
    match s.matches(':').count() {
        0 => Some((s, DEFAULT_PORT)),
        1 => {
            let (h, p) = s.split_once(':')?;
            if h.is_empty() {
                None
            } else {
                Some((h, p.parse().ok()?))
            }
        }
        _ => Some((s, DEFAULT_PORT)),
    }
}

fn connect(server: &str, opts: &NetOpts) -> Result<TcpStream, NetError> {
    let (host, port) =
        split_host_port(server).ok_or_else(|| NetError::BadAddress(server.into()))?;
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|e| NetError::Resolve(server.into(), e))?;
    let mut last = io::Error::new(io::ErrorKind::NotFound, "no addresses");
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, opts.connect_timeout) {
            Ok(s) => return Ok(s),
            Err(e) => last = e,
        }
    }
    Err(NetError::Connect(server.into(), last))
}

fn attempt(
    server: &str,
    msg: &[u8],
    want_reply: bool,
    opts: &NetOpts,
) -> Result<Vec<u8>, NetError> {
    let mut s = connect(server, opts)?;
    s.set_write_timeout(Some(opts.read_timeout)).ok();
    s.set_read_timeout(Some(opts.read_timeout)).ok();
    s.set_nodelay(true).ok();
    s.write_all(msg)
        .map_err(|e| NetError::Write(server.into(), e))?;
    s.shutdown(Shutdown::Write)
        .map_err(|e| NetError::Write(server.into(), e))?;
    let mut reply = Vec::new();
    if want_reply {
        // The message is already delivered at this point, so a read timeout or reset only
        // costs us the (optional) reply; keep whatever arrived instead of retrying and re-sending.
        let _ = s.take(MAX_REPLY).read_to_end(&mut reply);
    }
    Ok(reply)
}

/// Send `msg` to one server (`host[:port]`), retrying connect/write failures. Returns the reply bytes
/// (empty when `want_reply` is false or the server said nothing).
pub fn send_message(
    server: &str,
    msg: &[u8],
    want_reply: bool,
    opts: &NetOpts,
) -> Result<Vec<u8>, NetError> {
    let tries = opts.retries.max(1);
    let mut err = None;
    for i in 0..tries {
        if i > 0 {
            sleep(opts.backoff);
        }
        match attempt(server, msg, want_reply, opts) {
            Ok(r) => return Ok(r),
            Err(e @ NetError::BadAddress(_)) => return Err(e),
            Err(e) => err = Some(e),
        }
    }
    Err(err.expect("at least one attempt"))
}

/// Send to every server (legacy `XYMSERVERS` semantics). The reply is the first non-empty one.
/// Per-server failures are returned for logging; the caller decides what a partial failure means.
pub fn send_all(
    servers: &[String],
    msg: &[u8],
    want_reply: bool,
    opts: &NetOpts,
) -> (Vec<u8>, Vec<NetError>) {
    let mut reply = Vec::new();
    let mut errors = Vec::new();
    for s in servers {
        match send_message(s, msg, want_reply, opts) {
            Ok(r) => {
                if reply.is_empty() {
                    reply = r;
                }
            }
            Err(e) => errors.push(e),
        }
    }
    (reply, errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    fn fast() -> NetOpts {
        NetOpts {
            connect_timeout: Duration::from_secs(2),
            read_timeout: Duration::from_secs(2),
            retries: 2,
            backoff: Duration::from_millis(10),
        }
    }

    /// Loopback xymond: read to EOF, then answer with `reply`.
    fn fake_server(reply: &'static [u8]) -> (String, thread::JoinHandle<Vec<u8>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let h = thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            let mut got = Vec::new();
            c.read_to_end(&mut got).unwrap();
            c.write_all(reply).unwrap();
            got
        });
        (addr, h)
    }

    #[test]
    fn write_shutdown_read_reply() {
        let (addr, h) = fake_server(b"clientversion:4.3.30\n");
        let reply =
            send_message(&addr, b"client a.linux linux\n[date]\nx\n", true, &fast()).unwrap();
        assert_eq!(reply, b"clientversion:4.3.30\n");
        assert_eq!(h.join().unwrap(), b"client a.linux linux\n[date]\nx\n");
    }

    #[test]
    fn no_reply_wanted() {
        let (addr, h) = fake_server(b"ignored");
        let reply = send_message(&addr, b"combo\n", false, &fast()).unwrap();
        assert!(reply.is_empty());
        assert_eq!(h.join().unwrap(), b"combo\n");
    }

    #[test]
    fn all_servers_first_nonempty_reply() {
        let (a, ha) = fake_server(b"");
        let (b, hb) = fake_server(b"log:/x:1\n");
        let (reply, errs) = send_all(&[a, b], b"m", true, &fast());
        assert_eq!(reply, b"log:/x:1\n");
        assert!(errs.is_empty());
        ha.join().unwrap();
        hb.join().unwrap();
    }

    #[test]
    fn connect_refused_is_error() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        drop(l);
        assert!(matches!(
            send_message(&addr, b"x", false, &fast()),
            Err(NetError::Connect(..))
        ));
    }

    #[test]
    fn address_parsing() {
        assert_eq!(split_host_port("a"), Some(("a", 1984)));
        assert_eq!(split_host_port("a:99"), Some(("a", 99)));
        assert_eq!(split_host_port("[::1]:7"), Some(("::1", 7)));
        assert_eq!(split_host_port("[::1]"), Some(("::1", 1984)));
        assert_eq!(split_host_port("::1"), Some(("::1", 1984)));
        assert_eq!(split_host_port("a:x"), None);
        assert_eq!(split_host_port(""), None);
        assert!(matches!(
            send_message("a:x", b"", false, &fast()),
            Err(NetError::BadAddress(_))
        ));
    }
}
