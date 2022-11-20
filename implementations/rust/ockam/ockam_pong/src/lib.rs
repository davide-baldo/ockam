mod api;
mod command;
mod service;

use clap::{ArgMatches, Command, CommandFactory, FromArgMatches as _};
use command::PongCommand;
use ockam::Context;
use ockam_core::{Encoded, Worker};
use ockam_plugin::PluginAPI;
use service::PongService;
use std::sync::Arc;

#[derive(Debug)]
struct PongPlugin {}

impl PluginAPI for PongPlugin {
    fn name(&self) -> String {
        "pong".to_string()
    }

    fn create_command(&self) -> Command {
        PongCommand::command()
    }

    fn create_worker(&self) -> Box<dyn Worker<Context = Context, Message = Encoded>> {
        Box::new(PongService {})
    }

    fn run(&self, matches: &ArgMatches) {
        let pong = PongCommand::from_arg_matches(matches).expect("cannot map to derive");
        pong.run();
    }
}

#[no_mangle]
pub unsafe fn create_plugin_api() -> Arc<dyn PluginAPI> {
    Arc::new(PongPlugin {})
}
