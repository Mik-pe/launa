//! Simulated RS-485 transport.
//!
//! A bidirectional byte pipe connecting the `SpaSim` (acting as the real spa)
//! to `SpaApp` from `launa-core` (the real firmware logic). Bytes written by the
//! spa appear as readable data for the controller, and vice versa.

use alloc::vec::Vec;

use launa_hal::transport::{Transport, TransportError};
use std::collections::VecDeque;

/// Virtual RS-485 bus connecting spa simulator to controller.
///
/// Two independent byte queues:
/// - `spa_to_controller`: bytes the spa writes (status frames, registration queries)
/// - `controller_to_spa`: bytes the controller writes (commands, registration responses)
///
/// The `Transport` trait implementation reads from `spa_to_controller` and writes
/// into `controller_to_spa`, matching how the real UART works from the firmware's
/// perspective.
pub struct SimTransport {
    spa_to_controller: VecDeque<u8>,
    controller_to_spa: VecDeque<u8>,
}

impl SimTransport {
    pub fn new() -> Self {
        SimTransport {
            spa_to_controller: VecDeque::new(),
            controller_to_spa: VecDeque::new(),
        }
    }

    /// Inject bytes as if the spa sent them (into the controller's read buffer).
    pub fn inject_from_spa(&mut self, data: &[u8]) {
        self.spa_to_controller.extend(data.iter().copied());
    }

    /// Take all bytes the controller has written (to be processed by the spa).
    pub fn take_from_controller(&mut self) -> Vec<u8> {
        self.controller_to_spa.drain(..).collect()
    }

    /// Check if there are bytes available for the controller to read.
    pub fn has_incoming(&self) -> bool {
        !self.spa_to_controller.is_empty()
    }

    /// Check if the controller has written any bytes the spa hasn't consumed.
    pub fn has_outgoing(&self) -> bool {
        !self.controller_to_spa.is_empty()
    }
}

impl Transport for SimTransport {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
        let n = self.spa_to_controller.len().min(buf.len());
        for byte in buf.iter_mut().take(n) {
            *byte = self.spa_to_controller.pop_front().unwrap();
        }
        Ok(n)
    }

    async fn write(&mut self, data: &[u8]) -> Result<(), TransportError> {
        self.controller_to_spa.extend(data.iter().copied());
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), TransportError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use launa_hal::Transport;

    /// Helper: poll an async future to completion synchronously.
    /// SimTransport's async methods always complete immediately,
    /// so this is safe for test use.
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

        fn dummy_raw_waker() -> RawWaker {
            fn no_op(_: *const ()) {}
            fn clone(_: *const ()) -> RawWaker {
                dummy_raw_waker()
            }
            static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, no_op, no_op, no_op);
            RawWaker::new(std::ptr::null(), &VTABLE)
        }

        let waker = unsafe { Waker::from_raw(dummy_raw_waker()) };
        let mut cx = Context::from_waker(&waker);

        let mut future = std::pin::pin!(future);
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(val) => val,
            Poll::Pending => panic!("SimTransport async method should not return Pending"),
        }
    }

    #[test]
    fn test_bidirectional_flow() {
        let mut transport = SimTransport::new();

        // Spa sends data
        transport.inject_from_spa(&[0x7E, 0x01, 0x7E]);

        // Controller reads it
        let mut buf = [0u8; 16];
        let n = block_on(transport.read(&mut buf)).unwrap();
        assert_eq!(n, 3);
        assert_eq!(&buf[..3], &[0x7E, 0x01, 0x7E]);

        // Controller writes a command
        block_on(transport.write(&[0x7E, 0x02, 0x7E])).unwrap();

        // Spa reads controller's output
        let outgoing = transport.take_from_controller();
        assert_eq!(outgoing, vec![0x7E, 0x02, 0x7E]);
    }

    #[test]
    fn test_empty_read() {
        let mut transport = SimTransport::new();
        let mut buf = [0u8; 16];
        let n = block_on(transport.read(&mut buf)).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn test_partial_read() {
        let mut transport = SimTransport::new();
        transport.inject_from_spa(&[1, 2, 3, 4, 5]);

        let mut buf = [0u8; 3];
        let n = block_on(transport.read(&mut buf)).unwrap();
        assert_eq!(n, 3);
        assert_eq!(&buf, &[1, 2, 3]);

        let n = block_on(transport.read(&mut buf)).unwrap();
        assert_eq!(n, 2);
        assert_eq!(&buf[..2], &[4, 5]);
    }
}
