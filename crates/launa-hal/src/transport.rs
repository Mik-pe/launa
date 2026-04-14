/// Transport abstraction for reading/writing bytes to the spa controller.
pub trait Transport {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, TransportError>;
    fn write(&mut self, data: &[u8]) -> Result<(), TransportError>;
    fn flush(&mut self) -> Result<(), TransportError>;
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
        fn read(&mut self, buf: &mut [u8]) -> Result<usize, super::TransportError> {
            let n = self.incoming.len().min(buf.len());
            for byte in buf.iter_mut().take(n) {
                *byte = self.incoming.pop_front().unwrap();
            }
            Ok(n)
        }

        fn write(&mut self, data: &[u8]) -> Result<(), super::TransportError> {
            self.outgoing.extend_from_slice(data);
            Ok(())
        }

        fn flush(&mut self) -> Result<(), super::TransportError> {
            Ok(())
        }
    }
}
