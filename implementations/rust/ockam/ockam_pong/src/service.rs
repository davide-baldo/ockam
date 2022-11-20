use crate::api::{PongServiceReply, PongServiceRequest};
use minicbor::{Decoder, Encoder};
use ockam::Context;
use ockam_core::{Encoded, Routed, Worker};

#[derive(Debug)]
pub struct PongService {}

#[ockam_core::worker]
impl Worker for PongService {
    type Message = Encoded;
    type Context = Context;

    async fn handle_message(
        &mut self,
        ctx: &mut Context,
        msg: Routed<Encoded>,
    ) -> ockam_core::Result<()> {
        let return_route = msg.return_route();
        let body = msg.body();

        let request: PongServiceRequest = Decoder::new(body.as_slice()).decode()?;
        let reply = if request.payload == "ping" {
            "pong"
        } else {
            "ping me!"
        };

        let mut encoder = Encoder::new(vec![]);
        encoder.encode(PongServiceReply::new(reply))?;
        let body = Encoded::from(encoder.into_writer());

        ctx.send(return_route, body).await?;

        Ok(())
    }
}
