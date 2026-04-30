//! Shared integration test harness for Launa spa controller tests.
//!
//! Provides the common `TestHarness` struct that wires together:
//! `SpaSim → FrameDecoder → SpaApp → SimBroker`
//!
//! Each test file composes this harness and adds its domain-specific helpers.

use launa_core::{AppAction, SpaApp};
use launa_protocol::command::Command;
use launa_protocol::frame::FrameDecoder;
use launa_sim::{SimBroker, SpaSim, VirtualClock};
use std::boxed::Box;

/// Shared integration test harness wiring SpaSim → FrameDecoder → SpaApp → SimBroker.
///
/// The harness provides a clean pipeline:
/// - `tick_spa()`: SpaSim generates bytes → FrameDecoder decodes frames → SpaApp processes them
/// - `tick_app()`: SpaApp periodic tick for time-based checks (stale detection, etc.)
/// - `send_command()`: Queue MQTT command into SpaApp
/// - `complete_registration()`: Drive registration to completion
/// - `advance_ms()`: Advance VirtualClock by N milliseconds
/// - `process_outgoing()`: Feed outgoing frames from SpaApp back into SpaSim
/// - `execute_actions_on_broker()`: Execute publish actions against the SimBroker
pub struct TestHarness {
    pub sim: SpaSim,
    pub app: SpaApp<'static>,
    pub broker: SimBroker,
    pub clock: &'static VirtualClock,
    pub decoder: FrameDecoder,
}

impl Default for TestHarness {
    fn default() -> Self {
        Self::new()
    }
}

impl TestHarness {
    /// Create a new harness with clean state: unregistered, no status, no publications.
    pub fn new() -> Self {
        let clock: &'static VirtualClock = Box::leak(Box::new(VirtualClock::new()));
        let sim = SpaSim::new();
        let app = SpaApp::new(clock);
        let broker = SimBroker::new("test_spa");

        TestHarness {
            sim,
            app,
            broker,
            clock,
            decoder: FrameDecoder::new(),
        }
    }

    /// Run one SpaSim tick: generate spa bytes, decode frames, process through SpaApp.
    /// Returns all AppActions produced.
    pub fn tick_spa(&mut self) -> Vec<AppAction> {
        let spa_bytes = self.sim.tick();
        let frames = self.decoder.feed_slice(&spa_bytes);
        let mut all_actions = Vec::new();
        for frame in &frames {
            let actions = self.app.process_frame(frame);
            all_actions.extend(actions);
        }
        all_actions
    }

    /// Run SpaApp periodic tick (time-based checks: stale, diagnostics, etc.).
    pub fn tick_app(&mut self) -> Vec<AppAction> {
        self.app.tick()
    }

    /// Advance virtual clock by N milliseconds (without generating spa data).
    pub fn advance_ms(&mut self, ms: u64) {
        self.clock.advance_ms(ms);
    }

    /// Send an MQTT command into the SpaApp command queue.
    pub fn send_command(&mut self, cmd: Command) -> Vec<AppAction> {
        self.app.on_mqtt_command(cmd)
    }

    /// Drive registration to completion by ticking the spa until registered.
    /// Returns the number of ticks needed.
    /// Panics if registration doesn't complete within `max_ticks`.
    pub fn complete_registration(&mut self, max_ticks: usize) -> usize {
        for i in 0..max_ticks {
            let actions = self.tick_spa();
            self.process_outgoing(&actions);

            if self.app.is_registered() {
                return i + 1;
            }

            for action in &actions {
                if let AppAction::SendFrame(bytes) = action {
                    let responses = self.sim.process_incoming_bytes(bytes);
                    if !responses.is_empty() {
                        let resp_frames = self.decoder.feed_slice(&responses);
                        for frame in &resp_frames {
                            let resp_actions = self.app.process_frame(frame);
                            for ra in &resp_actions {
                                if let AppAction::SendFrame(rbytes) = ra {
                                    self.sim.process_incoming_bytes(rbytes);
                                }
                            }
                        }
                    }
                }
            }

            if self.app.is_registered() {
                return i + 1;
            }
        }
        panic!("Registration did not complete within {} ticks", max_ticks);
    }

    /// Process outgoing SendFrame actions from SpaApp through the SpaSim.
    /// This sends controller frames back to the simulator (e.g., registration responses,
    /// commands) so the simulator can update its state.
    pub fn process_outgoing(&mut self, actions: &[AppAction]) {
        for action in actions {
            if let AppAction::SendFrame(bytes) = action {
                let _responses = self.sim.process_incoming_bytes(bytes);
            }
        }
    }

