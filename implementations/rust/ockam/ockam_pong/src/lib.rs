use clap::{arg, ArgMatches, Command, CommandFactory, FromArgMatches as _, Parser};
use ockam_command::{CommandGlobalOpts, PluginAPI};
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "pong", about = "Ping/pong sample service plugin")]
struct PongCommand {
    #[arg(short, long)]
    parameter: String,
}

#[derive(Debug)]
struct PongPlugin {}

impl PluginAPI for PongPlugin {
    fn name(&self) -> String {
        "pong".to_string()
    }

    fn create_command(&self) -> Command {
        PongCommand::command()
    }

    fn run(&self, matches: &ArgMatches, options: CommandGlobalOpts) {
        let pong = PongCommand::from_arg_matches(matches).expect("cannot map to derive");

        println!("PongPlugin called with parameter: {}", &pong.parameter);
        println!(
            "PongPlugin: matches {:?}, global_args: {:?}",
            matches, options.global_args
        );
    }
}

#[no_mangle]
pub unsafe fn create_plugin_api() -> Arc<dyn PluginAPI> {
    Arc::new(PongPlugin {})
}
