//! Ordered RPC stdout writer. One mutable owner serializes logical frames.
//!
//! A completed receipt means every physical line was accepted by the
//! `AsyncWrite` and flushed. It does not assert that a peer processed the frame.
//! On a partial write or broken pipe, the writer stops: a later frame must not
//! be appended to an incomplete chunk sequence.

use std::io;

use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::frame::{RpcError, RpcFrameEncoder};
use crate::json::WireValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteReceipt {
    pub physical_frames: usize,
    pub bytes_written: usize,
}

pub struct RpcOutput<W> {
    writer: W,
    encoder: RpcFrameEncoder,
    failed: bool,
}

impl<W: AsyncWrite + Unpin> RpcOutput<W> {
    pub fn new(writer: W) -> Self {
        Self { writer, encoder: RpcFrameEncoder::new(), failed: false }
    }

    /// Switch only after the successful negotiation response has been
    /// submitted. Exclusive mutable access prevents interleaving that response
    /// with the next version's physical frames.
    pub fn set_protocol_version(&mut self, version: u8) -> Result<(), RpcError> {
        self.encoder.set_protocol_version(version)
    }

    /// Write one logical frame in physical-line order, honoring backpressure.
    /// No receipt is issued if any line or the final flush fails.
    pub async fn write_frame(&mut self, frame: &WireValue) -> io::Result<WriteReceipt> {
        if self.failed {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "RPC output failed previously"));
        }
        // Encoding changes snapshot state eagerly. Dropping this future may
        // leave a partial line or chunk behind, so only a completed flush
        // reopens this owner for the next logical frame.
        self.failed = true;
        let mut physical_frames = 0;
        let mut bytes_written = 0;
        for line in self.encoder.encode_frames(frame) {
            self.writer.write_all(line.as_bytes()).await?;
            physical_frames += 1;
            bytes_written += line.len();
        }
        self.writer.flush().await?;
        self.failed = false;
        Ok(WriteReceipt { physical_frames, bytes_written })
    }

    pub fn into_inner(self) -> W {
        self.writer
    }
}
