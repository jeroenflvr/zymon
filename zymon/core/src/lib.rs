//! Shared by the zymon client and (later) the server: wire protocol, TCP sender,
//! host naming helpers, logger. No OS probing, no config handling.

pub mod clientcfg;
pub mod host;
pub mod log;
pub mod net;
pub mod proto;
