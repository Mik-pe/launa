/// Transport abstraction for reading/writing bytes to the spa controller.
///
/// Uses async methods compatible with `embedded_io_async` signatures,
/// enabling a unified abstraction across production (ESP32 UART) and
/// test (mock/sim) transports.
#[allow(async_fn_in_trait)] // Intentional: trait is used only within this crate's ecosystem
pub trait Transport {
    /// Read bytes into `buf`, returning the number of bytes read.
    /// Returns 0 when no data is available (non-blocking semantics).
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, TransportError>;

    /// Write all bytes in `data`.
    async fn write(&mut self, data: &[u8]) -> Result<(), TransportError>;

    /// Flush any buffered write data.
    async fn flush(&mut self) -> Result<(), TransportError>;
}

#[derive(Debug)]
pub enum TransportError {
    Io,
    Timeout,
    BufferTooSmall,
}

#[cfg(feature = "std")]
pub mod mock {
    use std::collections::VecDeque;

    /// A mock transport that simulates bidirectional serial communication.
    /// Supports injecting incoming bytes and capturing outgoing bytes.
    ///
    /// All async methods complete immediately (poll-ready) since this is
    /// a synchronous in-memory mock.
    pub struct MockTransport {
        incoming: VecDeque<u8>,
        outgoing: Vec<u8>,
    }

    impl MockTransport {
        pub fn new() -> Self {
            MockTransport {
                incoming: VecDeque::new(),
                outgoing: Vec::new(),
            }
        }

        /// Queue bytes that will be returned by subsequent read() calls
        pub fn inject(&mut self, data: &[u8]) {
            self.incoming.extend(data.iter().copied());
        }

        /// Get all bytes written since last clear
        pub fn written(&self) -> &[u8] {
            &self.outgoing
        }

        /// Clear the outgoing buffer
        pub fn clear_written(&mut self) {
            self.outgoing.clear();
        }

        /// Returns true if there are incoming bytes available
        pub fn has_incoming(&self) -> bool {
            !self.incoming.is_empty()
        }
    }

    impl super::Transport for MockTransport {
        async fn read(&mut self, buf: &mut [u8]) -> Result<usize, super::TransportError> {
            let n = self.incoming.len().min(buf.len());
            for byte in buf.iter_mut().take(n) {
                *byte = self.incoming.pop_front().unwrap();
            }
            Ok(n)
        }

        async fn write(&mut self, data: &[u8]) -> Result<(), super::TransportError> {
            self.outgoing.extend_from_slice(data);
            Ok(())
        }

        async fn flush(&mut self) -> Result<(), super::TransportError> {
            Ok(())
        }
    }
}
