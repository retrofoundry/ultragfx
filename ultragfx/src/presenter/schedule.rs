use std::time::{Duration, Instant};

pub(super) const SPIN_WINDOW: Duration = Duration::from_micros(250);

pub(super) fn wait_until(at: Instant) {
    while let Some(remaining) = at.checked_duration_since(Instant::now()) {
        if remaining > SPIN_WINDOW {
            // Relative waits can be coalesced past their timeout on macOS.
            std::thread::sleep(remaining / 2);
        } else {
            std::hint::spin_loop();
        }
    }
}

pub(super) struct Deadlines {
    next: Instant,
    period: Duration,
}

impl Deadlines {
    pub(super) fn new(now: Instant, period: Duration) -> Self {
        Self {
            next: now + period,
            period,
        }
    }

    pub(super) fn next(&self) -> Instant {
        self.next
    }

    pub(super) fn acquire_at(&self, work: Duration) -> Instant {
        self.next - (work + Duration::from_millis(3)).min(self.period / 2)
    }

    pub(super) fn presented(&mut self) {
        self.next += self.period;
    }

    pub(super) fn skip_expired(&mut self, now: Instant) -> u64 {
        let skipped = now.saturating_duration_since(self.next).as_nanos() / self.period.as_nanos();
        if skipped != 0 {
            let remainder =
                now.saturating_duration_since(self.next).as_nanos() % self.period.as_nanos();
            self.next = now
                - Duration::new(
                    (remainder / 1_000_000_000) as u64,
                    (remainder % 1_000_000_000) as u32,
                );
        }
        skipped.min(u128::from(u64::MAX)) as u64
    }
}

fn phase(a: Instant, b: Instant, sample: Instant) -> f32 {
    let Some(interval) = b.checked_duration_since(a).filter(|value| !value.is_zero()) else {
        return 1.0;
    };
    (sample.saturating_duration_since(a).as_secs_f64() / interval.as_secs_f64()).clamp(0.0, 1.0)
        as f32
}

#[derive(Debug, PartialEq)]
pub(super) enum Phase {
    Previous(f32),
    Latest(f32),
}

