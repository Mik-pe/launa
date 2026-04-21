/// Network abstraction for WiFi + TCP connectivity.
///
/// Only available with the `std` feature (for desktop testing).
/// The ESP32 app uses `embassy-net` directly instead of this trait.
pub trait Network {
    fn connect_wifi(&mut self, ssid: &str, password: &str) -> Result<(), NetworkError>;
    fn is_connected(&self) -> bool;
    fn tcp_connect(&mut self, addr: &str, port: u16) -> Result<Box<dyn TcpSocket>, NetworkError>;
}

/// TCP socket abstraction for network communication.
///
/// Used by the MQTT client to send/receive data over a TCP connection.
/// Implementations wrap platform-specific socket APIs.
pub trait TcpSocket {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, NetworkError>;
    fn write(&mut self, data: &[u8]) -> Result<(), NetworkError>;
    fn close(&mut self) -> Result<(), NetworkError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NetworkError {
    #[error("connection failed")]
    ConnectionFailed,
    #[error("timeout")]
    Timeout,
    #[error("DNS resolution failed")]
    DnsFailed,
    #[error("I/O error")]
    Io,
}

#[cfg(feature = "std")]
pub mod mock {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// A mock network for desktop testing with bidirectional TCP simulation.
    pub struct MockNetwork {
        connected: bool,
        pending_responses: VecDeque<Vec<u8>>,
        sent_data: Arc<Mutex<Vec<u8>>>,
        last_connect_addr: Option<String>,
        last_connect_port: Option<u16>,
    }

    impl MockNetwork {
        pub fn new() -> Self {
            MockNetwork {
                connected: false,
                pending_responses: VecDeque::new(),
                sent_data: Arc::new(Mutex::new(Vec::new())),
                last_connect_addr: None,
                last_connect_port: None,
            }
        }

        /// Queue a response that will be available to the next TCP connection's read.
        pub fn queue_response(&mut self, data: Vec<u8>) {
            self.pending_responses.push_back(data);
        }

        /// Get all bytes sent across all TCP connections.
        pub fn get_sent_data(&self) -> Vec<u8> {
            self.sent_data.lock().unwrap().clone()
        }

        /// Clear all accumulated sent data.
        pub fn clear_sent_data(&mut self) {
            self.sent_data.lock().unwrap().clear();
        }

        /// Returns the last address used in tcp_connect.
        pub fn last_connect_addr(&self) -> Option<&str> {
            self.last_connect_addr.as_deref()
        }

        /// Returns the last port used in tcp_connect.
        pub fn last_connect_port(&self) -> Option<u16> {
            self.last_connect_port
        }
    }

    impl super::Network for MockNetwork {
        fn connect_wifi(
            &mut self,
            _ssid: &str,
            _password: &str,
        ) -> Result<(), super::NetworkError> {
            self.connected = true;
            Ok(())
        }

        fn is_connected(&self) -> bool {
            self.connected
        }

        fn tcp_connect(
            &mut self,
            addr: &str,
            port: u16,
        ) -> Result<Box<dyn super::TcpSocket>, super::NetworkError> {
            if !self.connected {
                return Err(super::NetworkError::ConnectionFailed);
            }
            self.last_connect_addr = Some(addr.to_string());
            self.last_connect_port = Some(port);
            let incoming = self.pending_responses.pop_front().unwrap_or_default();
            Ok(Box::new(MockTcpSocket {
                incoming,
                network_sent: Arc::clone(&self.sent_data),
            }))
        }
    }

    /// A mock TCP socket that tracks outgoing data in the parent MockNetwork.
    pub struct MockTcpSocket {
        incoming: Vec<u8>,
        network_sent: Arc<Mutex<Vec<u8>>>,
    }

    impl super::TcpSocket for MockTcpSocket {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize, super::NetworkError> {
            let n = self.incoming.len().min(buf.len());
            buf[..n].copy_from_slice(&self.incoming[..n]);
            self.incoming.drain(..n);
            Ok(n)
        }

        fn write(&mut self, data: &[u8]) -> Result<(), super::NetworkError> {
            self.network_sent.lock().unwrap().extend_from_slice(data);
            Ok(())
        }

        fn close(&mut self) -> Result<(), super::NetworkError> {
            Ok(())
        }
    }
}
