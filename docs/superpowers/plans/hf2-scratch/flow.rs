// ---------- throughput and lag measurement ----------

/// End-to-end lag estimate: how much later AUs arrive, relative to the phone's capture clock
/// (`pts`), than the best case seen in the last 30 s. A growing value means frames are queuing
/// somewhere between the phone's encoder and our socket (the phone's send queue, the network).
struct LagEstimator {
    first_pts_ms: Option<f64>,
    last_pts_ms: f64,
    /// `(second index, smallest arrival-minus-pts in that second)`, last 30 s.
    mins: std::collections::VecDeque<(u64, f64)>,
    sum: f64,
    n: u64,
}

impl LagEstimator {
    fn new() -> Self {
        Self { first_pts_ms: None, last_pts_ms: 0.0, mins: Default::default(), sum: 0.0, n: 0 }
    }

    fn observe(&mut self, arrival_ms: f64, pts_ms: f64) {
        // The phone restarted its capture clock: start over.
        if self.first_pts_ms.is_some() && pts_ms + 1000.0 < self.last_pts_ms {
            *self = Self::new();
        }
        let first = *self.first_pts_ms.get_or_insert(pts_ms);
        self.last_pts_ms = pts_ms;
        let d = arrival_ms - (pts_ms - first);
        let sec = (arrival_ms / 1000.0) as u64;
        match self.mins.back_mut() {
            Some((s, m)) if *s == sec => *m = m.min(d),
            _ => self.mins.push_back((sec, d)),
        }
        while self.mins.front().is_some_and(|(s, _)| *s + 30 < sec) {
            self.mins.pop_front();
        }
        self.sum += d;
        self.n += 1;
    }

    /// Lag averaged over the AUs seen since the last call, in ms; `None` when there were none.
    fn take(&mut self) -> Option<u32> {
        if self.n == 0 {
            return None;
        }
        let avg = self.sum / self.n as f64;
        let base = self.mins.iter().map(|(_, m)| *m).fold(f64::INFINITY, f64::min);
        self.sum = 0.0;
        self.n = 0;
        Some((avg - base).max(0.0).round() as u32)
    }
}

/// Per-connection counters behind the `[native]` stats line and the UI's bitrate / delay.
struct Flow {
    lag: LagEstimator,
    win_start: Instant,
    bytes: u64,
    log_start: Instant,
    bytes10: u64,
    aus10: u64,
    keys10: u64,
    lag_max10: u32,
    lag_last: u32,
}

impl Flow {
    fn new() -> Self {
        let now = Instant::now();
        Self { lag: LagEstimator::new(), win_start: now, bytes: 0, log_start: now, bytes10: 0, aus10: 0, keys10: 0, lag_max10: 0, lag_last: 0 }
    }

    fn on_au(&mut self, len: usize, keyframe: bool, arrival: Duration, pts_us: u64) {
        self.bytes += len as u64;
        self.bytes10 += len as u64;
        self.aus10 += 1;
        self.keys10 += u64::from(keyframe);
        self.lag.observe(arrival.as_secs_f64() * 1000.0, pts_us as f64 / 1000.0);
    }
}

