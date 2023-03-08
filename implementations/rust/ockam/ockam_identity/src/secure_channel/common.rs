use ockam_core::compat::vec::Vec;
use ockam_core::{KeyExchanger, Message, NewKeyExchanger};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Message)]
pub(crate) struct AuthenticationConfirmation;

#[derive(Clone)]
pub(crate) enum Role {
    Initiator,
    Responder,
}

impl Role {
    pub fn is_initiator(&self) -> bool {
        match self {
            Role::Initiator => true,
            Role::Responder => false,
        }
    }

    pub fn str(&self) -> &'static str {
        match self {
            Role::Initiator => "initiator",
            Role::Responder => "responder",
        }
    }
}

/// KeyExchanger with extra constraints
pub trait SecureChannelKeyExchanger: KeyExchanger + Send + Sync + 'static {}

impl<D> SecureChannelKeyExchanger for D where D: KeyExchanger + Send + Sync + 'static {}

/// NewKeyExchanger with extra constraints
pub trait SecureChannelNewKeyExchanger: NewKeyExchanger + Send + Sync + 'static {}

impl<D> SecureChannelNewKeyExchanger for D where D: NewKeyExchanger + Send + Sync + 'static {}

/// SecureChannelListener message wrapper.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, Message)]
pub struct CreateResponderChannelMessage {
    payload: Vec<u8>,
    custom_payload: Option<Vec<u8>>,
}

impl CreateResponderChannelMessage {
    /// Channel information.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    /// Callback Address
    pub fn custom_payload(&self) -> &Option<Vec<u8>> {
        &self.custom_payload
    }
}

impl CreateResponderChannelMessage {
    /// Create message using payload and callback_address
    pub fn new(payload: Vec<u8>, custom_payload: Option<Vec<u8>>) -> Self {
        CreateResponderChannelMessage {
            payload,
            custom_payload,
        }
    }
}
