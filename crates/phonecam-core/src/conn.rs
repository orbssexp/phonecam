use crate::backoff::Backoff;
use crate::protocol::PauseReason;

/// First retry waits for the ICE restart to complete; later ones back off from 1 s.
const FIRST_RESTART_TIMEOUT_MS: u64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Idle,
    Connecting,
    Live,
    Paused(PauseReason),
    Reconnecting { attempt: u32 },
}

/// What the "PhoneCam" virtual camera should show (spec §7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcamFrame {
    Placeholder,
    Camera,
    Reconnecting,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    PhoneJoined,
    MediaConnected,
    /// No media within 10 s of `PhoneJoined` (timer owned by transport).
    ConnectTimedOut,
    HeartbeatLost,
    RetryTimerFired,
    CameraPaused(PauseReason),
    CameraResumed,
    PhoneLeft,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    SetVcam(VcamFrame),
    /// Send `Signal::RestartIce` to the phone (DataChannel first, relay fallback).
    SendRestartIce,
    /// Call `handle(Event::RetryTimerFired)` after `ms`, unless state changed.
    ScheduleRetry { ms: u64 },
}

#[derive(Debug, Clone)]
pub struct Conn {
    state: ConnState,
    backoff: Backoff,
}

impl Default for Conn {
    fn default() -> Self {
        Self::new()
    }
}

impl Conn {
    pub fn new() -> Self {
        Self { state: ConnState::Idle, backoff: Backoff::new(1_000, 30_000) }
    }

    pub fn initial_vcam() -> VcamFrame {
        VcamFrame::Placeholder
    }

    pub fn state(&self) -> ConnState {
        self.state
    }

