#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceMetadata {
    pub package_name: &'static str,
    pub package_version: &'static str,
}

#[must_use]
pub const fn current() -> WorkspaceMetadata {
    WorkspaceMetadata {
        package_name: env!("CARGO_PKG_NAME"),
        package_version: env!("CARGO_PKG_VERSION"),
    }
}

#[cfg(test)]
mod tests {
    use super::{WorkspaceMetadata, current};

    #[test]
    fn reports_workspace_metadata() {
        assert_eq!(
            current(),
            WorkspaceMetadata {
                package_name: "weft-build-meta",
                package_version: env!("CARGO_PKG_VERSION"),
            }
        );
    }
}
