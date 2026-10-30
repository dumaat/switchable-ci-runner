//! The CPU limit the owner picks in the tray, turned into what Windows' job
//! objects take: a rate in hundredths of a percent, or no cap at all.

/// The choices in the tray menu, in percent; 100 means no cap.
pub const CHOICES: [u8; 4] = [25, 50, 75, 100];

/// `None` means no cap. Anything outside 1..=99 is no cap too, so a bad
/// config value can never throttle the CI to nothing.
pub fn hard_cap_rate(percent: u8) -> Option<u32> {
    (1..=99)
        .contains(&percent)
        .then(|| u32::from(percent) * 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates() {
        assert_eq!(hard_cap_rate(25), Some(2_500));
        assert_eq!(hard_cap_rate(50), Some(5_000));
        assert_eq!(hard_cap_rate(99), Some(9_900));
        assert_eq!(hard_cap_rate(100), None);
        assert_eq!(hard_cap_rate(0), None);
        assert_eq!(hard_cap_rate(255), None);
    }

    #[test]
    fn every_menu_choice_is_valid() {
        for p in CHOICES {
            assert!(p == 100 || hard_cap_rate(p).is_some());
        }
    }
}
