/// What the caller should do after a tick. Spec §7.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HbAction {
    /// Send `Ctl::Ping { t }` on the DataChannel. (`pong` doesn't match `t` against
    /// outstanding pings: any pong proves the link is alive, which is all we need.)
    SendPing { t: u64 },
    /// `miss_limit` pings went unanswered. Repeats every interval until `pong`/`reset`.
    Lost,
}

#[derive(Debug, Clone)]
pub struct Heartbeat {
    interval_ms: u64,
    miss_limit: u32,
    next_ping_at: u64,
    outstanding: u32,
    rtt_ms: Option<u64>,
}

impl Heartbeat {
    pub fn new(interval_ms: u64, miss_limit: u32, now_ms: u64) -> Self {
        Self { interval_ms, miss_limit, next_ping_at: now_ms + interval_ms, outstanding: 0, rtt_ms: None }
    }

    pub fn tick(&mut self, now_ms: u64) -> Option<HbAction> {
        if now_ms < self.next_ping_at {
            return None;
        }
        self.next_ping_at = now_ms + self.interval_ms;
        // With interval 2 s and limit 3, this fires 6 s after the last pong (spec §7.2).
        if self.outstanding + 1 >= self.miss_limit {
            return Some(HbAction::Lost);
        }
        self.outstanding += 1;
        Some(HbAction::SendPing { t: now_ms })
    }

    pub fn pong(&mut self, t: u64, now_ms: u64) {
        self.outstanding = 0;
        self.rtt_ms = Some(now_ms.saturating_sub(t));
    }

    pub fn rtt_ms(&self) -> Option<u64> {
        self.rtt_ms
    }

    pub fn reset(&mut self, now_ms: u64) {
        self.outstanding = 0;
        self.next_ping_at = now_ms + self.interval_ms;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pings_every_interval() {
        let mut h = Heartbeat::new(2_000, 3, 0);
        assert_eq!(h.tick(1_999), None);
        assert_eq!(h.tick(2_000), Some(HbAction::SendPing { t: 2_000 }));
        assert_eq!(h.tick(2_500), None);
        assert_eq!(h.tick(4_000), Some(HbAction::SendPing { t: 4_000 }));
    }

    #[test]
    fn lost_after_6s_without_pong() {
        // Spec §7.2: "3 missed pongs (6 s)". Pings at 2 s and 4 s go unanswered; at 6 s, Lost.
        let mut h = Heartbeat::new(2_000, 3, 0);
        assert_eq!(h.tick(2_000), Some(HbAction::SendPing { t: 2_000 }));
        assert_eq!(h.tick(4_000), Some(HbAction::SendPing { t: 4_000 }));
        assert_eq!(h.tick(6_000), Some(HbAction::Lost));
    }

    #[test]
    fn pong_clears_misses_and_measures_rtt() {
        let mut h = Heartbeat::new(2_000, 3, 0);
        h.tick(2_000);
        h.tick(4_000);
        h.pong(4_000, 4_007);
        assert_eq!(h.rtt_ms(), Some(7));
        assert_eq!(h.tick(6_000), Some(HbAction::SendPing { t: 6_000 }));
        assert_eq!(h.tick(8_000), Some(HbAction::SendPing { t: 8_000 }));
        assert_eq!(h.tick(10_000), Some(HbAction::Lost));
    }

    #[test]
    fn reset_restarts_the_schedule() {
        let mut h = Heartbeat::new(2_000, 3, 0);
        h.tick(2_000);
        h.tick(4_000);
        h.reset(10_000);
        assert_eq!(h.tick(11_000), None);
        assert_eq!(h.tick(12_000), Some(HbAction::SendPing { t: 12_000 }));
    }
}
