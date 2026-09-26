//! The text filter the side panel, the events list and the parameter
//! table share: a case-insensitive substring match, blank matching all.

/// A filter as typed, lowercased once; a candidate is compared in place,
/// without a copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    needle: String,
}

impl Filter {
    #[must_use]
    pub fn new(text: &str) -> Self {
        Self {
            needle: text.trim().to_ascii_lowercase(),
        }
    }

    /// Whether the filter is blank, and so matches everything.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.needle.is_empty()
    }

    #[must_use]
    pub fn matches(&self, text: &str) -> bool {
        let needle = self.needle.as_bytes();
        self.is_blank()
            || text
                .as_bytes()
                .windows(needle.len())
                .any(|window| window.eq_ignore_ascii_case(needle))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_matches_everything_and_text_matches_anywhere_ignoring_case() {
        for blank in ["", "  ", "\t"] {
            let filter = Filter::new(blank);
            assert!(filter.is_blank());
            assert!(filter.matches("anything"));
            assert!(filter.matches(""));
        }
        let filter = Filter::new("  GyR ");
        assert!(!filter.is_blank());
        assert!(filter.matches("IMU.GyrX"));
        assert!(filter.matches("gyr"));
        assert!(!filter.matches("IMU.AccX"));
        assert!(!filter.matches(""));
        assert!(!filter.matches("gy"), "shorter than the filter");
        assert!(Filter::new("GyrX").matches("gyrx"), "the whole text");
    }
}
