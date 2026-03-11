use serde::{Deserialize, Serialize};

pub const MAX_FRAME_LEN: usize = 4 * 1024 * 1024;

/// Messages sent from weft-appd to weft-compositor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppdToCompositor {
    AppSurfaceCreated {
        app_id: String,
        session_id: u64,
        pid: u32,
    },
    AppSurfaceDestroyed {
        session_id: u64,
    },
    AppFocusRequest {
        session_id: u64,
    },
}

/// Messages sent from weft-compositor to weft-appd.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CompositorToAppd {
    SurfaceReady { session_id: u64 },
    ClientDisconnected { pid: u32 },
}

/// Encode a message as a length-prefixed MessagePack frame.
///
/// Frame layout: 4-byte little-endian length followed by the MessagePack body.
/// Returns an error if the serialized length exceeds [`MAX_FRAME_LEN`].
pub fn frame_encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, rmp_serde::encode::Error> {
    let body = rmp_serde::to_vec_named(msg)?;
    assert!(
        body.len() <= MAX_FRAME_LEN,
        "IPC frame exceeds MAX_FRAME_LEN ({} > {})",
        body.len(),
        MAX_FRAME_LEN,
    );
    let len = body.len() as u32;
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decode a single MessagePack frame from a byte slice.
///
/// `buf` must contain exactly one complete frame (4-byte header + body).
/// Returns an error if the declared length does not match `buf.len() - 4`
/// or if the MessagePack body cannot be deserialized into `T`.
pub fn frame_decode<'de, T: Deserialize<'de>>(buf: &'de [u8]) -> Result<T, FrameDecodeError> {
    if buf.len() < 4 {
        return Err(FrameDecodeError::TooShort);
    }
    let declared_len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let body = &buf[4..];
    if body.len() != declared_len {
        return Err(FrameDecodeError::LengthMismatch {
            declared: declared_len,
            actual: body.len(),
        });
    }
    if declared_len > MAX_FRAME_LEN {
        return Err(FrameDecodeError::TooLong(declared_len));
    }
    rmp_serde::from_slice(body).map_err(FrameDecodeError::Deserialize)
}

#[derive(Debug)]
pub enum FrameDecodeError {
    TooShort,
    TooLong(usize),
    LengthMismatch { declared: usize, actual: usize },
    Deserialize(rmp_serde::decode::Error),
}

impl std::fmt::Display for FrameDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort => write!(f, "frame buffer shorter than 4-byte header"),
            Self::TooLong(n) => write!(f, "frame length {n} exceeds MAX_FRAME_LEN"),
            Self::LengthMismatch { declared, actual } => {
                write!(f, "declared length {declared} != actual body length {actual}")
            }
            Self::Deserialize(e) => write!(f, "MessagePack deserialize error: {e}"),
        }
    }
}

impl std::error::Error for FrameDecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Deserialize(e) => Some(e),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_appd_to_compositor_surface_created() {
        let msg = AppdToCompositor::AppSurfaceCreated {
            app_id: "com.example.app".into(),
            session_id: 42,
            pid: 1234,
        };
        let frame = frame_encode(&msg).unwrap();
        let decoded: AppdToCompositor = frame_decode(&frame).unwrap();
        match decoded {
            AppdToCompositor::AppSurfaceCreated { app_id, session_id, pid } => {
                assert_eq!(app_id, "com.example.app");
                assert_eq!(session_id, 42);
                assert_eq!(pid, 1234);
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn roundtrip_appd_to_compositor_destroyed() {
        let msg = AppdToCompositor::AppSurfaceDestroyed { session_id: 7 };
        let frame = frame_encode(&msg).unwrap();
        let decoded: AppdToCompositor = frame_decode(&frame).unwrap();
        match decoded {
            AppdToCompositor::AppSurfaceDestroyed { session_id } => assert_eq!(session_id, 7),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn roundtrip_appd_to_compositor_focus() {
        let msg = AppdToCompositor::AppFocusRequest { session_id: 99 };
        let frame = frame_encode(&msg).unwrap();
        let decoded: AppdToCompositor = frame_decode(&frame).unwrap();
        match decoded {
            AppdToCompositor::AppFocusRequest { session_id } => assert_eq!(session_id, 99),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn roundtrip_compositor_to_appd_surface_ready() {
        let msg = CompositorToAppd::SurfaceReady { session_id: 3 };
        let frame = frame_encode(&msg).unwrap();
        let decoded: CompositorToAppd = frame_decode(&frame).unwrap();
        match decoded {
            CompositorToAppd::SurfaceReady { session_id } => assert_eq!(session_id, 3),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn roundtrip_compositor_to_appd_disconnected() {
        let msg = CompositorToAppd::ClientDisconnected { pid: 5678 };
        let frame = frame_encode(&msg).unwrap();
        let decoded: CompositorToAppd = frame_decode(&frame).unwrap();
        match decoded {
            CompositorToAppd::ClientDisconnected { pid } => assert_eq!(pid, 5678),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn frame_decode_rejects_too_short_buffer() {
        let buf = [0u8; 3];
        let result: Result<AppdToCompositor, _> = frame_decode(&buf);
        assert!(matches!(result, Err(FrameDecodeError::TooShort)));
    }

    #[test]
    fn frame_decode_rejects_length_mismatch() {
        let mut frame = frame_encode(&AppdToCompositor::AppFocusRequest { session_id: 1 }).unwrap();
        // Corrupt the declared length to be larger than the actual body.
        let body_len = (frame.len() - 4) as u32;
        let bad_len = (body_len + 10).to_le_bytes();
        frame[0] = bad_len[0];
        frame[1] = bad_len[1];
        frame[2] = bad_len[2];
        frame[3] = bad_len[3];
        let result: Result<AppdToCompositor, _> = frame_decode(&frame);
        assert!(matches!(result, Err(FrameDecodeError::LengthMismatch { .. })));
    }
}
