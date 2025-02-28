use bytes::{Bytes, BytesMut};
use core::sync::atomic::AtomicBool;
use core::sync::atomic::Ordering;
use ockam_core::compat::sync::Arc;
use ockam_core::flow_control::{FlowControlId, FlowControlOutgoingAccessControl, FlowControls};
use ockam_core::{
    errcode::{Kind, Origin},
    route, Address, AllowSourceAddress, AnyIncomingAccessControl, Encodable, Error,
    IncomingAccessControl, LocalInfo, LocalMessage, NeutralMessage, Route, Routed, Worker,
};
use ockam_node::{Context, WorkerBuilder};
use ockam_transport_tcp::{PortalMessage, MAX_PAYLOAD_SIZE};

use crate::kafka::inlet_controller::KafkaInletController;
use crate::kafka::length_delimited::{length_encode, KafkaMessageDecoder};
use crate::kafka::protocol_aware::{InletInterceptorImpl, KafkaMessageInterceptor, TopicUuidMap};
use crate::kafka::secure_channel_map::KafkaSecureChannelController;
use crate::kafka::KAFKA_OUTLET_BOOTSTRAP_ADDRESS;

/// By default, kafka supports up to 1MB messages. 16MB is the maximum suggested
pub(crate) const MAX_KAFKA_MESSAGE_SIZE: u32 = 16 * 1024 * 1024;

enum Receiving {
    Requests,
    Responses,
}

/// Acts like a relay for messages between tcp inlet and outlet for both directions.
/// It's meant to be created by the portal listener.
///
/// This implementation manages both streams inlet and outlet in two different workers, one dedicated
/// to the requests (inlet=>outlet) the other for the responses (outlet=>inlet).
/// Since every kafka message is length-delimited, every message is read and written
/// through a framed encoder/decoder.
///
/// ```text
/// ┌────────┐  decoder    ┌─────────┐  encoder    ┌────────┐
/// │        ├────────────►│ Kafka   ├────────────►│        │
/// │        │             │ Request │             │        │
/// │  TCP   │             └─────────┘             │  TCP   │
/// │ Inlet  │             ┌─────────┐             │ Outlet │
/// │        │  encoder    │ Kafka   │   decoder   │        │
/// │        │◄────────────┤ Response│◄────────────┤        │
/// └────────┘             └─────────┘             └────────┘
///```
pub(crate) struct KafkaPortalWorker {
    // The instance of worker managing the opposite: request or response
    // The first one to receive the disconnect message will stop both workers
    other_worker_address: Address,
    receiving: Receiving,
    message_interceptor: Arc<dyn KafkaMessageInterceptor>,
    disconnect_received: Arc<AtomicBool>,
    decoder: KafkaMessageDecoder,
    max_message_size: u32,
    // Since we know the next step beforehand we simply ignore the provided onward route
    // and use the one we know.
    fixed_onward_route: Option<Route>,
}

#[ockam::worker]
impl Worker for KafkaPortalWorker {
    type Message = NeutralMessage;
    type Context = Context;

