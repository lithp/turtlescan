use ethers_core::types::Block as EthBlock;
use ethers_core::types::{TxHash, U256 /*H256*/};
use std::fmt::Write;
use std::ops::Div;

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

pub fn format_block(block: &EthBlock<TxHash>) -> String {
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
    // this is also empty if we were given a block by subscribe_blocks(). In order to
    //   populate these fields we need to do another round trip to fetch the block
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
