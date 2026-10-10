//! Tiny stderr/file logger. Levels are filtered with one atomic; no allocation unless a line is emitted.

use std::fs::File;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Level {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
}

static LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);
static FILE: Mutex<Option<File>> = Mutex::new(None);

pub fn set_level(level: Level) {
    LEVEL.store(level as u8, Ordering::Relaxed);
}

/// Also append every line to `file` (in addition to stderr).
pub fn set_file(file: File) {
    if let Ok(mut f) = FILE.lock() {
        *f = Some(file);
    }
}

pub fn enabled(level: Level) -> bool {
    level as u8 <= LEVEL.load(Ordering::Relaxed)
}

pub fn log(level: Level, args: std::fmt::Arguments<'_>) {
    if !enabled(level) {
        return;
    }
    let tag = match level {
        Level::Error => "ERROR",
        Level::Warn => "WARN",
        Level::Info => "INFO",
        Level::Debug => "DEBUG",
    };
    let line = format!("{tag} {args}\n");
    let _ = std::io::stderr().write_all(line.as_bytes());
    if let Ok(mut f) = FILE.lock()
        && let Some(f) = f.as_mut()
    {
        let _ = f.write_all(line.as_bytes());
    }
}

#[macro_export]
macro_rules! error { ($($t:tt)*) => { $crate::log::log($crate::log::Level::Error, format_args!($($t)*)) } }
#[macro_export]
macro_rules! warn { ($($t:tt)*) => { $crate::log::log($crate::log::Level::Warn, format_args!($($t)*)) } }
#[macro_export]
macro_rules! info { ($($t:tt)*) => { $crate::log::log($crate::log::Level::Info, format_args!($($t)*)) } }
#[macro_export]
macro_rules! debug { ($($t:tt)*) => { $crate::log::log($crate::log::Level::Debug, format_args!($($t)*)) } }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_filtering() {
        set_level(Level::Warn);
        assert!(enabled(Level::Error));
        assert!(enabled(Level::Warn));
        assert!(!enabled(Level::Debug));
        set_level(Level::Info);
    }
}
