//! MQTT client wrapper with Home Assistant discovery support.
//!
//! Generates HA auto-discovery payloads and maps spa state to MQTT topics.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

pub mod command_parser;
pub mod discovery;
pub mod escape;
pub mod mqtt_codec;
pub mod ota_url;
pub mod packet;
pub mod remote_log;
pub mod state;
pub mod state_change;
pub mod topics;

pub use discovery::{DiscoveryBuilder, DiscoveryMessage};
pub use topics::{
    birth_config, lwt_config, BirthConfig, LwtConfig, TopicBuilder, AVAILABILITY_OFFLINE,
    AVAILABILITY_ONLINE,
};

pub use command_parser::{
    parse_command, parse_command_ok, parse_set_temperature_validated, ParseResult,
};
pub use mqtt_codec::{
    append_lp_string, encode_connect, encode_disconnect, encode_pingreq, encode_pingresp,
    encode_puback, encode_publish, encode_remaining_length, encode_subscribe, parse_connack,
    parse_suback, ConnackError, ConnectConfig, SubackError,
};
pub use ota_url::parse_ota_url;
pub use remote_log::{log_entry_to_json, RemoteLogEntry};
pub use state::status_to_json;