    // Every tcp payload message is received gets written into a buffer
    // when the whole kafka message is received the message is intercepted
    // and then forwarded to the original destination.
    // As it may take several tcp payload messages to complete a single kafka
    // message or a single message may contain several kafka messages within
    // there is no guaranteed relation between message incoming and messages
    // outgoing.
    async fn handle_message(
        &mut self,
        context: &mut Self::Context,
        routed_message: Routed<Self::Message>,
    ) -> ockam::Result<()> {
        let onward_route = routed_message.onward_route();
        let return_route = routed_message.return_route();
        let local_info = routed_message.local_message().local_info();
        let portal_message = PortalMessage::decode(routed_message.payload())?;

        match portal_message {
            PortalMessage::Payload(message, _) => {
                let result = self
                    .intercept_and_transform_messages(context, message)
                    .await;

                match result {
                    Ok(maybe_kafka_message) => {
                        if let Some(encoded_message) = maybe_kafka_message {
                            self.split_and_send(
                                context,
                                onward_route,
                                return_route,
                                encoded_message,
                                local_info.as_slice(),
                            )
                            .await?;
                        }
                    }
                    Err(cause) => {
                        trace!("error: {cause:?}");
                        return match cause {
                            InterceptError::Io(cause) => {
                                Err(Error::new(Origin::Transport, Kind::Io, cause))
                            }
                            InterceptError::Ockam(error) => Err(error),
                        };
                    }
                }
            }
            PortalMessage::Disconnect => {
                self.forward(context, routed_message).await?;

                // The first one to receive disconnect and to swap the atomic will stop both workers
                let disconnect_received = self.disconnect_received.swap(true, Ordering::SeqCst);
                if !disconnect_received {
                    trace!(
                        "{:?} received disconnect event from {:?}",
                        context.address(),
                        return_route
                    );
                    context
                        .stop_worker(self.other_worker_address.clone())
                        .await?;
                    context.stop_worker(context.address()).await?;
                }
            }
            PortalMessage::Ping => self.forward(context, routed_message).await?,

            PortalMessage::Pong => {
                match self.receiving {
                    Receiving::Requests => {
                        // if we receive a pong message it means it must be from the other worker
                        if routed_message.src_addr() == self.other_worker_address {
                            if let Some(fixed_onward_route) = self.fixed_onward_route.as_ref() {
                                debug!(
                                    "updating onward route from {} to {}",
                                    fixed_onward_route,
                                    routed_message.return_route()
                                );
                                self.fixed_onward_route = Some(routed_message.return_route());
                            }
                        }
                    }
                    Receiving::Responses => {
                        // only the response worker should receive pongs but we forward
                        // the pong also to the other worker to update the fixed onward route
                        // with the final route
                        let mut local_message = routed_message.local_message().clone();
                        local_message = local_message
                            .set_onward_route(route![self.other_worker_address.clone()]);
                        context.forward(local_message).await?;

                        self.forward(context, routed_message).await?
                    }
                }
            }
        }

        Ok(())
    }
}

// internal error to return both io and ockam errors
#[derive(Debug)]
pub(crate) enum InterceptError {
    Io(ockam_core::compat::io::Error),
    Ockam(ockam_core::Error),
}

impl KafkaPortalWorker {
    async fn forward(
        &self,
        context: &mut Context,
        routed_message: Routed<NeutralMessage>,
    ) -> ockam_core::Result<()> {
        let mut local_message = routed_message.into_local_message();
        trace!(
            "before: onwards={:?}; return={:?};",
            local_message.onward_route_ref(),
            local_message.return_route_ref()
        );

        local_message = if let Some(fixed_onward_route) = &self.fixed_onward_route {
            trace!(
                "replacing onward_route {:?} with {:?}",
                local_message.onward_route_ref(),
                fixed_onward_route
            );
            local_message
                .set_onward_route(fixed_onward_route.clone())
                .push_front_return_route(&self.other_worker_address)
        } else {
            local_message = local_message.pop_front_onward_route()?;
            // Since we force the return route next step (fixed_onward_route in the other worker),
            // we can omit the previous return route.
            trace!(
                "replacing return_route {:?} with {:?}",
                local_message.return_route_ref(),
                self.other_worker_address
            );
            local_message.set_return_route(route![self.other_worker_address.clone()])
        };

        trace!(
            "after: onwards={:?}; return={:?};",
            local_message.onward_route_ref(),
            local_message.return_route_ref(),
        );
        context.forward(local_message).await
    }

    async fn split_and_send(
        &self,
        context: &mut Context,
        provided_onward_route: Route,
        provided_return_route: Route,
        buffer: Bytes,
        local_info: &[LocalInfo],
    ) -> ockam_core::Result<()> {
        let return_route: Route;
        let onward_route;

        if let Some(fixed_onward_route) = &self.fixed_onward_route {
            // To correctly proxy messages to the inlet or outlet side
            // we invert the return route when a message pass through
            return_route = provided_return_route
                .clone()
                .modify()
                .prepend(self.other_worker_address.clone())
                .into();
            onward_route = fixed_onward_route.clone();
        } else {
            // Since we force the return route next step (fixed_onward_route in the other worker),
            // we can omit the previous return route.
            return_route = route![self.other_worker_address.clone()];
            onward_route = provided_onward_route.clone().modify().pop_front().into();
        };

        for chunk in buffer.chunks(MAX_PAYLOAD_SIZE) {
            let message = LocalMessage::new()
                .with_onward_route(onward_route.clone())
                .with_return_route(return_route.clone())
                .with_payload(PortalMessage::Payload(chunk, None).encode()?)
                .with_local_info(local_info.to_vec());

            context.forward(message).await?;
        }
        Ok(())
    }

