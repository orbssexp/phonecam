    /// Tries every address in order (IPv4 first); logs exactly why each one failed.
    fn connect(&mut self, addrs: &[SocketAddr]) -> Option<TcpStream> {
        for a in addrs {
            if self.done {
                return None;
            }
            let t0 = Instant::now();
            match open_stream(a) {
                Ok(mut s) => {
                    let _ = s.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
                    let _ = s.set_write_timeout(Some(HANDSHAKE_TIMEOUT));
                    match client_handshake(&mut s, &self.cfg.secret) {
                        Ok(()) => {
                            tracing::info!("[native] authenticated with {a} (connect+handshake {} ms)", t0.elapsed().as_millis());
                            return Some(s);
                        }
                        Err(e) => tracing::warn!("[native] handshake with {a} failed: {e}"),
                    }
                }
                Err(e) => tracing::warn!("[native] connect to {a} failed: {}", describe_connect_error(&e)),
            }
        }
        None
    }

