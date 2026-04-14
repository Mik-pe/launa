/// Network abstraction for WiFi + TCP connectivity.
pub trait Network {
    fn connect_wifi(&mut self, ssid: &str, password: &str) -> Result<(), NetworkError>;
    fn is_connected(&self) -> bool;
    fn tcp_connect(&mut self, addr: &str, port: u16) -> Result<Box<dyn TcpSocket>, NetworkError>;
}

pub trait TcpSocket {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, NetworkError>;
    fn write(&mut self, data: &[u8]) -> Result<(), NetworkError>;
    fn close(&mut self) -> Result<(), NetworkError>;
}

#[derive(Debug)]
pub enum NetworkError {
    ConnectionFailed,
    Timeout,
    DnsFailed,
    Io,
}

#[cfg(feature = "std")]
pub mod mock {
    use std::collections::VecDeque;

    /// Mock network for desktop testing.
    pub struct MockNetwork {
        connected: bool,
        pending_responses: VecDeque<Vec<u8>>,
        sent_data: Vec<u8>,
    }

    impl MockNetwork {
        pub fn new() -> Self {
            MockNetwork {
                connected: false,
                pending_responses: VecDeque::new(),
                sent_data: Vec::new(),
            }
        }

        pub fn queue_response(&mut self, data: Vec<u8>) {
            self.pending_responses.push_back(data);
        }

        pub fn get_sent_data(&self) -> &[u8] {
            &self.sent_data
        }
    }

    impl super::Network for MockNetwork {
        fn connect_wifi(&mut self, _ssid: &str, _password: &str) -> Result<(), super::NetworkError> {
            self.connected = true;
            Ok(())
        }

        fn is_connected(&self) -> bool {
            self.connected
        }

        fn tcp_connect(&mut self, _addr: &str, _port: u16) -> Result<Box<dyn super::TcpSocket>, super::NetworkError> {
            if !self.connected {
                return Err(super::NetworkError::ConnectionFailed);
            }
            Ok(Box::new(MockTcpSocket {
                incoming: self.pending_responses.pop_front().unwrap_or_default(),
                outgoing: Vec::new(),
            }))
        }
    }

    pub struct MockTcpSocket {
        incoming: Vec<u8>,
        outgoing: Vec<u8>,
    }

    impl super::TcpSocket for MockTcpSocket {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize, super::NetworkError> {
            let n = self.incoming.len().min(buf.len());
            buf[..n].copy_from_slice(&self.incoming[..n]);
            self.incoming.drain(..n);
            Ok(n)
        }

        fn write(&mut self, data: &[u8]) -> Result<(), super::NetworkError> {
            self.outgoing.extend_from_slice(data);
            Ok(())
        }

        fn close(&mut self) -> Result<(), super::NetworkError> {
            Ok(())
        }
    }
}
