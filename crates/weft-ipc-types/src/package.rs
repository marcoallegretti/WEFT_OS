//! Package identity rules shared by the packaging tool and the supervisor.

/// Whether `id` is a valid application ID: at least three dot-separated
/// segments, each starting with a lowercase ASCII letter and containing only
/// lowercase ASCII letters and digits. A valid ID is safe to use as a single
/// path component.
pub fn is_valid_app_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('.').collect();
    parts.len() >= 3
        && parts.iter().all(|p| {
            p.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                && p.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_ids() {
        assert!(is_valid_app_id("org.weft.demo"));
        assert!(is_valid_app_id("com.example2.app3"));
        for bad in [
            "",
            "org.weft",
            "Org.weft.demo",
            "org..demo",
            "../x.y.z",
            "org.weft.a::b",
            "org.1weft.demo",
        ] {
            assert!(!is_valid_app_id(bad), "{bad:?}");
        }
    }
}
