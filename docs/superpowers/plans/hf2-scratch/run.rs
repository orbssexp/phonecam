    fn run(&mut self) {
        tracing::info!("[native] started ({})", if self.cfg.addr.is_some() { "explicit address" } else { "mDNS browse" });
        let mut failures = 0u32;
        while !self.done {
            let addrs = match self.cfg.addr {
                Some(a) => vec![a],
                None => self.discover(),
            };
            let mut visible = false;
            if !self.done && !addrs.is_empty() {
                visible = true;
                match self.connect(&addrs) {
                    Some(s) => {
                        failures = 0;
                        self.session(s);
                        // The phone just dropped us (or the app is quitting): look again right away.
                        continue;
                    }
                    None => failures += 1,
                }
            }
            if !self.done {
                // Service visible: retry quickly. Not visible: the browse above already waited.
                let d = if visible { retry_delay_ms(failures) } else { 250 };
                self.wait(Duration::from_millis(d));
            }
        }
        if let Some(d) = self.daemon.take() {
            let _ = d.shutdown();
        }
    }

