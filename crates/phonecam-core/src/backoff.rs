/// Exponential backoff: base, 2×base, 4×base … capped. Spec §7.2.
#[derive(Debug, Clone)]
pub struct Backoff {
    attempt: u32,
    base_ms: u64,
    cap_ms: u64,
}

impl Backoff {
    pub fn new(base_ms: u64, cap_ms: u64) -> Self {
        Self { attempt: 0, base_ms, cap_ms }
    }

    pub fn next_delay_ms(&mut self) -> u64 {
        let shift = self.attempt.min(32);
        let d = self.base_ms.saturating_mul(1u64 << shift).min(self.cap_ms);
        self.attempt = self.attempt.saturating_add(1);
        d
    }

    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_from_1s_and_caps_at_30s() {
        let mut b = Backoff::new(1_000, 30_000);
        let got: Vec<u64> = (0..7).map(|_| b.next_delay_ms()).collect();
        assert_eq!(got, vec![1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000]);
    }

    #[test]
    fn reset_starts_over() {
        let mut b = Backoff::new(1_000, 30_000);
        b.next_delay_ms();
        b.next_delay_ms();
        b.reset();
        assert_eq!(b.next_delay_ms(), 1_000);
    }

    #[test]
    fn never_overflows() {
        let mut b = Backoff::new(1_000, 30_000);
        for _ in 0..200 { assert!(b.next_delay_ms() <= 30_000); }
    }
}
