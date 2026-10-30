//! Portable PhoneCam core. No I/O, no threads, no clock: callers pass `now_ms`
//! and act on returned values, so Windows (tokio) and iOS (Swift/GCD) share it.

pub mod backoff;
pub mod conn;
pub mod controls;
pub mod heartbeat;
pub mod pairing;
pub mod path;
pub mod protocol;

pub use conn::{Action, Conn, ConnState, Event, VcamFrame};
pub use controls::{Profile, VideoMode};
pub use protocol::{Ctl, PauseReason, Role, Signal, PROTOCOL_VERSION};
