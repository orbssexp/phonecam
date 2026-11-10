    /// Once a second: refresh the received bitrate and lag the UI reads; every 10 s: one log line.
    fn flow_tick(&mut self, s: &mut Session) {
        let f = &mut s.flow;
        let el = f.win_start.elapsed();
        if el >= STATS_EVERY {
            let kbps = (f.bytes as f64 * 8.0 / 1000.0 / el.as_secs_f64()).round() as u32;
            self.stats.rx_kbps.store(kbps, Ordering::Relaxed);
            if let Some(lag) = f.lag.take() {
                f.lag_last = lag;
                f.lag_max10 = f.lag_max10.max(lag);
            }
            self.stats.lag_ms.store(f.lag_last, Ordering::Relaxed);
            f.bytes = 0;
            f.win_start = Instant::now();
        }
        let el = f.log_start.elapsed();
        if el >= LOG_EVERY {
            let secs = el.as_secs_f64();
            tracing::info!(
                "[native] stats last {:.0}s: received {:.0} kbps, {} AUs ({:.1}/s), average AU {} bytes, {} keyframes, lag {} ms (max {} ms)",
                secs,
                f.bytes10 as f64 * 8.0 / 1000.0 / secs,
                f.aus10,
                f.aus10 as f64 / secs,
                f.bytes10.checked_div(f.aus10).unwrap_or(0),
                f.keys10,
                f.lag_last,
                f.lag_max10,
            );
            f.bytes10 = 0;
            f.aus10 = 0;
            f.keys10 = 0;
            f.lag_max10 = f.lag_last;
            f.log_start = Instant::now();
        }
    }

