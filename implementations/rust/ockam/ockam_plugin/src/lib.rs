pub mod loader;

use clap::{ArgMatches, Command};
use ockam::Context;
use ockam_core::{Encoded, Worker};
use std::fmt::Debug;

/**
This is the API needed to be implemented by plugin in order to add a command
 */
pub trait PluginAPI: Send + Sync + Debug {
    fn name(&self) -> String;
    fn create_command(&self) -> Command;
    fn create_worker(&self) -> Box<dyn Worker<Context = Context, Message = Encoded>>;
    fn run(&self, matches: &ArgMatches);
}
