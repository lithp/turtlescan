use crate::tail_blocks;
use crate::tui;

use ansi_term::Style as AnsiStyle;
use clap::{App, Arg, SubCommand};
use ethers_core::abi::AbiParser;
use ethers_core::abi::FunctionExt;
use ethers_core::types::Bytes;
use ethers_core::types::NameOrAddress;
use ethers_core::types::TransactionRequest;
use ethers_core::types::H160;
use ethers_core::types::U256;
use ethers_providers::{Http, JsonRpcClient, Middleware, Provider, Ws};
use log::debug;
use log4rs;
use serde_json;
use simple_error::bail;
use std::convert::TryFrom;
use std::env;
use std::error::Error;
use std::path;
use std::str::FromStr;
use xdg;

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
        Ok(TypedProvider::Ws(Provider::<Ws>::connect(url).await?))
    } else {
        bail!("could not parse provider url")
    }
}

pub async fn main() -> Result<(), Box<dyn Error>> {
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
        .subcommand(SubCommand::with_name("egl").about("returns the current EGL gas target"))
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

    let xdg_dirs = xdg::BaseDirectories::with_prefix("turtlescan")?;
    let cache_path = path::Path::new("cache");
    let cache_path = xdg_dirs.place_cache_file(cache_path)?;
    debug!("using cache: {:?}", cache_path);

    match matches.subcommand_name() {
        Some("getBlock") => {
            let typed_provider = new_provider(provider_url).await?;

            //TODO: there must be a better way to do this
            match typed_provider {
                TypedProvider::Ws(provider) => get_block(provider).await,
                TypedProvider::Http(provider) => get_block(provider).await,
            }
        }
        Some("egl") => {
            let typed_provider = new_provider(provider_url).await?;

            match typed_provider {
                TypedProvider::Ws(provider) => egl(provider).await,
                TypedProvider::Http(provider) => egl(provider).await,
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
                TypedProvider::Ws(provider) => tui::run_tui(provider, cache_path),
                TypedProvider::Http(_) => bail!("tui requires connecting via websocket"),
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

async fn egl<T: JsonRpcClient>(provider: Provider<T>) -> Result<(), Box<dyn Error>> {
    // let egl_erc20 = "1e83916ea2ef2d7a6064775662e163b2d4c330a7";
    // let sig = "function totalSupply() returns (int256)";
    //
    // this is the proxy contract
    let egl_vote = "e9a09e0032d1ab5ce4bf09149ef746258252bd0b";
    let addr = H160::from_str(egl_vote)?;

    let sig = "function desiredEgl() view returns (int256)";
    let func = AbiParser::default().parse_function(sig)?;
    let data = ethers_contract::encode_function_data(&func, ())?;

    {
        let selector: [u8; 4] = func.selector();
        debug!(
            "calling function selector={} sig={:?}",
            hex::encode(selector),
            func
        );
    }

    let output = query_chain(&provider, addr, data).await?;
    let gas_target: U256 = ethers_contract::decode_function_data(&func, output, false)?;
    println!("gas_target: {:?}", gas_target);
    Ok(())
}

async fn query_chain<T: JsonRpcClient>(
    provider: &Provider<T>,
    address: H160,
    data: Bytes,
) -> Result<Bytes, Box<dyn Error>> {
    let tx = /*Eip1559*/TransactionRequest {
        to: Some(NameOrAddress::Address(address)),
        data: Some(data),
        // TODO: how do you pick a sensible gas price?
        gas_price: Some(U256::from_str_radix("290F915100", 16)?),
        ..Default::default()
    };

    let result = provider.trace_call(tx, vec![], None).await;
    let trace = result?;

    return Ok(trace.output);
}