    /// Takes in buffer and returns a buffer made of one or more complete kafka message
    async fn intercept_and_transform_messages(
        &mut self,
        context: &mut Context,
        encoded_message: &[u8],
    ) -> Result<Option<Bytes>, InterceptError> {
        let mut encoded_buffer: Option<BytesMut> = None;

        for complete_kafka_message in self
            .decoder
            .extract_complete_messages(BytesMut::from(encoded_message), self.max_message_size)
            .map_err(InterceptError::Ockam)?
        {
            let transformed_message = match self.receiving {
                Receiving::Requests => {
                    self.message_interceptor
                        .intercept_request(context, complete_kafka_message)
                        .await
                }
                Receiving::Responses => {
                    self.message_interceptor
                        .intercept_response(context, complete_kafka_message)
                        .await
                }
            }?;

            // avoid copying the first message
            if let Some(encoded_buffer) = encoded_buffer.as_mut() {
                encoded_buffer.extend_from_slice(
                    length_encode(transformed_message)
                        .map_err(InterceptError::Ockam)?
                        .as_ref(),
                );
            } else {
                encoded_buffer =
                    Some(length_encode(transformed_message).map_err(InterceptError::Ockam)?);
            }
        }

        Ok(encoded_buffer.map(|buffer| buffer.freeze()))
    }
}

impl KafkaPortalWorker {
    /// Creates the two specular kafka workers for the outlet use case.
    /// Returns the address of the worker which handles the Requests
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn create_outlet_side_kafka_portal(
        context: &mut Context,
        max_kafka_message_size: Option<u32>,
        fixed_outlet_route: Route,
        message_interceptor: Arc<dyn KafkaMessageInterceptor>,
        flow_controls: &FlowControls,
        secure_channel_flow_control_id: Option<FlowControlId>,
        spawner_flow_control_id: Option<FlowControlId>,
        incoming_access_control: Arc<dyn IncomingAccessControl>,
    ) -> ockam_core::Result<Address> {
        let requests_worker_address = Address::random_tagged("KafkaPortalWorker.requests");
        let responses_worker_address = Address::random_tagged("KafkaPortalWorker.responses");
        let disconnect_received = Arc::new(AtomicBool::new(false));

        let request_worker = Self {
            message_interceptor: message_interceptor.clone(),
            other_worker_address: responses_worker_address.clone(),
            receiving: Receiving::Requests,
            disconnect_received: disconnect_received.clone(),
            decoder: KafkaMessageDecoder::new(),
            max_message_size: max_kafka_message_size.unwrap_or(MAX_KAFKA_MESSAGE_SIZE),
            fixed_onward_route: Some(fixed_outlet_route),
        };
        let response_worker = Self {
            message_interceptor,
            other_worker_address: requests_worker_address.clone(),
            receiving: Receiving::Responses,
            disconnect_received: disconnect_received.clone(),
            decoder: KafkaMessageDecoder::new(),
            max_message_size: max_kafka_message_size.unwrap_or(MAX_KAFKA_MESSAGE_SIZE),
            fixed_onward_route: None,
        };

        let flow_control_id = FlowControls::generate_flow_control_id();

        // add the default outlet as consumer for the interceptor
        flow_controls.add_consumer(KAFKA_OUTLET_BOOTSTRAP_ADDRESS, &flow_control_id);

        flow_controls.add_producer(
            requests_worker_address.clone(),
            &flow_control_id,
            spawner_flow_control_id.as_ref(),
            vec![],
        );

        // we need to receive the first message from the listener
        if let Some(spawner_flow_control_id) = spawner_flow_control_id.as_ref() {
            flow_controls.add_consumer(requests_worker_address.clone(), spawner_flow_control_id);
        }

        if let Some(secure_channel_flow_control_id) = secure_channel_flow_control_id.as_ref() {
            flow_controls.add_consumer(
                requests_worker_address.clone(),
                secure_channel_flow_control_id,
            );
        }

        // allow the other worker to forward the `pong` message
        WorkerBuilder::new(request_worker)
            .with_address(requests_worker_address.clone())
            .with_incoming_access_control_arc(Arc::new(AnyIncomingAccessControl::new(vec![
                Arc::new(AllowSourceAddress(responses_worker_address.clone())),
                incoming_access_control,
            ])))
            .with_outgoing_access_control_arc(Arc::new(FlowControlOutgoingAccessControl::new(
                flow_controls,
                flow_control_id.clone(),
                spawner_flow_control_id.clone(),
            )))
            .start(context)
            .await?;

        WorkerBuilder::new(response_worker)
            .with_address(responses_worker_address)
            .start(context)
            .await?;

        Ok(requests_worker_address)
    }