pub(super) fn select_phase(
    older: Option<Instant>,
    a: Instant,
    b: Instant,
    display: Instant,
    lead: Duration,
) -> Phase {
    let Some(interval) = b.checked_duration_since(a).filter(|value| !value.is_zero()) else {
        return Phase::Latest(1.0);
    };
    let older = older.filter(|older| *older < a);
    // A short source interval must not consume the following interval's jitter budget.
    let delay = older.map_or(interval, |older| interval.max(a.duration_since(older))) + lead;
    let sample = display.checked_sub(delay).unwrap_or(a);
    if let Some(older) = older.filter(|_| sample < a) {
        Phase::Previous(phase(older, a, sample))
    } else {
        Phase::Latest(phase(a, b, sample))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparation_lead_keeps_the_pending_source_boundary_inside_the_pair() {
        let start = Instant::now();
        assert_eq!(
            select_phase(
                None,
                start,
                start + Duration::from_millis(50),
                start + Duration::from_millis(102),
                Duration::from_millis(6)
            ),
            Phase::Latest(0.92)
        );
    }

    #[test]
    fn a_short_source_interval_does_not_end_the_pair_early() {
        let start = Instant::now();
        assert_eq!(
            select_phase(
                Some(start),
                start + Duration::from_millis(56),
                start + Duration::from_millis(100),
                start + Duration::from_millis(150),
                Duration::from_millis(6)
            ),
            Phase::Latest(32.0 / 44.0)
        );
    }

    #[test]
    fn a_new_source_preserves_the_previous_pair_until_the_delayed_time_reaches_it() {
        let start = Instant::now();
        assert_eq!(
            select_phase(
                Some(start),
                start + Duration::from_millis(50),
                start + Duration::from_millis(100),
                start + Duration::from_millis(102),
                Duration::from_millis(6)
            ),
            Phase::Previous(0.92)
        );
    }

    #[test]
    fn delayed_selection_never_extrapolates_or_uses_a_missing_pair() {
        let start = Instant::now();
        let b = start + Duration::from_millis(50);
        assert_eq!(
            select_phase(None, start, b, b, Duration::from_millis(6)),
            Phase::Latest(0.0)
        );
        assert_eq!(
            select_phase(
                None,
                start,
                b,
                start + Duration::from_secs(1),
                Duration::from_millis(6)
            ),
            Phase::Latest(1.0)
        );
    }

    #[test]
    fn deadlines_skip_late_frames_without_a_catch_up_burst() {
        let origin = Instant::now();
        let mut deadlines = Deadlines::new(origin, Duration::from_millis(10));
        assert_eq!(
            deadlines.skip_expired(origin + Duration::from_millis(19)),
            0
        );
        assert_eq!(
            deadlines.skip_expired(origin + Duration::from_millis(20)),
            1
        );
        assert_eq!(deadlines.next(), origin + Duration::from_millis(20));
        assert_eq!(
            deadlines.skip_expired(origin + Duration::from_millis(55)),
            3
        );
        assert_eq!(deadlines.next(), origin + Duration::from_millis(50));
        deadlines.presented();
        assert_eq!(deadlines.next(), origin + Duration::from_millis(60));
        assert!(deadlines.next() > origin + Duration::from_millis(57));
        deadlines = Deadlines::new(
            origin + Duration::from_millis(60),
            Duration::from_millis(20),
        );
        assert_eq!(deadlines.next(), origin + Duration::from_millis(80));
    }

    #[test]
    fn present_overhead_does_not_accumulate_over_many_frames() {
        let origin = Instant::now();
        let period = Duration::from_nanos(16_666_667);
        let overhead = Duration::from_micros(300);
        let mut deadlines = Deadlines::new(origin, period);
        for n in 1..=36_000 {
            let presented = deadlines.next() + overhead;
            assert_eq!(deadlines.skip_expired(presented), 0);
            assert_eq!(presented, origin + period * n + overhead);
            deadlines.presented();
        }
        assert_eq!(deadlines.next(), origin + period * 36_001);
    }

    #[test]
    fn phase_is_delayed_by_one_source_interval_and_clamped() {
        let a = Instant::now();
        let b = a + Duration::from_millis(50);
        for (offset, expected) in [(25, 0.0), (50, 0.0), (75, 0.5), (100, 1.0), (150, 1.0)] {
            assert_eq!(
                select_phase(
                    None,
                    a,
                    b,
                    a + Duration::from_millis(offset),
                    Duration::ZERO
                ),
                Phase::Latest(expected)
            );
        }
        assert_eq!(
            select_phase(None, a, a, a, Duration::ZERO),
            Phase::Latest(1.0)
        );
        assert_eq!(
            select_phase(None, b, a, b, Duration::ZERO),
            Phase::Latest(1.0)
        );
    }

    #[test]
    fn phase_preparation_does_not_extend_the_requested_present_period() {
        let origin = Instant::now();
        let period = Duration::from_millis(10);
        let work = Duration::from_millis(2);
        let mut deadlines = Deadlines::new(origin, period);
        for n in 1..=100 {
            let ready = deadlines.acquire_at(work) + work;
            let presented = ready.max(deadlines.next());
            assert_eq!(presented, origin + period * n);
            deadlines.presented();
        }
        assert_eq!(
            deadlines.acquire_at(Duration::from_secs(1)),
            deadlines.next() - period / 2
        );
    }

    #[test]
    fn endpoint_to_interior_reserves_preparation_headroom() {
        let deadlines = Deadlines::new(Instant::now(), Duration::from_nanos(16_666_667));
        let ready = deadlines.acquire_at(Duration::from_micros(50)) + Duration::from_millis(3);
        assert!(ready <= deadlines.next());
    }
}
