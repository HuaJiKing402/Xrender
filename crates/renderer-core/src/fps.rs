use std::{collections::VecDeque, time::Instant};

const WINDOW: usize = 30;

/// 滚动窗口帧率统计，避免单帧抖动导致 FPS 数字跳变。
pub(crate) struct FpsCounter {
    last: Option<Instant>,
    samples: VecDeque<f32>,
    sum: f32,
    value: Option<f32>,
}

impl FpsCounter {
    pub fn new() -> Self {
        Self {
            last: None,
            samples: VecDeque::with_capacity(WINDOW),
            sum: 0.0,
            value: None,
        }
    }

    pub fn tick(&mut self, now: Instant) -> Option<f32> {
        let dt = self
            .last
            .map(|last| now.saturating_duration_since(last).as_secs_f32())
            .unwrap_or(0.0);
        self.last = Some(now);
        self.push_duration(dt)
    }

    pub fn push_duration(&mut self, dt: f32) -> Option<f32> {
        if !dt.is_finite() || dt <= 0.0 {
            return self.value;
        }
        let dt = dt.clamp(0.001, 0.5);
        if self.samples.len() == WINDOW {
            if let Some(old) = self.samples.pop_front() {
                self.sum -= old;
            }
        }
        self.samples.push_back(dt);
        self.sum += dt;
        if self.samples.len() >= 2 {
            self.value = Some(self.samples.len() as f32 / self.sum);
        }
        self.value
    }

    pub fn value(&self) -> Option<f32> {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::FpsCounter;

    #[test]
    fn averages_sixty_hz() {
        let mut counter = FpsCounter::new();
        for _ in 0..30 {
            counter.push_duration(1.0 / 60.0);
        }
        assert!((counter.value().unwrap() - 60.0).abs() < 0.01);
    }

    #[test]
    fn averages_thirty_hz() {
        let mut counter = FpsCounter::new();
        for _ in 0..30 {
            counter.push_duration(1.0 / 30.0);
        }
        assert!((counter.value().unwrap() - 30.0).abs() < 0.01);
    }

    #[test]
    fn first_frame_does_not_produce_huge_value() {
        let mut counter = FpsCounter::new();
        counter.push_duration(0.000001);
        assert!(counter.value().is_none());
    }
}
