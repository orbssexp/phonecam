//! Portable PhoneCam core. No I/O, no threads, no clock: callers pass `now_ms`
//! and act on returned values, so Windows (tokio) and iOS (Swift/GCD) share it.
