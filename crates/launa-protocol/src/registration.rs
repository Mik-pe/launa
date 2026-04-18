/// Client ID registration state machine.
///
/// The spa controller assigns client IDs to new devices on the RS-485 bus.
/// The flow is:
/// 1. Spa sends `FE BF 00` — "any new clients?"
/// 2. Client responds `FE BF 01 02 F1 73` — ID request
/// 3. Spa replies `FE BF 02 <ID>` — assigned ID
/// 4. Client acknowledges `<ID> BF 03`

/// Client ID registration state.
///
/// Tracks the current phase of the RS-485 bus registration handshake:
/// waiting for the spa to query → waiting for ID assignment → registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationState {
    /// Waiting for the spa to send the new-client query frame.
    WaitingForQuery,
    /// Client has requested an ID; waiting for the spa to assign one.
    WaitingForAssignment,
    /// Registration complete; the client has been assigned an ID.
    Registered { client_id: u8 },
}

/// Action to take during the registration handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationAction {
    /// Respond with an ID request frame.
    SendIdRequest,
    /// Acknowledge the assigned client ID.
    SendIdAck { client_id: u8 },
    /// No action needed.
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

    // --- Error path tests ---

    #[test]
    fn test_wrong_frame_type_in_waiting_for_query_stays_and_returns_none() {
        let mut sm = RegistrationStateMachine::new();
        let action = sm.process([0xAB, 0xCD], &[0x00]);
        assert_eq!(action, RegistrationAction::None);
        assert_eq!(sm.state(), &RegistrationState::WaitingForQuery);
    }

    #[test]
    fn test_empty_payload_in_waiting_for_query_returns_none() {
        let mut sm = RegistrationStateMachine::new();
        let action = sm.process([0xFE, 0xBF], &[]);
        assert_eq!(action, RegistrationAction::None);
        assert_eq!(sm.state(), &RegistrationState::WaitingForQuery);
    }

    #[test]
    fn test_assignment_missing_id_byte_in_waiting_for_assignment_returns_none() {
        let mut sm = RegistrationStateMachine::new();
        sm.process([0xFE, 0xBF], &[0x00]);
        assert_eq!(sm.state(), &RegistrationState::WaitingForAssignment);

        let action = sm.process([0xFE, 0xBF], &[0x02]);
        assert_eq!(action, RegistrationAction::None);
        assert_eq!(sm.state(), &RegistrationState::WaitingForAssignment);
    }

    #[test]
    fn test_assignment_frame_in_waiting_for_query_ignored() {
        let mut sm = RegistrationStateMachine::new();
        let action = sm.process([0xFE, 0xBF], &[0x02, 0x05]);
        assert_eq!(action, RegistrationAction::None);
        assert_eq!(sm.state(), &RegistrationState::WaitingForQuery);
    }

    #[test]
    fn test_query_frame_while_registered_ignored() {
        let mut sm = RegistrationStateMachine::new();
        sm.process([0xFE, 0xBF], &[0x00]);
        sm.process([0xFE, 0xBF], &[0x02, 0x03]);
        assert!(sm.is_registered());

        let action = sm.process([0xFE, 0xBF], &[0x00]);
        assert_eq!(action, RegistrationAction::None);
        assert!(sm.is_registered());
        assert_eq!(sm.client_id(), Some(0x03));
    }

    #[test]
    fn test_payload_02_alone_in_waiting_for_assignment_returns_none() {
        let mut sm = RegistrationStateMachine::new();
        sm.process([0xFE, 0xBF], &[0x00]);

        let action = sm.process([0xFE, 0xBF], &[0x02]);
        assert_eq!(action, RegistrationAction::None);
        assert_eq!(sm.state(), &RegistrationState::WaitingForAssignment);
    }

    #[test]
    fn test_assignment_with_id_zero_transitions_correctly() {
        let mut sm = RegistrationStateMachine::new();
        sm.process([0xFE, 0xBF], &[0x00]);

        let action = sm.process([0xFE, 0xBF], &[0x02, 0x00]);
        assert_eq!(action, RegistrationAction::SendIdAck { client_id: 0x00 });
        assert!(sm.is_registered());
        assert_eq!(sm.client_id(), Some(0x00));
    }

    #[test]
    fn test_wrong_frame_type_in_waiting_for_assignment_ignored() {
        let mut sm = RegistrationStateMachine::new();
        sm.process([0xFE, 0xBF], &[0x00]);

        let action = sm.process([0xAA, 0xBB], &[0x02, 0x05]);
        assert_eq!(action, RegistrationAction::None);
        assert_eq!(sm.state(), &RegistrationState::WaitingForAssignment);
    }

    #[test]
    fn test_registered_state_ignores_all_frames() {
        let mut sm = RegistrationStateMachine::new();
        sm.process([0xFE, 0xBF], &[0x00]);
        sm.process([0xFE, 0xBF], &[0x02, 0x07]);

        let action = sm.process([0xFE, 0xBF], &[0x02, 0x09]);
        assert_eq!(action, RegistrationAction::None);
        assert_eq!(sm.client_id(), Some(0x07));
    }
}
