extern crate term_size;

use ansi_term::Style as AnsiStyle;
use ethers_core::types::Block as EthBlock;
use ethers_core::types::{TxHash, U256 /*H256*/};
use ethers_providers::StreamExt;
use ethers_providers::{Middleware, Provider, Ws};
use std::error::Error;
use std::fmt::Write;
use std::ops::Div;

pub async fn tail_blocks(provider: Provider<Ws>) -> Result<(), Box<dyn Error>> {
    //TODO(2021-08-29) this needs much better error handling

    let block_number = provider.get_block_number().await?;
    let current_block = provider.get_block(block_number).await?;
    let current_block = current_block.expect("something weird happened");

    // for easier development:
    // println!("{}", serde_json::to_string_pretty(&current_block).unwrap());

    let print_header = || {
        println!(
            "{} {} {} {} {} {} {} {} {}",
            AnsiStyle::new().underline().paint(" blk num"),
            AnsiStyle::new().underline().paint("  blk hash  "),
            AnsiStyle::new().underline().paint(" parent hash"),
            AnsiStyle::new().underline().paint("  coinbase  "),
            AnsiStyle::new().underline().paint(" gas used"),
            AnsiStyle::new().underline().paint("gas limit"),
            AnsiStyle::new().underline().paint("base fee"),
            // AnsiStyle::new().underline().paint("timestamp"),
            AnsiStyle::new().underline().paint("txns"),
            AnsiStyle::new().underline().paint("size"),
        );
    };
    print_header();

    println!("{}", format_block(current_block));

    let mut lines_since_last_header_print = 1;

    let mut stream = provider.subscribe_blocks().await?;
    while let Some(block) = stream.next().await {
        // blocks pushed to us do not include transactions or sizes!
        // in order to get the size of the txn count we need to refetch the block

        // technically, a reorg *could* cause this next line to fail, if that block number
        // no longer exists on the canonical chain. This error ought to be handled.
        let complete_block = provider.get_block(block.number.unwrap()).await?.unwrap();
        println!("{}", format_block(complete_block));
        lines_since_last_header_print += 1;

        // check the term size on every loop b/c it's cheap and seamlessly handles resizes
        if let Some((_, term_height)) = term_size::dimensions() {
            if lines_since_last_header_print >= term_height - 1 {
                lines_since_last_header_print = 0;
                print_header();
            }
        }
    }

    Ok(())
}

fn format_block_hash(hash: &[u8]) -> String {
    //TODO(2021-08-29) surely there's a better way...
    //TODO(2021-08-29) input validation and nice error messages

    let mut result = String::from("0x");

    for byte in hash[..2].iter() {
        write!(result, "{:02x}", byte).expect("oops");
    }

    result.push_str("..");

    let len = hash.len();
    for byte in hash[len - 2..].iter() {
        write!(result, "{:02x}", byte).expect("oops");
    }

    result
}

fn humanize_u256(number: U256) -> String {
    //TODO: lol this code

    if number < U256::from(1000) {
        return number.to_string();
    }

    let number = number.div(1000);
    if number < U256::from(1000) {
        return format!("{} Kwei", number);
    }

    let number = number.div(1000);
    if number < U256::from(1000) {
        return format!("{} Mwei", number);
    }

    let number = number.div(1000);
    if number < U256::from(1000) {
        return format!("{} Gwei", number);
    }

    panic!("number too large to format");
}

fn format_block(block: EthBlock<TxHash>) -> String {
    let mut result = String::new();

    match block.number {
        Some(number) => {
            let formatted = number.to_string();
            result.push_str(&formatted);
        }
        None => {
            result.push_str("unknown");
        }
    };

    result.push_str(" ");

    match block.hash {
        Some(hash) => {
            result.push_str(&format_block_hash(hash.as_bytes()));
        }
        None => {
            result.push_str("unknown");
        }
    };

    result.push_str(" ");
    result.push_str(&format_block_hash(block.parent_hash.as_bytes()));

    result.push_str(" ");
    result.push_str(&format_block_hash(block.author.as_bytes()));

    write!(result, " {:>9}", &block.gas_used.to_string()).expect("oops");

    write!(result, " {:>9}", &block.gas_limit.to_string()).expect("oops");

    //TODO(2021-08-30) not all blocks are london blocks
    let base_fee = block.base_fee_per_gas.unwrap();
    let base_fee_fmt = humanize_u256(base_fee);
    write!(result, " {:>8}", &base_fee_fmt).expect("oops");

    // TODO: also format block.timestamp?

    write!(result, " {:>4}", &block.transactions.len()).expect("oops");

    result.push_str(" ");

    // believe it or not, this is None for blocks from eth_subscribe
    match block.size {
        Some(size) => {
            result.push_str(&size.to_string());
        }
        None => {
            result.push_str("none?");
        }
    }

    result
}
