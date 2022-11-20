use minicbor::{Decode, Decoder, Encode, Encoder};
use ockam::Context;
use ockam_api::nodes::service::message::SendMessage;
use ockam_command::util::{
    delete_embedded_node, node_rpc, start_embedded_node, OckamConfig, RpcBuilder,
};
use ockam_command::{CommandGlobalOpts, GlobalArgs};
use ockam_core::api::Request;
use ockam_core::{CowStr, Encoded};
use ockam_multiaddr::MultiAddr;

#[derive(Debug, Clone, Decode, Encode)]
#[rustfmt::skip]
#[cbor(map)]
pub struct PongServiceRequest<'a> {
    #[cfg(feature = "tag")]
    #[n(0)] tag: TypeTag<4536656>,
    #[b(1)] pub payload: CowStr<'a>,
}

impl PongServiceRequest<'_> {
    pub fn new(payload: &str) -> Self {
        PongServiceRequest {
            payload: CowStr::from(payload.to_string()),
        }
    }
}

#[derive(Debug, Clone, Decode, Encode)]
#[rustfmt::skip]
#[cbor(map)]
pub struct PongServiceReply<'a> {
    #[cfg(feature = "tag")]
    #[n(0)] tag: TypeTag<4533656>,
    #[b(1)] pub payload: CowStr<'a>,
}

impl PongServiceReply<'_> {
    pub fn new(payload: &str) -> Self {
        PongServiceReply {
            payload: CowStr::from(payload.to_string()),
        }
    }
}

pub fn send_receive(config: OckamConfig, to: MultiAddr, payload: &str, f: fn(String)) {
    let options = CommandGlobalOpts::new(GlobalArgs::default(), config.clone());
    node_rpc(send_receive_impl, (options, to, payload.to_string(), f))
}

pub async fn send_receive_impl(
    ctx: Context,
    info: (CommandGlobalOpts, MultiAddr, String, fn(String)),
) -> ockam_command::error::Result<()> {
    let (options, to, payload, f) = info;

    let mut encoder = Encoder::new(vec![]);
    encoder.encode(PongServiceRequest::new(&payload))?;
    let body: Vec<u8> = encoder.into_writer();

    let api_node = start_embedded_node(&ctx, &options.config).await?;

    let mut rpc = RpcBuilder::new(&ctx, &options, &api_node)
        .tcp(None.as_ref())?
        .build();
    rpc.request(Request::post("v0/message").body(SendMessage::new(&to, body)))
        .await?;

    let reply = rpc.parse_response::<Encoded>()?;
    let reply: PongServiceReply = Decoder::new(reply.as_slice()).decode()?;

    f(reply.payload.to_string());

    delete_embedded_node(&options.config, &api_node).await;

    Ok(())
}
