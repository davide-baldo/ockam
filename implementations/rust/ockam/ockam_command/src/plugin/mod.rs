mod command;

use clap::ArgMatches;
use ockam_plugin::PluginAPI;
use std::fmt::Debug;
use std::sync::Arc;

#[derive(Debug)]
pub struct PluginCommand {
    plugin: Option<Arc<dyn PluginAPI>>,
    matches: ArgMatches,
}
