//! Thin OS shims. Nothing above this layer touches libc.

pub mod common;
pub mod exec;
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
