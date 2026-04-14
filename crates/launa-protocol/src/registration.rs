/// Client ID registration state machine.
///
/// The spa controller assigns client IDs to new devices on the RS-485 bus.
/// The flow is:
/// 1. Spa sends `FE BF 00` — "any new clients?"
/// 2. Client responds `FE BF 01 02 F1 73` — ID request
/// 3. Spa replies `FE BF 02 <ID>` — assigned ID
/// 4. Client acknowledges `<ID> BF 03`

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationState {
    WaitingForQuery,
    WaitingForAssignment,
    Registered { client_id: u8 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationAction {
    SendIdRequest,
    SendIdAck { client_id: u8 },
    None,
}

pub struct RegistrationStateMachine {
    state: RegistrationState,
}

impl RegistrationStateMachine {
    pub fn new() -> Self {
        RegistrationStateMachine {
            state: RegistrationState::WaitingForQuery,
        }
    }

    pub fn state(&self) -> &RegistrationState {
        &self.state
    }

    pub fn is_registered(&self) -> bool {
        matches!(self.state, RegistrationState::Registered { .. })
    }

    pub fn client_id(&self) -> Option<u8> {
        match self.state {
            RegistrationState::Registered { client_id } => Some(client_id),
            _ => None,
        }
    }

    /// Process an incoming frame relevant to registration.
    /// Returns the action the caller should take.
    pub fn process(&mut self, frame_type: [u8; 2], payload: &[u8]) -> RegistrationAction {
        match self.state {
            RegistrationState::WaitingForQuery => {
                // Looking for FE BF 00
                if frame_type == [0xFE, 0xBF] && payload.first() == Some(&0x00) {
                    self.state = RegistrationState::WaitingForAssignment;
                    return RegistrationAction::SendIdRequest;
                }
                RegistrationAction::None
            }
            RegistrationState::WaitingForAssignment => {
                // Looking for FE BF 02 <ID>
                if frame_type == [0xFE, 0xBF] && payload.len() >= 1 && payload[0] == 0x02 {
                    if let Some(&id) = payload.get(1) {
                        self.state = RegistrationState::Registered { client_id: id };
                        return RegistrationAction::SendIdAck { client_id: id };
                    }
                }
                RegistrationAction::None
            }
            RegistrationState::Registered { .. } => RegistrationAction::None,
        }
    }

    /// Reset state (e.g. after a bus error or timeout).
    pub fn reset(&mut self) {
        self.state = RegistrationState::WaitingForQuery;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_registration_flow() {
        let mut sm = RegistrationStateMachine::new();
        assert_eq!(sm.state(), &RegistrationState::WaitingForQuery);

        // Step 1: spa queries for new clients
        let action = sm.process([0xFE, 0xBF], &[0x00]);
        assert_eq!(action, RegistrationAction::SendIdRequest);
        assert_eq!(sm.state(), &RegistrationState::WaitingForAssignment);

        // Step 2: spa assigns ID 0x02
        let action = sm.process([0xFE, 0xBF], &[0x02, 0x02]);
        assert_eq!(action, RegistrationAction::SendIdAck { client_id: 0x02 });
        assert!(sm.is_registered());
        assert_eq!(sm.client_id(), Some(0x02));
    }

    #[test]
    fn test_reset() {
        let mut sm = RegistrationStateMachine::new();
        sm.process([0xFE, 0xBF], &[0x00]);
        sm.process([0xFE, 0xBF], &[0x02, 0x05]);
        assert!(sm.is_registered());

        sm.reset();
        assert_eq!(sm.state(), &RegistrationState::WaitingForQuery);
        assert!(!sm.is_registered());
    }
}
