use crate::api::send_receive;
use anyhow::Context as _;
use clap::{arg, Parser};
use ockam_api::clean_multiaddr;
use ockam_command::util::OckamConfig;
use ockam_multiaddr::MultiAddr;
use serde::Serialize;

#[derive(Parser, Debug)]
#[command(name = "pong", about = "Ping/Pong sample service plugin")]
pub(crate) struct PongCommand {
    #[arg(short, long)]
    to: MultiAddr,
    #[arg(short, long)]
    payload: String,
    #[arg(short, long)]
    json: bool,
}

impl PongCommand {
    pub(crate) fn run(&self) {
        //TODO: HACK we should receive the config & the global options from the parent command
        let config = OckamConfig::load().unwrap();
        let (to, _) = clean_multiaddr(&self.to, &config.lookup())
            .context("Argument '--to' is invalid")
            .unwrap();

        let print_function = if self.json {
            print_result_json
        } else {
            print_result_plain
        };
        send_receive(config, to, &self.payload, print_function);
    }
}

#[derive(Serialize, Debug)]
struct Output {
    payload: String,
}

fn print_result_plain(payload: String) {
    println!("{}", serde_yaml::to_string(&Output { payload }).unwrap());
}

fn print_result_json(payload: String) {
    println!("{}", serde_json::to_string(&Output { payload }).unwrap());
}
