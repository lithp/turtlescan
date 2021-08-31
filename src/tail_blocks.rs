use crate::util;

extern crate term_size;

use ansi_term::Style as AnsiStyle;
use ethers_providers::StreamExt;
use ethers_providers::{Middleware, Provider, Ws};
use std::error::Error;

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

    println!("{}", util::format_block(&current_block));

    let mut lines_since_last_header_print = 1;

    let mut stream = provider.subscribe_blocks().await?;
    while let Some(block) = stream.next().await {
        // blocks pushed to us do not include transactions or sizes!
        // in order to get the size of the txn count we need to refetch the block

        // technically, a reorg *could* cause this next line to fail, if that block number
        // no longer exists on the canonical chain. This error ought to be handled.
        let complete_block = provider.get_block(block.number.unwrap()).await?.unwrap();
        println!("{}", util::format_block(&complete_block));
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