    pub fn handle(&mut self, ev: Event) -> Vec<Action> {
        use ConnState::*;
        match (self.state, ev) {
            (Idle | Reconnecting { .. }, Event::PhoneJoined) => {
                self.state = Connecting;
                vec![]
            }
            (Connecting | Reconnecting { .. }, Event::MediaConnected) => {
                self.state = Live;
                self.backoff.reset();
                vec![Action::SetVcam(VcamFrame::Camera)]
            }
            (Live | Paused(_), Event::HeartbeatLost) | (Connecting, Event::ConnectTimedOut) => {
                self.state = Reconnecting { attempt: 1 };
                vec![
                    Action::SetVcam(VcamFrame::Reconnecting),
                    Action::SendRestartIce,
                    Action::ScheduleRetry { ms: FIRST_RESTART_TIMEOUT_MS },
                ]
            }
            (Reconnecting { attempt }, Event::RetryTimerFired) => {
                self.state = Reconnecting { attempt: attempt + 1 };
                vec![Action::SendRestartIce, Action::ScheduleRetry { ms: self.backoff.next_delay_ms() }]
            }
            (Live, Event::CameraPaused(r)) => {
                self.state = Paused(r);
                vec![Action::SetVcam(VcamFrame::Paused)]
            }
            (Paused(_), Event::CameraResumed) => {
                self.state = Live;
                vec![Action::SetVcam(VcamFrame::Camera)]
            }
            (Connecting | Live | Paused(_), Event::PhoneLeft) => {
                self.state = Reconnecting { attempt: 1 };
                vec![Action::SetVcam(VcamFrame::Reconnecting)]
            }
            _ => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PauseReason;

    fn live() -> Conn {
        let mut c = Conn::new();
        c.handle(Event::PhoneJoined);
        c.handle(Event::MediaConnected);
        c
    }

    #[test]
    fn starts_idle_with_placeholder() {
        let c = Conn::new();
        assert_eq!(c.state(), ConnState::Idle);
        assert_eq!(Conn::initial_vcam(), VcamFrame::Placeholder);
    }

    #[test]
    fn join_then_media_goes_live() {
        let mut c = Conn::new();
        assert!(c.handle(Event::PhoneJoined).is_empty());
        assert_eq!(c.state(), ConnState::Connecting);
        assert_eq!(c.handle(Event::MediaConnected), vec![Action::SetVcam(VcamFrame::Camera)]);
        assert_eq!(c.state(), ConnState::Live);
    }

    #[test]
    fn connect_timeout_starts_restarting() {
        let mut c = Conn::new();
        c.handle(Event::PhoneJoined);
        assert_eq!(c.handle(Event::ConnectTimedOut), vec![
            Action::SetVcam(VcamFrame::Reconnecting),
            Action::SendRestartIce,
            Action::ScheduleRetry { ms: 10_000 },
        ]);
        assert_eq!(c.state(), ConnState::Reconnecting { attempt: 1 });
    }

    #[test]
    fn heartbeat_loss_asks_for_ice_restart() {
        let mut c = live();
        let a = c.handle(Event::HeartbeatLost);
        assert_eq!(a, vec![
            Action::SetVcam(VcamFrame::Reconnecting),
            Action::SendRestartIce,
            Action::ScheduleRetry { ms: 10_000 },
        ]);
        assert_eq!(c.state(), ConnState::Reconnecting { attempt: 1 });
    }

    #[test]
    fn repeated_loss_while_reconnecting_is_ignored() {
        let mut c = live();
        c.handle(Event::HeartbeatLost);
        assert!(c.handle(Event::HeartbeatLost).is_empty());
    }

    #[test]
    fn retries_back_off_from_1s() {
        let mut c = live();
        c.handle(Event::HeartbeatLost);
        assert_eq!(c.handle(Event::RetryTimerFired), vec![Action::SendRestartIce, Action::ScheduleRetry { ms: 1_000 }]);
        assert_eq!(c.handle(Event::RetryTimerFired), vec![Action::SendRestartIce, Action::ScheduleRetry { ms: 2_000 }]);
        assert_eq!(c.state(), ConnState::Reconnecting { attempt: 3 });
    }

    #[test]
    fn recovery_resets_backoff() {
        let mut c = live();
        c.handle(Event::HeartbeatLost);
        c.handle(Event::RetryTimerFired);
        c.handle(Event::RetryTimerFired);
        c.handle(Event::MediaConnected);
        assert_eq!(c.state(), ConnState::Live);
        c.handle(Event::HeartbeatLost);
        assert_eq!(c.handle(Event::RetryTimerFired), vec![Action::SendRestartIce, Action::ScheduleRetry { ms: 1_000 }]);
    }

    #[test]
    fn call_pauses_and_resumes() {
        let mut c = live();
        assert_eq!(c.handle(Event::CameraPaused(PauseReason::Call)), vec![Action::SetVcam(VcamFrame::Paused)]);
        assert_eq!(c.state(), ConnState::Paused(PauseReason::Call));
        assert_eq!(c.handle(Event::CameraResumed), vec![Action::SetVcam(VcamFrame::Camera)]);
        assert_eq!(c.state(), ConnState::Live);
    }

    #[test]
    fn loss_while_paused_reconnects() {
        let mut c = live();
        c.handle(Event::CameraPaused(PauseReason::Locked));
        assert_eq!(c.handle(Event::HeartbeatLost)[0], Action::SetVcam(VcamFrame::Reconnecting));
    }

    #[test]
    fn phone_leaving_waits_without_restart() {
        let mut c = live();
        assert_eq!(c.handle(Event::PhoneLeft), vec![Action::SetVcam(VcamFrame::Reconnecting)]);
        assert_eq!(c.state(), ConnState::Reconnecting { attempt: 1 });
        c.handle(Event::PhoneJoined);
        assert_eq!(c.state(), ConnState::Connecting);
    }

    #[test]
    fn phone_leaving_while_already_reconnecting_is_ignored() {
        let mut c = live();
        c.handle(Event::HeartbeatLost);
        assert!(c.handle(Event::PhoneLeft).is_empty());
        assert_eq!(c.state(), ConnState::Reconnecting { attempt: 1 });
    }

    #[test]
    fn irrelevant_events_do_nothing() {
        let mut c = Conn::new();
        assert!(c.handle(Event::HeartbeatLost).is_empty());
        assert!(c.handle(Event::CameraResumed).is_empty());
        assert!(c.handle(Event::RetryTimerFired).is_empty());
        assert!(c.handle(Event::PhoneLeft).is_empty());
        assert_eq!(c.state(), ConnState::Idle);
    }
}