    /// Execute all AppActions against the broker using raw publish (respects disconnect/loss).
    pub fn execute_actions_on_broker(&mut self, actions: &[AppAction]) {
        for action in actions {
            match action {
                AppAction::PublishState { status, .. } => {
                    let json = launa_mqtt::state::status_to_json(
                        status,
                        None,
                        None,
                        false,
                        None,
                        "registered",
                    );
                    let topic = launa_mqtt::topics::TopicBuilder::new("test_spa").state_topic();
                    self.broker.publish(&topic, &json);
                }
                AppAction::PublishAvailability { online } => {
                    let payload = if *online { "online" } else { "offline" };
                    let topic =
                        launa_mqtt::topics::TopicBuilder::new("test_spa").availability_topic();
                    self.broker.publish(&topic, payload);
                }
                AppAction::PublishStaleAvailability => {
                    let topic =
                        launa_mqtt::topics::TopicBuilder::new("test_spa").availability_topic();
                    self.broker.publish(&topic, "offline");
                }
                AppAction::PublishAlert { level, message } => {
                    self.broker
                        .publish(&format!("launa/test_spa/alert/{}", level), message);
                }
                AppAction::PublishDiagnostics {
                    uptime_secs,
                    frames_received,
                    unregistered_frames: _,
                    command_retries,
                    command_drops,
                    registration_state,
                    frame_errors,
                    uart_bytes,
                } => {
                    let payload = format!(
                        "{{\"uptime\":{},\"frames\":{},\"retries\":{},\"drops\":{},\"reg\":\"{}\",\"frame_err\":{},\"uart_bytes\":{}}}",
                        uptime_secs, frames_received, command_retries, command_drops, registration_state, frame_errors, uart_bytes
                    );
                    self.broker.publish("launa/test_spa/diagnostics", &payload);
                }
                _ => {}
            }
        }
    }

    /// Execute publish actions against the broker using convenience methods (bypasses disconnect/loss).
    /// Use this when you don't need to simulate broker disconnect or message loss.
    pub fn execute_actions_on_broker_simple(&mut self, actions: &[AppAction]) {
        for action in actions {
            match action {
                AppAction::PublishState { status, .. } => {
                    self.broker.publish_state(status);
                }
                AppAction::PublishAvailability { online } => {
                    self.broker.publish_availability(*online);
                }
                AppAction::PublishStaleAvailability => {
                    self.broker.publish_availability(false);
                }
                _ => {}
            }
        }
    }

    /// Collect and execute all actions from a single spa tick cycle.
    /// Returns all actions produced.
    pub fn collect_actions(&mut self) -> Vec<AppAction> {
        let actions = self.tick_spa();
        self.process_outgoing(&actions);
        self.execute_actions_on_broker(&actions);
        actions
    }

    /// Run a full tick cycle: spa tick + app tick, process outgoing, execute on broker.
    /// Returns all actions from both tick sources.
    pub fn full_tick(&mut self) -> Vec<AppAction> {
        let mut all_actions = self.tick_spa();
        self.process_outgoing(&all_actions);
        all_actions.extend(self.tick_app());
        self.execute_actions_on_broker(&all_actions);
        all_actions
    }

    /// Tick spa and process outgoing (no app tick, no broker).
    pub fn tick_spa_with_outgoing(&mut self) -> Vec<AppAction> {
        let actions = self.tick_spa();
        self.process_outgoing(&actions);
        actions
    }

    /// Get the frame error count from the internal decoder.
    pub fn frame_error_count(&self) -> u32 {
        self.decoder.frame_error_count()
    }

    /// Helper: count how many actions of a specific type are in the list.
    pub fn count_action_type(actions: &[AppAction], matcher: impl Fn(&AppAction) -> bool) -> usize {
        actions.iter().filter(|a| matcher(a)).count()
    }

    /// Helper: check if any SendFrame action contains an encoded toggle for the given item.
    pub fn has_toggle_for(
        actions: &[AppAction],
        item: launa_protocol::command::ToggleItem,
    ) -> bool {
        let (mt, payload) = Command::ToggleItem(item).encode();
        let expected = launa_protocol::frame::FrameEncoder::encode(mt, &payload).unwrap();
        actions.iter().any(|a| {
            if let AppAction::SendFrame(bytes) = a {
                bytes == &expected
            } else {
                false
            }
        })
    }
}
