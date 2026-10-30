//! Location resolution for client location redirection in RDP sessions.

pub const SYNC_HOST_LOCATION: &str = "sync";
pub const DISABLED_LOCATION: &str = "disabled";

/// Common locations available for selection in preferences.
pub const COMMON_LOCATIONS: &[(&str, &str, f64, f64)] = &[
    ("sync", "Sync with Host OS (Default)", 0.0, 0.0),
    ("new-york", "New York, USA", 40.7128, -74.0060),
    ("washington", "Washington D.C., USA", 38.9072, -77.0369),
    ("san-francisco", "San Francisco, USA", 37.7749, -122.4194),
    ("seattle", "Seattle, USA", 47.6062, -122.3321),
    ("london", "London, UK", 51.5074, -0.1278),
    ("paris", "Paris, France", 48.8566, 2.3522),
    ("frankfurt", "Frankfurt, Germany", 50.1109, 8.6821),
    ("amsterdam", "Amsterdam, Netherlands", 52.3676, 4.9041),
    ("dublin", "Dublin, Ireland", 53.3498, -6.2603),
    ("tokyo", "Tokyo, Japan", 35.6762, 139.6503),
    ("singapore", "Singapore", 1.3521, 103.8198),
    ("sydney", "Sydney, Australia", -33.8688, 151.2093),
    ("mumbai", "Mumbai, India", 19.0760, 72.8777),
    ("disabled", "Disabled (Do not redirect)", 0.0, 0.0),
];

/// Returns `(redirect_enabled, optional_coordinates)` based on preference.
#[must_use]
pub fn resolve_location(preference: &str) -> (bool, Option<(f64, f64)>) {
    let trimmed = preference.trim();
    if trimmed.eq_ignore_ascii_case(DISABLED_LOCATION) || trimmed.eq_ignore_ascii_case("off") {
        (false, None)
    } else if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case(SYNC_HOST_LOCATION)
        || trimmed.eq_ignore_ascii_case("auto")
    {
        (true, None)
    } else if let Some((_, _, lat, lon)) = COMMON_LOCATIONS
        .iter()
        .find(|(id, _, _, _)| id.eq_ignore_ascii_case(trimmed))
    {
        (true, Some((*lat, *lon)))
    } else {
        (true, None)
    }
}

/// Returns a human-readable label for a location preference id.
#[must_use]
pub fn location_label(preference: &str) -> &'static str {
    let trimmed = preference.trim();
    for (id, label, _, _) in COMMON_LOCATIONS {
        if id.eq_ignore_ascii_case(trimmed) {
            return label;
        }
    }
    "Sync with Host OS (Default)"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_location_preference() {
        assert_eq!(resolve_location("sync"), (true, None));
        assert_eq!(resolve_location(""), (true, None));
        assert_eq!(resolve_location("disabled"), (false, None));
        assert_eq!(
            resolve_location("new-york"),
            (true, Some((40.7128, -74.0060)))
        );
        assert_eq!(resolve_location("london"), (true, Some((51.5074, -0.1278))));
    }
}
