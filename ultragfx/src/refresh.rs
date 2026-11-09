use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum RefreshRate {
    #[default]
    Original,
    Display,
    Manual(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshRateError {
    InvalidManualRate,
    InvalidDisplayRate,
    UnsupportedTarget,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct RefreshPolicy {
    pub(crate) requested: RefreshRate,
    pub(crate) display_millihz: Option<u32>,
}

impl RefreshPolicy {
    pub(crate) fn effective_hz(self) -> Option<f64> {
        let display = f64::from(self.display_millihz.unwrap_or(60_000)) / 1000.0;
        match self.requested {
            RefreshRate::Original => None,
            RefreshRate::Display => Some(display),
            RefreshRate::Manual(hz) => Some(hz.min(display)),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn period(self) -> Option<Duration> {
        self.effective_hz()
            .map(|hz| Duration::from_secs_f64(1.0 / hz))
    }
}

impl RefreshRate {
    /// Validate a setting before a renderer exists.
    pub fn validate(self) -> Result<(), RefreshRateError> {
        if let Self::Manual(hz) = self {
            let period = Duration::try_from_secs_f64(1.0 / hz)
                .map_err(|_| RefreshRateError::InvalidManualRate)?;
            if !hz.is_finite() || hz <= 0.0 || period.is_zero() {
                return Err(RefreshRateError::InvalidManualRate);
            }
            #[cfg(not(target_arch = "wasm32"))]
            if std::time::Instant::now().checked_add(period).is_none() {
                return Err(RefreshRateError::InvalidManualRate);
            }
        }
        Ok(())
    }
}

impl crate::Renderer {
    /// Queue a presentation rate for the next scanout. Retention remains per-task opt-in.
    pub fn set_refresh_rate(&mut self, rate: RefreshRate) -> Result<(), RefreshRateError> {
        rate.validate()?;
        if rate != RefreshRate::Original {
            #[cfg(target_arch = "wasm32")]
            return Err(RefreshRateError::UnsupportedTarget);
            #[cfg(not(target_arch = "wasm32"))]
            if self.presenter.is_none() {
                return Err(RefreshRateError::UnsupportedTarget);
            }
        }
        self.refresh_requested.requested = rate;
        Ok(())
    }

    /// Queue nominal monitor millihertz; `None` explicitly uses the 60 Hz fallback.
    pub fn set_display_refresh_rate(
        &mut self,
        millihz: Option<u32>,
    ) -> Result<(), RefreshRateError> {
        if millihz == Some(0) {
            return Err(RefreshRateError::InvalidDisplayRate);
        }
        self.refresh_requested.display_millihz = millihz;
        Ok(())
    }

    pub fn requested_refresh_rate(&self) -> RefreshRate {
        self.refresh_requested.requested
    }

    /// Effective Hz at the last scanout, including the display clamp. Original has no rate.
    pub fn effective_refresh_rate(&self) -> Option<f64> {
        self.refresh_latched.effective_hz()
    }

    pub fn display_refresh_rate(&self) -> Option<u32> {
        self.refresh_latched.display_millihz
    }

    /// Whether the last scanout selected interpolation; hosts may omit annotations when false.
    pub fn interpolation_active(&self) -> bool {
        self.refresh_latched.requested != RefreshRate::Original
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clamps_without_losing_the_requested_rate() {
        let mut policy = RefreshPolicy {
            requested: RefreshRate::Manual(120.0),
            display_millihz: None,
        };
        assert_eq!(policy.effective_hz(), Some(60.0));
        policy.display_millihz = Some(59940);
        assert_eq!(policy.effective_hz(), Some(59.94));
        assert_eq!(policy.requested, RefreshRate::Manual(120.0));
        policy.display_millihz = Some(144000);
        assert_eq!(policy.effective_hz(), Some(120.0));
        assert_eq!(policy.period(), Some(Duration::from_nanos(8_333_333)));
    }

    #[test]
    fn arbitrary_rates_and_original_have_distinct_periods() {
        let mut policy = RefreshPolicy::default();
        assert_eq!(policy.period(), None);
        policy.requested = RefreshRate::Display;
        assert_eq!(policy.period(), Some(Duration::from_nanos(16_666_667)));
        policy.display_millihz = Some(59940);
        assert_eq!(policy.period(), Some(Duration::from_nanos(16_683_350)));
        policy.requested = RefreshRate::Manual(24.0);
        assert_eq!(policy.period(), Some(Duration::from_nanos(41_666_667)));
    }

    #[test]
    fn invalid_manual_rates_are_rejected() {
        for hz in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::MIN_POSITIVE, 1e20] {
            assert_eq!(
                RefreshRate::Manual(hz).validate(),
                Err(RefreshRateError::InvalidManualRate)
            );
        }
        for rate in [
            RefreshRate::Original,
            RefreshRate::Display,
            RefreshRate::Manual(59.94),
        ] {
            assert_eq!(rate.validate(), Ok(()));
        }
    }
}
