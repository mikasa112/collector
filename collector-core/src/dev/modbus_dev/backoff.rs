use std::time::Duration;

pub(super) struct Backoff {
    current: Duration,
    base: Duration,
    max: Duration,
}

impl Backoff {
    pub(super) fn new(base: Duration, max: Duration) -> Self {
        Self {
            current: base,
            base,
            max,
        }
    }

    pub(super) fn reset(&mut self) {
        self.current = self.base;
    }

    pub(super) fn next_delay(&mut self) -> Duration {
        let delay = self.current;
        self.current = (self.current * 2).min(self.max);
        delay
    }
}
