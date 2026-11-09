use crate::{PresentError, Renderer};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PresentationFit {
    /// Stretch the image over the whole surface.
    #[default]
    Fill,
    /// Center the largest rectangle of this aspect ratio and clear the remainder to black.
    Aspect { width: u32, height: u32 },
}

impl PresentationFit {
    pub(crate) fn viewport(self, target_width: u32, target_height: u32) -> Option<[u32; 4]> {
        let Self::Aspect { width, height } = self else {
            return None;
        };
        let (width, height) = (u64::from(width), u64::from(height));
        let (tw, th) = (u64::from(target_width), u64::from(target_height));
        let (w, h) = if tw * height > th * width {
            (
                ((th * width + height / 2) / height).max(1) as u32,
                target_height,
            )
        } else {
            (
                target_width,
                ((tw * height + width / 2) / width).max(1) as u32,
            )
        };
        Some([(target_width - w) / 2, (target_height - h) / 2, w, h])
    }
}

impl Renderer {
    /// Queue the owned-surface fit for the next scanout. Caller-owned targets ignore it.
    pub fn set_presentation_fit(&mut self, fit: PresentationFit) -> Result<(), PresentError> {
        if matches!(
            fit,
            PresentationFit::Aspect { width: 0, .. } | PresentationFit::Aspect { height: 0, .. }
        ) {
            return Err(PresentError::InvalidPresentationFit);
        }
        self.presentation_fit = fit;
        Ok(())
    }

    pub fn presentation_fit(&self) -> PresentationFit {
        self.presentation_fit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentation_fit_rounds_half_up_centers_and_keeps_thin_rectangles_visible() {
        let fit = PresentationFit::Aspect {
            width: 4,
            height: 3,
        };
        for (width, height, expected) in [
            (1920, 1080, [240, 0, 1440, 1080]),
            (1080, 1920, [0, 555, 1080, 810]),
            (19, 10, [3, 0, 13, 10]),
            (10, 19, [0, 5, 10, 8]),
            (12, 9, [0, 0, 12, 9]),
            (1, 1, [0, 0, 1, 1]),
            (u32::MAX, u32::MAX, [0, 536870912, u32::MAX, 3221225471]),
        ] {
            assert_eq!(fit.viewport(width, height), Some(expected));
            assert_eq!(PresentationFit::Fill.viewport(width, height), None);
        }
        for (width, height, expected) in [
            (u32::MAX, 1, [0, 4, 10, 1]),
            (1, u32::MAX, [4, 0, 1, 10]),
            (u32::MAX, u32::MAX, [0, 0, 10, 10]),
        ] {
            assert_eq!(
                PresentationFit::Aspect { width, height }.viewport(10, 10),
                Some(expected)
            );
        }
    }
}
