use ockam_core::Encoded;
// #[cfg(feature = "tag")]
// use ockam_core::TypeTag;

use ockam::{Context, Result};
use ockam_core::{Routed, Worker};

//adapt template based API to trait based one
pub(crate) struct PluginServiceAdapter {
    pub(crate) worker: Box<dyn Worker<Context = Context, Message = Encoded>>,
}

#[ockam::worker]
impl Worker for PluginServiceAdapter {
    type Message = Encoded;
    type Context = Context;

    async fn handle_message(&mut self, ctx: &mut Context, msg: Routed<Encoded>) -> Result<()> {
        self.worker.handle_message(ctx, msg).await
    }
}
