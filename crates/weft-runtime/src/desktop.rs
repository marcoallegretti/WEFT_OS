//! The desktop services behind the clipboard and notification imports.
//!
//! Each import runs a helper program (`wl-paste`, `wl-copy`,
//! `notify-send`) with a bounded input, a bounded output and a deadline,
//! after which the helper is killed and the call fails. A notification
//! names the application that sent it, so an app cannot pass one off as
//! the system's.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long a helper may run.
const HELPER_TIMEOUT: Duration = Duration::from_secs(5);
/// The most clipboard text read or written, in bytes.
pub const MAX_CLIPBOARD: usize = 1024 * 1024;
/// The longest notification title and body, in bytes.
pub const MAX_TITLE: usize = 256;
pub const MAX_BODY: usize = 4096;

/// What a helper's output streams are connected to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Output {
    /// Read: up to the limit from standard output, and standard error for
    /// the failure message.
    Captured,
    /// Discarded, for a helper that leaves a process running which would
    /// keep the streams open, as `wl-copy` does to serve the selection.
    Discarded,
}

/// Runs `command` with `input` on its standard input and returns at most
/// `max_output` bytes of its standard output, or why it failed.
fn run(
    mut command: Command,
    input: Option<Vec<u8>>,
    max_output: usize,
    output: Output,
) -> Result<Vec<u8>, String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let stream = || match output {
        Output::Captured => Stdio::piped(),
        Output::Discarded => Stdio::null(),
    };
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(stream())
        .stderr(stream())
        .spawn()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    if let (Some(mut stdin), Some(input)) = (child.stdin.take(), input) {
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let reader = |stream: Option<Box<dyn Read + Send>>, limit: usize| {
        std::thread::spawn(move || {
            let mut data = Vec::new();
            if let Some(stream) = stream {
                let _ = stream.take(limit as u64 + 1).read_to_end(&mut data);
            }
            data
        })
    };
    let stdout = reader(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
        max_output,
    );
    let stderr = reader(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
        4096,
    );
    let deadline = Instant::now() + HELPER_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{program} did not finish within {} s",
                    HELPER_TIMEOUT.as_secs()
                ));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    let output = stdout.join().unwrap_or_default();
    if !status.success() {
        let message = String::from_utf8_lossy(&stderr.join().unwrap_or_default())
            .trim()
            .to_owned();
        return Err(if message.is_empty() {
            format!("{program} exited with {status}")
        } else {
            message
        });
    }
    if output.len() > max_output {
        return Err(format!("{program} returned more than {max_output} bytes"));
    }
    Ok(output)
}

pub fn clipboard_read() -> Result<String, String> {
    let mut command = Command::new("wl-paste");
    command.args(["--no-newline", "--type", "text/plain;charset=utf-8"]);
    let data = run(command, None, MAX_CLIPBOARD, Output::Captured)?;
    String::from_utf8(data).map_err(|_| "the clipboard does not hold UTF-8 text".to_owned())
}

pub fn clipboard_write(text: &str) -> Result<(), String> {
    if text.len() > MAX_CLIPBOARD {
        return Err(format!("clipboard text exceeds {MAX_CLIPBOARD} bytes"));
    }
    let mut command = Command::new("wl-copy");
    command.args(["--type", "text/plain;charset=utf-8"]);
    run(
        command,
        Some(text.as_bytes().to_vec()),
        0,
        Output::Discarded,
    )
    .map(|_| ())
}

/// Checks a notification before it is sent.
pub fn check_notification(title: &str, body: &str, icon: Option<&str>) -> Result<(), String> {
    if title.len() > MAX_TITLE {
        return Err(format!("notification title exceeds {MAX_TITLE} bytes"));
    }
    if body.len() > MAX_BODY {
        return Err(format!("notification body exceeds {MAX_BODY} bytes"));
    }
    // Only icon theme names: a path or URL would let the app show any file
    // or reach the network through the notification service.
    if let Some(icon) = icon
        && !is_icon_name(icon)
    {
        return Err("a notification icon must be an icon theme name".to_owned());
    }
    Ok(())
}

fn is_icon_name(icon: &str) -> bool {
    !icon.is_empty()
        && icon.len() <= 64
        && !icon.starts_with('.')
        && icon
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Sends a notification attributed to `app_id`.
pub fn notify(app_id: &str, title: &str, body: &str, icon: Option<&str>) -> Result<(), String> {
    check_notification(title, body, icon)?;
    let mut command = Command::new("notify-send");
    command.arg(format!("--app-name={app_id}"));
    if let Some(icon) = icon {
        command.arg(format!("--icon={icon}"));
    }
    command.arg("--").arg(title).arg(body);
    run(command, None, 0, Output::Captured).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifications_are_bounded_and_icons_are_theme_names() {
        assert!(check_notification("t", "b", None).is_ok());
        assert!(check_notification("t", "b", Some("dialog-information")).is_ok());
        assert!(check_notification(&"t".repeat(MAX_TITLE + 1), "b", None).is_err());
        assert!(check_notification("t", &"b".repeat(MAX_BODY + 1), None).is_err());
        for icon in [
            "/etc/passwd",
            "file:///x.png",
            "https://x/i.png",
            "../x",
            "",
            "a b",
        ] {
            assert!(check_notification("t", "b", Some(icon)).is_err(), "{icon}");
        }
    }

    #[test]
    fn helpers_are_bounded_in_time_and_output() {
        let mut slow = Command::new("sleep");
        slow.arg("30");
        let started = Instant::now();
        let error = run(slow, None, 0, Output::Captured).unwrap_err();
        assert!(error.contains("did not finish"), "{error}");
        assert!(started.elapsed() < HELPER_TIMEOUT + Duration::from_secs(2));

        let mut chatty = Command::new("head");
        chatty.args(["-c", "2000", "/dev/zero"]);
        assert!(
            run(chatty, None, 1000, Output::Captured)
                .unwrap_err()
                .contains("more than 1000 bytes")
        );

        let mut echo = Command::new("cat");
        echo.stdin(Stdio::piped());
        assert_eq!(
            run(echo, Some(b"text".to_vec()), 100, Output::Captured).unwrap(),
            b"text"
        );

        // A helper that leaves a process behind returns once it exits
        // itself when its streams are discarded.
        let mut forking = Command::new("sh");
        forking.args(["-c", "sleep 30 & exit 0"]);
        let started = Instant::now();
        assert!(run(forking, None, 0, Output::Discarded).is_ok());
        assert!(started.elapsed() < Duration::from_secs(2));

        let failing = Command::new("false");
        assert!(
            run(failing, None, 0, Output::Captured)
                .unwrap_err()
                .contains("exited")
        );
    }
}
