use crate::plugin::PluginCommand;
use crate::CommandGlobalOpts;
use clap::error::ErrorKind;
use clap::{ArgMatches, Command, Error, FromArgMatches};
use ockam_plugin::loader::load_plugins;

/*
    This is the implementation of the plugin command,
    this command is created dynamically based on plugin presents.
*/
impl PluginCommand {
    pub fn run(self, _options: CommandGlobalOpts) {
        if let Some(plugin) = self.plugin {
            plugin.run(&self.matches);
        } else {
            //display help when no subcommand is provided
            create_plugin_command().print_help().unwrap();
            std::process::exit(1);
        }
    }
}

impl FromArgMatches for PluginCommand {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, Error> {
        if let Some((command_name, matches)) = matches.subcommand() {
            if let Some(plugin) = load_plugins()
                .lock()
                .unwrap()
                .iter()
                .find(|plugin| plugin.name() == command_name)
            {
                Ok(PluginCommand {
                    plugin: Some(plugin.clone()),
                    matches: matches.clone(),
                })
            } else {
                Err(Error::raw(
                    ErrorKind::InvalidSubcommand,
                    format!("no plugin found with name '{}'", command_name),
                ))
            }
        } else {
            Ok(PluginCommand {
                plugin: None,
                matches: matches.clone(),
            })
        }
    }

    fn update_from_arg_matches(&mut self, _: &ArgMatches) -> Result<(), Error> {
        panic!("updating args is not supported by plugin command")
    }
}

fn create_plugin_command() -> Command {
    Command::new("plugin")
        .about("Manage and call plugins")
        .subcommands(
            load_plugins()
                .lock()
                .unwrap()
                .iter()
                .map(|plugin| plugin.create_command())
                .collect::<Vec<Command>>(),
        )
        .subcommand_required(false)
}

impl clap::Args for PluginCommand {
    //fully replace original 'plugin' command

    fn augment_args(_: Command) -> Command {
        create_plugin_command()
    }

    fn augment_args_for_update(_: Command) -> Command {
        create_plugin_command()
    }
}