    /// Returns address used for inlet communications, aka the one facing the client side,
    /// used for requests.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn create_inlet_side_kafka_portal(
        context: &mut Context,
        secure_channel_controller: Arc<dyn KafkaSecureChannelController>,
        uuid_to_name: TopicUuidMap,
        inlet_map: KafkaInletController,
        max_kafka_message_size: Option<u32>,
        flow_control_id: Option<FlowControlId>,
        inlet_responder_route: Route,
    ) -> ockam_core::Result<Address> {
        let shared_protocol_state = Arc::new(InletInterceptorImpl::new(
            secure_channel_controller,
            uuid_to_name,
            inlet_map,
        ));

        let requests_worker_address = Address::random_tagged("KafkaPortalWorker.requests");
        let responses_worker_address = Address::random_tagged("KafkaPortalWorker.responses");
        let disconnect_received = Arc::new(AtomicBool::new(false));

        let request_worker = Self {
            message_interceptor: shared_protocol_state.clone(),
            other_worker_address: responses_worker_address.clone(),
            receiving: Receiving::Requests,
            disconnect_received: disconnect_received.clone(),
            decoder: KafkaMessageDecoder::new(),
            max_message_size: max_kafka_message_size.unwrap_or(MAX_KAFKA_MESSAGE_SIZE),
            fixed_onward_route: None,
        };
        let response_worker = Self {
            message_interceptor: shared_protocol_state,
            other_worker_address: requests_worker_address.clone(),
            receiving: Receiving::Responses,
            disconnect_received: disconnect_received.clone(),
            decoder: KafkaMessageDecoder::new(),
            max_message_size: max_kafka_message_size.unwrap_or(MAX_KAFKA_MESSAGE_SIZE),
            fixed_onward_route: Some(inlet_responder_route),
        };

        context
            .start_worker(requests_worker_address.clone(), request_worker)
            .await?;

        if let Some(flow_control_id) = flow_control_id {
            let flow_controls = context.flow_controls();
            flow_controls.add_consumer(responses_worker_address.clone(), &flow_control_id);
            flow_controls.add_consumer(KAFKA_OUTLET_BOOTSTRAP_ADDRESS, &flow_control_id);
        }
        context
            .start_worker(responses_worker_address, response_worker)
            .await?;

        Ok(requests_worker_address)
    }
}

#[cfg(test)]
mod test {
    use bytes::{Buf, BufMut, Bytes, BytesMut};
    use kafka_protocol::messages::metadata_request::MetadataRequestBuilder;
    use kafka_protocol::messages::metadata_response::MetadataResponseBroker;
    use kafka_protocol::messages::{
        ApiKey, BrokerId, MetadataRequest, MetadataResponse, RequestHeader, ResponseHeader,
    };
    use kafka_protocol::protocol::Builder;
    use kafka_protocol::protocol::Decodable;
    use kafka_protocol::protocol::Encodable as KafkaEncodable;
    use kafka_protocol::protocol::StrBytes;
    use ockam::identity::{secure_channels, Identifier};
    use ockam_core::compat::sync::{Arc, Mutex};
    use ockam_core::{route, Address, NeutralMessage, Routed, Worker};
    use ockam_multiaddr::MultiAddr;
    use ockam_node::Context;
    use ockam_transport_tcp::{PortalMessage, MAX_PAYLOAD_SIZE};
    use std::collections::BTreeMap;
    use std::str::FromStr;
    use std::time::Duration;

