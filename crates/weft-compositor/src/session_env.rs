//! Tells the systemd user manager which Wayland socket the compositor
//! serves, so the services started after it, the shell and weft-appd and
//! through it the app shells, connect to this compositor. Only done when
//! systemd started the compositor; its own unit unsets the variable, so a
//! restarted compositor does not mistake the value for an enclosing
//! Wayland session.

use std::ffi::{OsStr, OsString};
use std::process::{Command, Stdio};

/// Publishes `socket_name` as `WAYLAND_DISPLAY` for the units the user
/// manager starts from now on, when running under systemd.
pub fn publish_wayland_display(socket_name: &OsStr) {
    if std::env::var_os("NOTIFY_SOCKET").is_none() {
        return;
    }
    publish_with(OsStr::new("systemctl"), socket_name);
}

fn publish_with(systemctl: &OsStr, socket_name: &OsStr) -> bool {
    let mut assignment = OsString::from("WAYLAND_DISPLAY=");
    assignment.push(socket_name);
    let status = Command::new(systemctl)
        .args(["--user", "set-environment"])
        .arg(&assignment)
        .stdin(Stdio::null())
        .status();
    match status {
        Ok(status) if status.success() => {
            tracing::info!(
                ?socket_name,
                "WAYLAND_DISPLAY published to the user manager"
            );
            true
        }
        Ok(status) => {
            tracing::warn!(%status, "cannot publish WAYLAND_DISPLAY to the user manager");
            false
        }
        Err(e) => {
            tracing::warn!(error = %e, "cannot publish WAYLAND_DISPLAY to the user manager");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn the_socket_name_is_set_in_the_user_manager() {
        let dir = std::env::temp_dir().join(format!("weft_session_env_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let record = dir.join("args");
        let fake = dir.join("systemctl");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n",
                record.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(publish_with(fake.as_os_str(), OsStr::new("wayland-3")));
        assert_eq!(
            std::fs::read_to_string(&record).unwrap(),
            "--user\nset-environment\nWAYLAND_DISPLAY=wayland-3\n"
        );

        let failing = dir.join("failing");
        std::fs::write(&failing, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&failing, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!publish_with(failing.as_os_str(), OsStr::new("wayland-3")));
        assert!(!publish_with(
            dir.join("missing").as_os_str(),
            OsStr::new("wayland-3")
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
