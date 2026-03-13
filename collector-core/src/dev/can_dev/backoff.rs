use std::time::Duration;

pub(super) struct Backoff {
    current: Duration,
    initial: Duration,
    max: Duration,
}

impl Backoff {
    pub(super) fn new(initial: Duration, max: Duration) -> Self {
        Self {
            current: initial,
            initial,
            max,
        }
    }

    pub(super) fn reset(&mut self) {
        self.current = self.initial;
    }

    pub(super) fn next_delay(&mut self) -> Duration {
        let delay = self.current;
        self.current = (self.current * 2).min(self.max);
        delay
    }
}