    use crate::kafka::inlet_controller::KafkaInletController;
    use crate::kafka::portal_worker::KafkaPortalWorker;
    use crate::kafka::secure_channel_map::KafkaSecureChannelControllerImpl;
    use crate::kafka::ConsumerNodeAddr;
    use crate::port_range::PortRange;
    use ockam::MessageReceiveOptions;

    const TEST_MAX_KAFKA_MESSAGE_SIZE: u32 = 128 * 1024;
    const TEST_KAFKA_API_VERSION: i16 = 13;

    // a simple worker that keep receiving buffer
    #[derive(Clone)]
    struct TcpPayloadReceiver {
        buffer: Arc<Mutex<Vec<u8>>>,
    }

    #[ockam_core::worker]
    impl Worker for TcpPayloadReceiver {
        type Message = NeutralMessage;
        type Context = Context;

        async fn handle_message(
            &mut self,
            _context: &mut Self::Context,
            message: Routed<Self::Message>,
        ) -> ockam_core::Result<()> {
            let message = PortalMessage::decode(message.payload())?;
            if let PortalMessage::Payload(payload, _) = message {
                self.buffer.lock().unwrap().extend_from_slice(payload);
            }
            Ok(())
        }
    }

    #[allow(non_snake_case)]
    #[ockam_macros::test(timeout = 5_000)]
    async fn kafka_portal_worker__ping_pong_pass_through__should_pass(
        context: &mut Context,
    ) -> ockam::Result<()> {
        let portal_inlet_address = setup_only_worker(context).await;

        context
            .send(
                route![portal_inlet_address, context.address()],
                PortalMessage::Ping.to_neutral_message()?,
            )
            .await?;

        let message = context.receive::<NeutralMessage>().await?;
        let return_route = message.return_route();
        let message = PortalMessage::decode(message.payload())?;
        if let PortalMessage::Ping = message {
        } else {
            panic!("invalid message type")
        }

        context
            .send(return_route, PortalMessage::Pong.to_neutral_message()?)
            .await?;

        let payload = context.receive::<NeutralMessage>().await?.into_payload();
        let message = PortalMessage::decode(&payload)?;
        if let PortalMessage::Pong = message {
        } else {
            panic!("invalid message type")
        }

        Ok(())
    }

    #[allow(non_snake_case)]
    #[ockam_macros::test(timeout = 5_000)]
    async fn kafka_portal_worker__pieces_of_kafka_message__message_assembled(
        context: &mut Context,
    ) -> ockam::Result<()> {
        let portal_inlet_address = setup_only_worker(context).await;

        let mut request_buffer = BytesMut::new();
        encode(
            &mut request_buffer,
            create_request_header(ApiKey::MetadataKey),
            MetadataRequest::default(),
        );

        let first_piece_of_payload = &request_buffer[0..request_buffer.len() - 1];
        let second_piece_of_payload = &request_buffer[request_buffer.len() - 1..];

        // send 2 distinct pieces and see if the kafka message is re-assembled back
        context
            .send(
                route![portal_inlet_address.clone(), context.address()],
                PortalMessage::Payload(first_piece_of_payload, None).to_neutral_message()?,
            )
            .await?;
        context
            .send(
                route![portal_inlet_address, context.address()],
                PortalMessage::Payload(second_piece_of_payload, None).to_neutral_message()?,
            )
            .await?;

        let payload = context.receive::<NeutralMessage>().await?.into_payload();
        let message = PortalMessage::decode(&payload)?;
        if let PortalMessage::Payload(payload, _) = message {
            assert_eq!(payload, request_buffer.as_ref());
        } else {
            panic!("invalid message")
        }
        Ok(())
    }

