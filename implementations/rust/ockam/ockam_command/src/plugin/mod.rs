mod command;
mod loader;

use crate::CommandGlobalOpts;
use clap::{ArgMatches, Command};
use std::fmt::Debug;
use std::sync::Arc;

#[derive(Debug)]
pub struct PluginCommand {
    plugin: Option<Arc<dyn PluginAPI>>,
    matches: ArgMatches,
}

/**
    This is the API needed to be implemented by plugin in order to add a command
*/
pub trait PluginAPI: Send + Sync + Debug {
    fn name(&self) -> String;
    fn create_command(&self) -> Command;
    fn run(&self, matches: &ArgMatches, options: CommandGlobalOpts);
}
