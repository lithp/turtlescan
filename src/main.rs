extern crate simple_error;

mod tail_blocks;
mod tui;
mod util;

use ansi_term::Style as AnsiStyle;
use clap::{App, Arg, SubCommand};
use ethers_providers::{Http, Middleware, Provider, Ws, JsonRpcClient};
use serde_json;
use std::convert::TryFrom;
use std::env;
use std::error::Error;

use log::debug;
use log4rs;

use simple_error::bail;


// https://mainnet.infura.io/v3/PROJECT_ID
// wss://mainnet.infura.io/ws/v3/PROJECT_ID


enum TypedProvider {
    Ws(Provider<Ws>),
    Http(Provider<Http>),
}


async fn new_provider(url: String) -> Result<TypedProvider, Box<dyn Error>> {
    if url.starts_with("http") {
        Ok(TypedProvider::Http(Provider::<Http>::try_from(url)?))
    } else if url.starts_with("ws") {
        Ok(TypedProvider::Ws(Provider::<Ws>::connect(url)
            .await?))
    } else {
        bail!("could not parse provider url")
    }
}


#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {

    // initialize logging, .ok() because rn logging is just for debugging anyway
    log4rs::init_file("log.yml", Default::default()).ok();

    debug!("started");

    let matches = App::new("turtlescan")
        .version("0.1")
        .author("Brian Cloutier <brian@ethereum.org>")
        .about("Explore from the safety of your shell")
        .arg(
            Arg::with_name("provider.url")
                .takes_value(true)
                .long("provider.url")
                .help("the address of the JSON-RPC server (e.g. https://mainnet.infura.io/v3/PROJECT_ID)"),
        )
        .subcommand(SubCommand::with_name("getBlock").about("emits the current block (json)"))
        .subcommand(SubCommand::with_name("tui").about("starts a tui (experimental)"))
        .subcommand(SubCommand::with_name("tailBlocks").about("emits blocks as they are received"))
        .get_matches();

    let provider_url: String = {
        if let Some(url) = matches.value_of("provider.url") {
            url.to_string()
        } else if let Ok(url) = env::var("TURTLE_PROVIDER") {
            url
        } else {
            eprintln!("You must provide a JSON-RPC server");
            eprintln!("Try setting TURTLE_PROVIDER, or passing --provider.url");
            bail!("Could not connect to server");
        }
    };

    match matches.subcommand_name() {
        Some("getBlock") => {
            let typed_provider = new_provider(provider_url).await?;

            //TODO: there must be a better way to do this
            match typed_provider {
                TypedProvider::Ws(provider) => get_block(provider).await,
                TypedProvider::Http(provider) => get_block(provider).await,
            }
        }
        Some("tailBlocks") => {
            let typed_provider = new_provider(provider_url).await?;

            println!(
                "{} - explore from the safety of your shell",
                AnsiStyle::new().bold().paint("turtlescan"),
            );

            if let TypedProvider::Ws(provider) = typed_provider {
                tail_blocks::tail_blocks(provider).await
            } else {
                bail!("tailBlocks requires connecting via websocket")
            }
        }
        Some("tui") => {
            let typed_provider = new_provider(provider_url).await?;

            match typed_provider {
                TypedProvider::Ws(provider) => tui::run_tui(provider),
                TypedProvider::Http(provider) => tui::run_tui(provider),
            }
        }
        _ => {
            println!("please select a subcommand");

            Ok(())
        }
    }
}

async fn get_block<T: JsonRpcClient>(provider: Provider<T>) -> Result<(), Box<dyn Error>> {
    // for now emits json, which can easily be formatted using jq
    // TODO(2021-08-27) make the output format configurable
    // TODO(2021-08-30) any kind of error handling

    let block = provider.get_block(1000000).await?;
    println!("{}", serde_json::to_string_pretty(&block).unwrap());

    Ok(())
}