    #[allow(non_snake_case)]
    #[ockam_macros::test(timeout = 5_000)]
    async fn kafka_portal_worker__double_kafka_message__message_assembled(
        context: &mut Context,
    ) -> ockam::Result<()> {
        let portal_inlet_address = setup_only_worker(context).await;

        let mut request_buffer = BytesMut::new();
        encode(
            &mut request_buffer,
            create_request_header(ApiKey::MetadataKey),
            MetadataRequest::default(),
        );
        encode(
            &mut request_buffer,
            create_request_header(ApiKey::MetadataKey),
            MetadataRequest::default(),
        );

        let double_payload = request_buffer.as_ref();
        context
            .send(
                route![portal_inlet_address.clone(), context.address()],
                PortalMessage::Payload(double_payload, None).to_neutral_message()?,
            )
            .await?;
        let payload = context.receive::<NeutralMessage>().await?.into_payload();
        let message = PortalMessage::decode(&payload)?;
        if let PortalMessage::Payload(payload, _) = message {
            assert_eq!(payload, double_payload);
        } else {
            panic!("invalid message")
        }
        Ok(())
    }

    #[allow(non_snake_case)]
    #[ockam_macros::test(timeout = 5_000)]
    async fn kafka_portal_worker__bigger_than_limit_kafka_message__error(
        context: &mut Context,
    ) -> ockam::Result<()> {
        let portal_inlet_address = setup_only_worker(context).await;

        // with the message container it goes well over the max allowed message kafka size
        let mut zero_buffer: Vec<u8> = Vec::new();
        for _n in 0..TEST_MAX_KAFKA_MESSAGE_SIZE + 1 {
            zero_buffer.push(0);
        }

        // you don't want to create a produce request since it would trigger
        // a lot of side effects and we just want to validate the transport
        let mut insanely_huge_tag: BTreeMap<i32, Bytes> = BTreeMap::new();
        insanely_huge_tag.insert(0, zero_buffer.into());

        let mut request_buffer = BytesMut::new();
        encode(
            &mut request_buffer,
            create_request_header(ApiKey::MetadataKey),
            MetadataRequestBuilder::default()
                .topics(Default::default())
                .include_cluster_authorized_operations(Default::default())
                .include_topic_authorized_operations(Default::default())
                .allow_auto_topic_creation(Default::default())
                .unknown_tagged_fields(insanely_huge_tag)
                .build()
                .unwrap(),
        );

        let huge_payload = request_buffer.as_ref();
        for chunk in huge_payload.chunks(MAX_PAYLOAD_SIZE) {
            let _error = context
                .send(
                    route![portal_inlet_address.clone(), context.address()],
                    PortalMessage::Payload(chunk, None).to_neutral_message()?,
                )
                .await;
        }

        let message = context
            .receive_extended::<NeutralMessage>(
                MessageReceiveOptions::new().with_timeout(Duration::from_millis(200)),
            )
            .await;

        assert!(message.is_err(), "expected timeout!");
        Ok(())
    }

