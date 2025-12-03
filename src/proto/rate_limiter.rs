use tokio::time::Instant;
pub struct RateLimiter {
    capacity: usize,
    tokens: f64,
    refill_per_sec: f64,
    last: Instant,
}
impl RateLimiter {
    pub fn new(bytes_per_sec: usize, burst: usize) -> Self {
        Self {
            capacity: burst,
            tokens: burst as f64,
            refill_per_sec: bytes_per_sec as f64,
            last: Instant::now(),
        }
    }

    pub fn allow(&mut self, n: usize) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f64();
        let refill = elapsed * self.refill_per_sec;
        self.tokens = (self.tokens + refill).min(self.capacity as f64);
        self.last = now;
        if (self.tokens as f64) >= (n as f64) {
            self.tokens -= n as f64;
            true
        } else {
            false
        }
    }
}