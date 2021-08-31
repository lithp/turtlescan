mod tail_blocks;
mod tui;

use ansi_term::Style as AnsiStyle;
use clap::{App, Arg, SubCommand};
use ethers_providers::{Http, Middleware, Provider, Ws};
use serde_json;
use std::convert::TryFrom;
use std::env;
use std::error::Error;

use log::debug;
use log4rs;

fn new_infura_provider(project_id: &str) -> Provider<Http> {
    let base_url = "https://mainnet.infura.io/v3/";
    let url = format!("{}{}", base_url, project_id);

    Provider::<Http>::try_from(url).expect("could not instantiate HTTP Provider")
}

async fn new_ws_infura_provider(project_id: &str) -> Provider<Ws> {
    let base_url = "wss://mainnet.infura.io/ws/v3/";
    let url = format!("{}{}", base_url, project_id);

    Provider::<Ws>::connect(url)
        .await
        .expect("could not instantiate Ws Provider")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    log4rs::init_file("log.yml", Default::default()).unwrap();
    debug!("started");

    let matches = App::new("turtlescan")
        .version("0.1")
        .author("Brian Cloutier <brian@ethereum.org>")
        .about("Explore from the safety of your shell")
        .arg(
            Arg::with_name("infura")
                .takes_value(true)
                .long("infura")
                .help("your infura project ID"),
        )
        .subcommand(SubCommand::with_name("getBlock").about("emits the current block (json)"))
        .subcommand(SubCommand::with_name("tui").about("starts a tui (experimental)"))
        .subcommand(SubCommand::with_name("tailBlocks").about("emits blocks as they are received"))
        .get_matches();

    let infura_project_id: String = {
        // TOOD(2021-08-30): I bet there's a way to use &str's.
        if let Some(project_id) = matches.value_of("infura") {
            project_id.to_string()
        } else if let Ok(project_id) = env::var("TURTLE_INFURA_ID") {
            project_id
        } else {
            panic!("You must provide an infura project id (try setting TURTLE_INFURA_ID");
        }
    };

    match matches.subcommand_name() {
        Some("getBlock") => {
            let provider = new_infura_provider(&infura_project_id);
            get_block(provider).await
        }
        Some("tailBlocks") => {
            let provider = new_ws_infura_provider(&infura_project_id).await;
            println!(
                "{} - explore from the safety of your shell",
                AnsiStyle::new().bold().paint("turtlescan"),
            );

            tail_blocks::tail_blocks(provider).await
        }
        Some("tui") => {
            let provider = new_infura_provider(&infura_project_id);
            tui::run_tui(provider)
        }
        _ => {
            println!("please select a subcommand");

            Ok(())
        }
    }
}

async fn get_block(provider: Provider<Http>) -> Result<(), Box<dyn Error>> {
    // for now emits json, which can easily be formatted using jq
    // TODO(2021-08-27) make the output format configurable
    // TODO(2021-08-30) any kind of error handling

    let block = provider.get_block(1000000).await?;
    println!("{}", serde_json::to_string_pretty(&block).unwrap());

    Ok(())
}
