use ethers_core::types::Block as EthBlock;
use ethers_core::types::{TxHash, U256 /*H256*/};
use std::fmt::Write;
use std::ops::Div;

pub fn format_block_hash(hash: &[u8]) -> String {
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

pub fn format_bytes_into_width(hash: &[u8], width: usize) -> String {
    assert!(width >= 8);  // e.g. "0x01..ff"
                          
    let mut formatted_bytes: String = String::new();
    for byte in hash.iter() {
        write!(formatted_bytes, "{:02x}", byte).expect("oops");
    }
    
    assert!(formatted_bytes.len() % 2 == 0); // each byte adds 2 chars
    
    if 2 + formatted_bytes.len() <= width {
        // easy, it already fits!
        return String::from("0x") + &formatted_bytes
    }
    
    // at this point we know `result.len() > width - 2`
    // and we know `width >= 8`
    // so `result.len() > 6, it looks something like "010203[...]"
    let trim_length = 2 + formatted_bytes.len() - width;
    
    // imagine a string just 2 chars longer than `width`. If we remove 2 chars and
    // then add ".." back in then we'll have made no progress! By trimming out 2 extra
    // chars we create space for our substitution
    let trim_length = trim_length + 2;
    
    // how much of the string is _not_ being trimmed 
    let remainder = formatted_bytes.len() - trim_length;

    let left_remainder = remainder.div_euclid(2); // explicitly round to 0
    let right_remainder = remainder - left_remainder;
    
    let mut result = String::from("0x");
    result.push_str(&formatted_bytes[..left_remainder]);
    result.push_str("..");
    result.push_str(&formatted_bytes[formatted_bytes.len()-right_remainder..]);

    result
}

pub fn humanize_u256(number: U256) -> String {
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

    let number = number.div(1000);
    if number < U256::from(1000) {
        return format!("{} Twei", number);
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

#[cfg(test)]
mod tests {
    use super::format_block_hash;
    use super::format_bytes_into_width;

    #[test]
    fn simple_block_hash() {
        let hash = [1u8, 2u8, 3u8, 4u8, 5u8, 6u8, 7u8, 8u8];
        assert_eq!(format_block_hash(&hash), "0x0102..0708");
    }
    
    #[test]
    fn format_into_width() {
        let hash = [1u8, 2u8, 3u8, 4u8, 5u8, 6u8, 7u8, 8u8, 9u8, 10u8, 11u8, 12u8];
        assert_eq!(format_bytes_into_width(&hash, 8), "0x01..0c");
        assert_eq!(format_bytes_into_width(&hash, 9), "0x01..b0c");
        assert_eq!(format_bytes_into_width(&hash, 10), "0x010..b0c");
        assert_eq!(format_bytes_into_width(&hash, 11), "0x010..0b0c");
        assert_eq!(format_bytes_into_width(&hash, 12), "0x0102..0b0c");
        assert_eq!(format_bytes_into_width(&hash, 15), "0x01020..0a0b0c");
        assert_eq!(format_bytes_into_width(&hash, 20), "0x01020304..090a0b0c");
        assert_eq!(format_bytes_into_width(&hash, 24), "0x0102030405..08090a0b0c");
        assert_eq!(format_bytes_into_width(&hash, 25), "0x0102030405..708090a0b0c");
        assert_eq!(format_bytes_into_width(&hash, 26), "0x0102030405060708090a0b0c");
    }
}