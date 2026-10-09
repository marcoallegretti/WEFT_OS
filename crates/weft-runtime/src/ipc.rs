//! The component's connection to its session's IPC relay in weft-appd.
//!
//! Messages are single lines. A message is at most `MAX_MESSAGE` bytes, the
//! limit weft-appd applies to app messages, and cannot contain a line break,
//! which would split it into several messages. Received data is buffered
//! up to one message; a peer that sends a longer line, or closes the
//! connection, ends it for good.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

/// The longest message in either direction, in bytes.
pub const MAX_MESSAGE: usize = 64 * 1024;

/// Checks a message the component wants to send.
pub fn check_payload(payload: &str) -> Result<(), String> {
    if payload.len() > MAX_MESSAGE {
        return Err(format!("message exceeds {MAX_MESSAGE} bytes"));
    }
    if payload.contains(['\n', '\r']) {
        return Err("message contains a line break".to_owned());
    }
    Ok(())
}

pub struct IpcState {
    socket: UnixStream,
    recv_buf: Vec<u8>,
    /// Why the connection ended, once it has.
    closed: Option<&'static str>,
}

impl IpcState {
    pub fn connect(path: &str) -> Option<Self> {
        let socket = UnixStream::connect(path).ok()?;
        Self::new(socket)
    }

    fn new(socket: UnixStream) -> Option<Self> {
        socket.set_nonblocking(true).ok()?;
        Some(Self {
            socket,
            recv_buf: Vec::new(),
            closed: None,
        })
    }

    pub fn send(&mut self, payload: &str) -> Result<(), String> {
        check_payload(payload)?;
        if let Some(reason) = self.closed {
            return Err(format!("IPC connection {reason}"));
        }
        let _ = self.socket.set_nonblocking(false);
        let mut line = payload.to_owned();
        line.push('\n');
        let result = self
            .socket
            .write_all(line.as_bytes())
            .map_err(|e| e.to_string());
        let _ = self.socket.set_nonblocking(true);
        result
    }

    /// The next complete message, if one has arrived.
    pub fn recv(&mut self) -> Option<String> {
        if let Some(message) = self.take_line() {
            return Some(message);
        }
        if self.closed.is_some() {
            return None;
        }
        let mut chunk = [0u8; 4096];
        loop {
            match self.socket.read(&mut chunk) {
                Ok(0) => {
                    self.close("closed by weft-appd");
                    break;
                }
                Ok(n) => {
                    self.recv_buf.extend_from_slice(&chunk[..n]);
                    if self.recv_buf.contains(&b'\n') {
                        break;
                    }
                    if self.recv_buf.len() > MAX_MESSAGE {
                        self.close("ended by a message over the size limit");
                        self.recv_buf.clear();
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    self.close("failed");
                    break;
                }
            }
        }
        self.take_line()
    }

    fn close(&mut self, reason: &'static str) {
        tracing::warn!(reason, "IPC connection ended");
        self.closed = Some(reason);
    }

    /// Removes the first complete line from the buffer. A line that is not
    /// UTF-8, or longer than a message, is dropped.
    fn take_line(&mut self) -> Option<String> {
        loop {
            let pos = self.recv_buf.iter().position(|&b| b == b'\n')?;
            let raw: Vec<u8> = self.recv_buf.drain(..=pos).collect();
            let text = raw.strip_suffix(b"\n").unwrap_or(&raw);
            let text = text.strip_suffix(b"\r").unwrap_or(text);
            if text.len() > MAX_MESSAGE {
                continue;
            }
            if let Ok(message) = String::from_utf8(text.to_vec()) {
                return Some(message);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (IpcState, UnixStream) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        (IpcState::new(ours).unwrap(), theirs)
    }

    #[test]
    fn payloads_that_would_break_framing_are_refused() {
        assert!(check_payload("ok").is_ok());
        assert!(check_payload("two\nmessages").is_err());
        assert!(check_payload("carriage\rreturn").is_err());
        assert!(check_payload(&"x".repeat(MAX_MESSAGE)).is_ok());
        assert!(check_payload(&"x".repeat(MAX_MESSAGE + 1)).is_err());
        let (mut ipc, _peer) = pair();
        assert!(ipc.send("a\nb").is_err());
    }

    #[test]
    fn messages_arrive_one_at_a_time() {
        let (mut ipc, mut peer) = pair();
        assert_eq!(ipc.recv(), None);
        peer.write_all(b"first\nsecond\r\npart").unwrap();
        assert_eq!(ipc.recv().as_deref(), Some("first"));
        assert_eq!(ipc.recv().as_deref(), Some("second"));
        assert_eq!(ipc.recv(), None);
        peer.write_all(b"ial\n").unwrap();
        assert_eq!(ipc.recv().as_deref(), Some("partial"));
    }

    #[test]
    fn an_overlong_line_ends_the_connection_without_unbounded_buffering() {
        let (mut ipc, mut peer) = pair();
        peer.write_all(&vec![b'x'; MAX_MESSAGE + 4097]).unwrap();
        while ipc.closed.is_none() {
            assert_eq!(ipc.recv(), None);
        }
        assert!(ipc.recv_buf.len() <= MAX_MESSAGE + 4096);
        assert!(ipc.send("after").is_err());
    }

    #[test]
    fn a_closed_connection_is_reported_to_senders() {
        let (mut ipc, peer) = pair();
        drop(peer);
        assert_eq!(ipc.recv(), None);
        let error = ipc.send("hello").unwrap_err();
        assert!(error.contains("closed"), "{error}");
    }
}
