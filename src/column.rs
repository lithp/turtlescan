use chrono::{DateTime, NaiveDateTime, Utc};
use ethers_core::types::{Block as EthBlock, Transaction, TransactionReceipt, TxHash, U64};
use tui::{text::{Spans, Span}, style::{Style, Modifier}};
use std::convert::TryInto;

use crate::util;

pub struct Column<T> {
    pub name: &'static str,
    pub width: usize,
    pub enabled: bool,
    pub render: Box<dyn Fn(&T) -> String>,
}

pub fn default_receipt_columns() -> Vec<Column<TransactionReceipt>> {
    vec![
        Column {
            name: "status",
            width: 7,
            render: Box::new(|receipt| match receipt.status {
                None => "unknown".to_string(),

                //TODO(2021-09-15) the docs for this are backwards, contribute a fix!
                Some(status) => {
                    if status == U64::from(0) {
                        "failure".to_string()
                    } else if status == U64::from(1) {
                        "success".to_string()
                    } else {
                        panic!("unexpected txn status: {}", status)
                    }
                }
            }),
            enabled: false,
        },
        Column {
            // "gas used is None if the client is running in light client mode"
            name: "gas used",
            width: 9,
            render: Box::new(|receipt| match receipt.gas_used {
                None => "???".to_string(),
                Some(gas) => gas.to_string(),
            }),
            enabled: true,
        },
        Column {
            name: "total gas",
            width: 9,
            render: Box::new(|receipt| receipt.cumulative_gas_used.to_string()),
            enabled: true,
        },
        Column {
            // TODO in the details pane only render this column if it's Some
            name: "deployed to",
            width: 12,
            render: Box::new(|receipt| {
                receipt
                    .contract_address
                    .map(|addr| addr.to_string())
                    .unwrap_or("not deployed".to_string())
            }),
            enabled: false,
        },
        Column {
            // TODO(2021-09-15): actually render the logs, this reqires writing some parsers
            name: "log count",
            width: 9,
            render: Box::new(|receipt| receipt.logs.len().to_string()),
            enabled: false,
        }, // root: Option<H256>
           // logs_bloom: Bloom
           // effective_gas_price: Option<U256>
           //   base fee + priority fee
    ]
}

pub fn default_txn_columns() -> Vec<Column<Transaction>> {
    vec![
        Column {
            name: "idx",
            width: 3,
            render: Box::new(|txn| match txn.transaction_index {
                None => "???".to_string(),
                Some(i) => i.to_string(),
            }),
            enabled: false,
        },
        Column {
            name: "hash",
            width: 12,
            render: Box::new(|txn| util::format_block_hash(txn.hash.as_bytes())),
            enabled: true,
        },
        Column {
            name: "from",
            width: 12,
            render: Box::new(|txn| util::format_block_hash(txn.from.as_bytes())),
            enabled: true,
        },
        Column {
            name: "to",
            width: 12,
            render: Box::new(|txn| match txn.to {
                None => "".to_string(), // e.g. contract creation
                Some(addr) => util::format_block_hash(addr.as_bytes()),
            }),
            enabled: true,
        },
        // also, there are more important cols if we fetch out the receipts
        // more cols:
        //   nonce: U256
        //   value: U256
        //   gas_price: Option<U256>,
        //   input: Bytes,
        //   max_priority_fee_per_gas: Option<U256>
        //   max_fee_per_gas: Option<U256
    ]
}

pub fn default_columns() -> Vec<Column<EthBlock<TxHash>>> {
    //TODO(2021-09-15) also include the uncle count
    //TODO(2022-09-21) also include the grafitti! And look into hitting an eth2 endpoint for more
    vec![
        Column {
            name: "blk num",
            // TODO(2022-09-21) make this dynamic? On gorli 7 is better, on a private chain 8
            // might be too small
            width: 8,
            render: Box::new(|block| match block.number {
                Some(number) => number.to_string(),
                None => "unknown".to_string(),
            }),
            enabled: true,
        },
        Column {
            name: "block hash",
            width: 12,
            render: Box::new(|block| match block.hash {
                Some(hash) => util::format_block_hash(hash.as_bytes()),
                None => "unknown".to_string(),
            }),
            enabled: true,
        },
        Column {
            name: "date UTC",
            width: 10,
            render: Box::new(|block| {
                let timestamp = block.timestamp;
                let low64 = timestamp.as_u64(); // TODO: panics if too big
                let low64signed = low64.try_into().unwrap(); // TODO: panic
                let naive_time = NaiveDateTime::from_timestamp(low64signed, 0);
                let time = DateTime::<Utc>::from_utc(naive_time, Utc);
                time.format("%Y-%m-%d").to_string()
            }),
            enabled: false,
        },
        Column {
            name: "time UTC",
            width: 8,
            render: Box::new(|block| {
                let timestamp = block.timestamp;
                let low64 = timestamp.as_u64(); // TODO: panics if too big
                let low64signed = low64.try_into().unwrap(); // TODO: panic
                let naive_time = NaiveDateTime::from_timestamp(low64signed, 0);
                let time = DateTime::<Utc>::from_utc(naive_time, Utc);
                // %Y-%m-%d for when you want to add the date back in
                time.format("%H:%M:%S").to_string()
            }),
            enabled: true,
        },
        Column {
            name: "parent hash",
            width: 12,
            render: Box::new(|block| util::format_block_hash(block.parent_hash.as_bytes())),
            enabled: true,
        },
        Column {
            name: "coinbase",
            width: 12,
            render: Box::new(|block| util::format_block_hash(block.author.as_bytes())),
            enabled: true,
        },
        Column {
            name: "gas used",
            width: 9,
            render: Box::new(|block| block.gas_used.to_string()),
            enabled: true,
        },
        Column {
            name: "gas limit",
            width: 9,
            render: Box::new(|block| block.gas_limit.to_string()),
            enabled: true,
        },
        Column {
            name: "base fee",
            width: 8,
            render: Box::new(|block| match block.base_fee_per_gas {
                None => "???".to_string(),
                Some(base_fee) => util::humanize_u256(base_fee),
            }),
            enabled: true,
        },
        Column {
            name: "txns",
            width: 4,
            render: Box::new(|block| block.transactions.len().to_string()),
            enabled: true,
        },
        Column {
            name: "size",
            width: 6,
            render: Box::new(|block| match block.size {
                Some(size) => size.to_string(),
                None => "none".to_string(), // blocks from eth_subscribe have no size
            }),
            enabled: true,
        },
    ]
}

pub fn columns_to_header<T>(columns: &Vec<Column<T>>) -> Spans<'static> {
    let underline_style = Style::default().add_modifier(Modifier::UNDERLINED);
    Spans::from(
        columns
            .iter()
            .filter(|col| col.enabled)
            .fold(Vec::new(), |mut accum, column| {
                // soon rust iterators will have an intersperse method
                if accum.len() != 0 {
                    accum.push(Span::raw(" "));
                }
                let filled = format!("{:<width$}", column.name, width = column.width);
                accum.push(Span::styled(filled, underline_style));
                accum
            }),
    )
}

pub fn render_item_with_cols<T>(columns: &Vec<Column<T>>, item: &T) -> String {
    columns
        .iter()
        .filter(|col| col.enabled)
        .fold(String::new(), |mut accum, column| {
            if accum.len() != 0 {
                accum.push_str(" ");
            }
            let rendered = (column.render)(item);
            let filled = format!("{:>width$}", rendered, width = column.width);

            accum.push_str(&filled);
            accum
        })
}
