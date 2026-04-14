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
    use std::io::{Read, Cursor};
    use std::sync::{Arc, Mutex};

    /// Mock transport for desktop testing. Pre-load with expected responses.
    pub struct MockTransport {
        incoming: Arc<Mutex<Cursor<Vec<u8>>>>,
        outgoing: Arc<Mutex<Vec<u8>>>,
    }

    impl MockTransport {
        pub fn new() -> Self {
            MockTransport {
                incoming: Arc::new(Mutex::new(Cursor::new(Vec::new()))),
                outgoing: Arc::new(Mutex::new(Vec::new())),
            }
        }

        /// Pre-load bytes that will be returned by `read`.
        pub fn inject_incoming(&self, data: &[u8]) {
            let mut incoming = self.incoming.lock().unwrap();
            let pos = incoming.position() as usize;
            let mut buf = incoming.get_ref().clone();
            buf.extend_from_slice(data);
            *incoming = Cursor::new(buf);
            incoming.set_position(pos as u64);
        }

        /// Get all bytes written via `write`.
        pub fn get_outgoing(&self) -> Vec<u8> {
            self.outgoing.lock().unwrap().clone()
        }
    }

    impl super::Transport for MockTransport {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize, super::TransportError> {
            let mut incoming = self.incoming.lock().unwrap();
            incoming.read(buf).map_err(|_| super::TransportError::Io)
        }

        fn write(&mut self, data: &[u8]) -> Result<(), super::TransportError> {
            let mut outgoing = self.outgoing.lock().unwrap();
            outgoing.extend_from_slice(data);
            Ok(())
        }

        fn flush(&mut self) -> Result<(), super::TransportError> {
            Ok(())
        }
    }
}
