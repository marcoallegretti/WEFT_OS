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
/// How long one write of a message may block before the connection is
/// given up.
const SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

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
        let mut line = payload.to_owned();
        line.push('\n');
        let result = self
            .socket
            .set_nonblocking(false)
            .and_then(|()| self.socket.set_write_timeout(Some(SEND_TIMEOUT)))
            .and_then(|()| self.socket.write_all(line.as_bytes()));
        let restored = self.socket.set_nonblocking(true);
        match result.and(restored) {
            Ok(()) => Ok(()),
            // Part of the line may have been written; anything sent after it
            // would join it, so the connection ends.
            Err(e) => {
                self.close("failed");
                Err(e.to_string())
            }
        }
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
                    if self.recv_buf.len() > MAX_MESSAGE + 2 {
                        self.overlong();
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

    /// Ends the connection for both sides: weft-appd sees it close and
    /// ends the session, as it does when the component closes it.
    fn close(&mut self, reason: &'static str) {
        tracing::warn!(reason, "IPC connection ended");
        self.closed = Some(reason);
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
    }

    fn overlong(&mut self) {
        self.close("ended by a message over the size limit");
        self.recv_buf.clear();
    }

    /// Removes the first complete line from the buffer. A line that is not
    /// UTF-8 is dropped; one longer than a message ends the connection.
    fn take_line(&mut self) -> Option<String> {
        loop {
            let pos = self.recv_buf.iter().position(|&b| b == b'\n')?;
            let raw: Vec<u8> = self.recv_buf.drain(..=pos).collect();
            let text = raw.strip_suffix(b"\n").unwrap_or(&raw);
            let text = text.strip_suffix(b"\r").unwrap_or(text);
            if text.len() > MAX_MESSAGE {
                self.overlong();
                return None;
            }
            match String::from_utf8(text.to_vec()) {
                Ok(message) => return Some(message),
                Err(_) => tracing::warn!("IPC message that is not UTF-8 dropped"),
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
        // The peer sees the connection end.
        let mut rest = Vec::new();
        assert_eq!(peer.read_to_end(&mut rest).unwrap(), 0);
    }

    #[test]
    fn a_message_at_the_limit_arrives_and_one_over_it_ends_the_connection() {
        let (mut ipc, mut peer) = pair();
        let mut at_limit = vec![b'x'; MAX_MESSAGE];
        at_limit.push(b'\n');
        peer.write_all(&at_limit).unwrap();
        let mut received = None;
        while received.is_none() {
            received = ipc.recv();
        }
        assert_eq!(received.map(|m| m.len()), Some(MAX_MESSAGE));
        // One byte over, with its newline in the same read.
        let mut over = vec![b'x'; MAX_MESSAGE + 1];
        over.extend_from_slice(b"\nnext\n");
        peer.write_all(&over).unwrap();
        while ipc.closed.is_none() {
            assert_eq!(ipc.recv(), None);
        }
        assert_eq!(ipc.recv(), None);
    }

    #[test]
    fn a_line_that_is_not_utf8_is_dropped() {
        let (mut ipc, mut peer) = pair();
        peer.write_all(b"\xff\xfe\nafter\n").unwrap();
        assert_eq!(ipc.recv().as_deref(), Some("after"));
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