    #[allow(non_snake_case)]
    #[ockam_macros::test(timeout = 5_000)]
    async fn kafka_portal_worker__almost_over_limit_than_limit_kafka_message__two_kafka_message_pass(
        context: &mut Context,
    ) -> ockam::Result<()> {
        let portal_inlet_address = setup_only_worker(context).await;

        // let's build the message to 90% of max. size
        let mut zero_buffer: Vec<u8> = Vec::new();
        for _n in 0..(TEST_MAX_KAFKA_MESSAGE_SIZE as f64 * 0.9) as usize {
            zero_buffer.push(0);
        }

        // you don't want to create a produce request since it would trigger
        // a lot of side effects, and we just want to validate the transport
        let mut insanely_huge_tag: BTreeMap<i32, Bytes> = BTreeMap::new();
        insanely_huge_tag.insert(0, zero_buffer.into());

        let mut huge_outgoing_request = BytesMut::new();
        encode(
            &mut huge_outgoing_request,
            create_request_header(ApiKey::MetadataKey),
            MetadataRequestBuilder::default()
                .topics(Default::default())
                .include_cluster_authorized_operations(Default::default())
                .include_topic_authorized_operations(Default::default())
                .allow_auto_topic_creation(Default::default())
                .unknown_tagged_fields(insanely_huge_tag.clone())
                .build()
                .unwrap(),
        );

        let receiver = TcpPayloadReceiver {
            buffer: Default::default(),
        };

        context
            .start_worker(
                Address::from_string("tcp_payload_receiver"),
                receiver.clone(),
            )
            .await?;

        // let's duplicate the message
        huge_outgoing_request.extend(huge_outgoing_request.clone());

        for chunk in huge_outgoing_request.as_ref().chunks(MAX_PAYLOAD_SIZE) {
            context
                .send(
                    route![portal_inlet_address.clone(), "tcp_payload_receiver"],
                    PortalMessage::Payload(chunk, None).to_neutral_message()?,
                )
                .await?;
        }

        // make sure every packet was received
        loop {
            if receiver.buffer.lock().unwrap().len() >= huge_outgoing_request.len() {
                break;
            }
            ockam_node::compat::tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let incoming_rebuilt_buffer = receiver.buffer.lock().unwrap().to_vec();

        assert_eq!(incoming_rebuilt_buffer.len(), huge_outgoing_request.len());
        assert_eq!(
            incoming_rebuilt_buffer.as_slice(),
            huge_outgoing_request.as_ref()
        );

        Ok(())
    }

    async fn setup_only_worker(context: &mut Context) -> Address {
        let inlet_map = KafkaInletController::new(
            MultiAddr::default(),
            route![],
            route![],
            [255, 255, 255, 255].into(),
            PortRange::new(0, 0).unwrap(),
            None,
        );

        // Random Identifier, doesn't affect the test
        let authority_identifier = Identifier::from_str(
            "I0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap();

        let secure_channels = secure_channels().await.unwrap();
        let secure_channel_controller = KafkaSecureChannelControllerImpl::new(
            secure_channels,
            ConsumerNodeAddr::Relay(MultiAddr::default()),
            authority_identifier,
        )
        .into_trait();

        KafkaPortalWorker::create_inlet_side_kafka_portal(
            context,
            secure_channel_controller,
            Default::default(),
            inlet_map,
            Some(TEST_MAX_KAFKA_MESSAGE_SIZE),
            None,
            route![context.address()],
        )
        .await
        .unwrap()
    }

    fn encode<H, R>(mut request_buffer: &mut BytesMut, header: H, request: R)
    where
        H: KafkaEncodable,
        R: KafkaEncodable,
    {
        let size = header.compute_size(TEST_KAFKA_API_VERSION).unwrap()
            + request.compute_size(TEST_KAFKA_API_VERSION).unwrap();
        request_buffer.put_u32(size as u32);

        header
            .encode(&mut request_buffer, TEST_KAFKA_API_VERSION)
            .unwrap();
        request
            .encode(&mut request_buffer, TEST_KAFKA_API_VERSION)
            .unwrap();
    }

    fn create_request_header(api_key: ApiKey) -> RequestHeader {
        RequestHeader::builder()
            .request_api_key(api_key as i16)
            .request_api_version(TEST_KAFKA_API_VERSION)
            .correlation_id(1)
            .client_id(Some(StrBytes::from_static_str("my-client-id")))
            .unknown_tagged_fields(Default::default())
            .build()
            .unwrap()
    }

    #[allow(non_snake_case)]
    #[ockam_macros::test(timeout = 5000)]
    async fn kafka_portal_worker__metadata_exchange__response_changed(
        context: &mut Context,
    ) -> ockam::Result<()> {
        let handle = crate::test_utils::start_manager_for_tests(context, None, None).await?;
        let project_authority = handle
            .node_manager
            .node_manager
            .project_authority()
            .unwrap();

        let secure_channel_controller = KafkaSecureChannelControllerImpl::new(
            handle.secure_channels.clone(),
            ConsumerNodeAddr::Relay(MultiAddr::default()),
            project_authority,
        )
        .into_trait();

        let inlet_map = KafkaInletController::new(
            MultiAddr::default(),
            route![],
            route![],
            [127, 0, 0, 1].into(),
            PortRange::new(0, 0).unwrap(),
            None,
        );
        let portal_inlet_address = KafkaPortalWorker::create_inlet_side_kafka_portal(
            context,
            secure_channel_controller,
            Default::default(),
            inlet_map.clone(),
            None,
            None,
            route![context.address()],
        )
        .await?;

        let mut request_buffer = BytesMut::new();
        // let's create a real kafka request and pass it through the portal
        encode(
            &mut request_buffer,
            create_request_header(ApiKey::MetadataKey),
            MetadataRequest::default(),
        );

        context
            .send(
                route![portal_inlet_address, context.address()],
                PortalMessage::Payload(&request_buffer, None).to_neutral_message()?,
            )
            .await?;

        let message = context
            .receive_extended::<NeutralMessage>(MessageReceiveOptions::new().without_timeout())
            .await?;
        let return_route = message.return_route();
        let message = PortalMessage::decode(message.payload())?;

        if let PortalMessage::Payload(payload, _) = message {
            assert_eq!(&request_buffer, payload);
        } else {
            panic!("invalid message type")
        }
        trace!("return_route: {:?}", &return_route);

        let mut response_buffer = BytesMut::new();
        {
            let response_header = ResponseHeader::builder()
                .correlation_id(1)
                .unknown_tagged_fields(Default::default())
                .build()
                .unwrap();

            let metadata_response = MetadataResponse::builder()
                .throttle_time_ms(Default::default())
                .cluster_id(Default::default())
                .cluster_authorized_operations(-2147483648)
                .unknown_tagged_fields(Default::default())
                .controller_id(BrokerId::from(1))
                .topics(Default::default())
                .brokers(indexmap::IndexMap::from_iter(vec![(
                    BrokerId(1),
                    MetadataResponseBroker::builder()
                        .host(StrBytes::from_static_str("bad.remote.host.example.com"))
                        .port(1234)
                        .rack(Default::default())
                        .unknown_tagged_fields(Default::default())
                        .build()
                        .unwrap(),
                )]))
                .build()
                .unwrap();

            let size = response_header
                .compute_size(TEST_KAFKA_API_VERSION)
                .unwrap()
                + metadata_response
                    .compute_size(TEST_KAFKA_API_VERSION)
                    .unwrap();

            response_buffer.put_u32(size as u32);
            response_header
                .encode(&mut response_buffer, TEST_KAFKA_API_VERSION)
                .unwrap();
            metadata_response
                .encode(&mut response_buffer, TEST_KAFKA_API_VERSION)
                .unwrap();
            assert_eq!(size + 4, response_buffer.len());
        }

        context
            .send(
                return_route,
                PortalMessage::Payload(&response_buffer, None).to_neutral_message()?,
            )
            .await?;

        let message = context
            .receive_extended::<NeutralMessage>(MessageReceiveOptions::new().without_timeout())
            .await?;
        let message = PortalMessage::decode(message.payload())?;

        if let PortalMessage::Payload(payload, _) = message {
            assert_ne!(&response_buffer.to_vec(), &payload);
            let mut buffer_received = BytesMut::from(payload);
            let _size = buffer_received.get_u32();
            let header =
                ResponseHeader::decode(&mut buffer_received, TEST_KAFKA_API_VERSION).unwrap();
            assert_eq!(1, header.correlation_id);
            let response =
                MetadataResponse::decode(&mut buffer_received, TEST_KAFKA_API_VERSION).unwrap();
            assert_eq!(1, response.brokers.len());
            let broker = response.brokers.get(&BrokerId::from(1)).unwrap();
            assert_eq!("127.0.0.1", &broker.host.to_string());
            assert_eq!(0, broker.port);

            let address = inlet_map.retrieve_inlet(1).await.expect("inlet not found");
            assert_eq!("127.0.0.1".to_string(), address.ip().to_string());
            assert_eq!(0, address.port());
        } else {
            panic!("invalid message type")
        }
        Ok(())
    }
}
