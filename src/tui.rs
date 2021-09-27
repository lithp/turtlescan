use crate::data;
use crate::util;

use chrono::{DateTime, NaiveDateTime, Utc};
use ethers_core::types::{Block as EthBlock, Transaction, TransactionReceipt, TxHash, U64};
use ethers_providers::{Provider, Ws};
use log::{debug, warn};
use signal_hook::consts::signal::*;
use signal_hook::iterator::Signals;
use std::cmp;
use std::convert::TryInto;
use std::error::Error;
use std::io;
use std::path;
use std::thread;
use std::time::Instant;
use termion::event::Key;
use termion::input::TermRead;
use termion::raw::IntoRawMode;
use termion::screen::AlternateScreen;
use tui::backend::Backend;
use tui::backend::TermionBackend;
use tui::buffer::Buffer;
use tui::layout::{Constraint, Direction, Layout, Rect};
use tui::style::{Color, Modifier, Style};
use tui::text::{Span, Spans, Text};
use tui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Paragraph, StatefulWidget, Widget,
};
use tui::Frame;
use tui::Terminal;

pub enum UIMessage {
    // the user has given us some input over stdin
    Key(termion::event::Key),

    // something in the background has updated state and wants the UI to rerender
    Refresh(),

    Response(data::Response),
}

struct Column<T> {
    name: &'static str,
    width: usize,
    enabled: bool,
    render: Box<dyn Fn(&T) -> String>,
}

fn default_receipt_columns() -> Vec<Column<TransactionReceipt>> {
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

fn default_txn_columns() -> Vec<Column<Transaction>> {
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

fn default_columns() -> Vec<Column<EthBlock<TxHash>>> {
    //TODO(2021-09-15) also include the uncle count
    vec![
        Column {
            name: "blk num",
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

fn render_item_with_cols<T>(columns: &Vec<Column<T>>, item: &T) -> String {
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

fn columns_to_list_items<T>(columns: &Vec<Column<T>>, offset: usize) -> Vec<ListItem<'static>> {
    columns
        .iter()
        .fold((Vec::new(), offset), |(mut result, count), col| {
            if col.enabled {
                let s = format!("[{}] {}", count, col.name);
                result.push(ListItem::new(Span::raw(s)));
                (result, count + 1)
            } else {
                let s = format!("[ ] {}", col.name);
                result.push(ListItem::new(Span::raw(s)));
                (result, count)
            }
        })
        .0
}

fn block_to_txn_list_items(
    txn_columns: &Vec<Column<Transaction>>,
    receipt_columns: &Vec<Column<TransactionReceipt>>,
    block: &EthBlock<Transaction>,
    receipts: &Vec<data::RequestStatus<TransactionReceipt>>,
) -> Vec<ListItem<'static>> {
    if block.transactions.len() == 0 {
        return vec![ListItem::new(Span::raw("this block has no transactions"))];
    }

    assert!(block.transactions.len() == receipts.len());

    let txn_spans: Vec<Span> = block
        .transactions
        .iter()
        .map(|txn| {
            let formatted = render_item_with_cols(txn_columns, txn);
            Span::raw(formatted)
        })
        .collect();

    use data::RequestStatus::*;
    let receipt_spans: Vec<Span> = receipts
        .iter()
        .map(|status| match status {
            Waiting() | Started() => Span::raw("fetching"),
            Completed(receipt) => {
                let formatted = render_item_with_cols(receipt_columns, receipt);
                Span::raw(formatted)
            }
        })
        .collect();

    txn_spans
        .into_iter()
        .zip(receipt_spans)
        .map(|(txn, receipt)| ListItem::new(Spans::from(vec![txn, Span::raw(" "), receipt])))
        .collect()
}

const FOCUSED_BORDER: Color = Color::Gray;
const UNFOCUSED_BORDER: Color = Color::DarkGray;

const FOCUSED_SELECTION: Color = Color::LightGreen;
const UNFOCUSED_SELECTION: Color = Color::Green;

fn border_color(focused: bool) -> Color {
    match focused {
        true => FOCUSED_BORDER,
        false => UNFOCUSED_BORDER,
    }
}

fn selection_color(focused: bool) -> Color {
    match focused {
        true => FOCUSED_SELECTION,
        false => UNFOCUSED_SELECTION,
    }
}

#[derive(Debug, Clone)]
enum PaneState {
    JustBlocks,

    /// usize is the index of the focused_pane: 0 or 1
    BlocksTransactions(usize),

    /// usize is the index of the focused pane: 0, 1, or 2
    BlocksTransactionsTransaction(usize),
}

impl PaneState {
    fn next(&self) -> Self {
        use PaneState::*;

        match self {
            JustBlocks => JustBlocks,
            BlocksTransactions(focused) => BlocksTransactions((focused + 1).rem_euclid(2)),
            BlocksTransactionsTransaction(focused) => {
                BlocksTransactionsTransaction((focused + 1).rem_euclid(3))
            }
        }
    }

    fn prev(&self) -> Self {
        use PaneState::*;

        match self {
            JustBlocks => JustBlocks,
            BlocksTransactions(focused) => BlocksTransactions((focused + 1).rem_euclid(2)),
            BlocksTransactionsTransaction(focused) => BlocksTransactionsTransaction(
                // 2 is the inverse of 1 so adding it is equiv to subtracting 1
                (focused + 2).rem_euclid(3),
            ),
        }
    }

    fn focus(&self) -> FocusedPane {
        use FocusedPane::*;
        use PaneState::*;
        match self {
            JustBlocks => Blocks,
            BlocksTransactions(0) => Blocks,
            BlocksTransactions(1) => Transactions,
            BlocksTransactions(_) => unreachable!(),
            BlocksTransactionsTransaction(0) => Blocks,
            BlocksTransactionsTransaction(1) => Transactions,
            BlocksTransactionsTransaction(2) => Transaction,
            BlocksTransactionsTransaction(_) => unreachable!(),
        }
    }

    fn close_all_to_right(&self) -> PaneState {
        use FocusedPane::*;
        use PaneState::*;

        match self.focus() {
            Blocks => JustBlocks,
            Transactions => BlocksTransactions(1),
            Transaction => BlocksTransactionsTransaction(2),
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq)]
enum FocusedPane {
    Blocks,
    Transactions,
    Transaction,
}

const BLOCK_LIST_BORDER_HEIGHT: usize = 3;

pub struct TUI<'a, T: data::Data> {
    /* UI state */
    block_list_height: Option<usize>,
    block_list_top_block: Option<u64>,
    block_list_selected_block: Option<u64>,

    column_list_state: ListState,

    txn_list_state: ListState,
    txn_list_length: Option<usize>,

    configuring_columns: bool,
    pane_state: PaneState,

    txn_columns: Vec<Column<Transaction>>,
    txn_column_len: usize,

    receipt_columns: Vec<Column<TransactionReceipt>>,
    receipt_column_len: usize,

    columns: Vec<Column<EthBlock<TxHash>>>,
    column_items_len: usize,

    database: &'a mut T,
}

fn list_state_with_selection(selection: Option<usize>) -> ListState {
    let mut res = ListState::default();
    res.select(selection);
    res
}

fn scroll_up_one(state: &mut ListState, item_count: usize) {
    if item_count == 0 {
        state.select(None);
        return;
    }

    match state.selected() {
        // this is exhaustive because usize cannot be negative
        None | Some(0) => {
            state.select(Some(item_count - 1));
        }
        Some(current_selection) => state.select(Some(current_selection - 1)),
    }
}

fn scroll_down_one(state: &mut ListState, item_count: usize) {
    let desired_state = state.selected().map(|i| i + 1).unwrap_or(0);
    state.select(desired_state.checked_rem(item_count));

    // the above two-liner is cute but the code below is probably better:

    /*
    match state.selected() {
        None => {
            state.select(Some(0));
        }
        Some(current_selection) => {
            if item_count == 0 {
                state.select(None);
            } else if current_selection >= item_count - 1 {
                state.select(Some(0));
            } else {
                state.select(Some(current_selection + 1))
            }
        }
    }
    */
}

fn block_list_bounds(height: u64, top_block: u64, selected_block: Option<u64>) -> (u64, u64) {
    // if you have a list of height 1 the top block and bottom block are the same
    let height_offset = height.saturating_sub(1);

    match selected_block {
        None => {
            let bottom_block = top_block.saturating_sub(height_offset);
            return (bottom_block, top_block);
        }
        Some(selection) => {
            if selection > top_block {
                // we need to scroll up
                let bottom_block = selection.saturating_sub(height_offset);
                return (bottom_block, selection);
            }

            let bottom_block = top_block.saturating_sub(height_offset);
            if selection < bottom_block {
                // we need to scroll down
                let top_block = selection + height_offset;
                return (selection, top_block);
            }

            return (bottom_block, top_block);
        }
    }
}

impl<'a, T: data::Data> TUI<'a, T> {
    pub fn new(database: &'a mut T) -> TUI<'a, T> {
        let txn_columns = default_txn_columns();
        let txn_column_len = txn_columns.len();

        let columns = default_columns();
        let column_items_len = columns.len();

        let receipt_columns = default_receipt_columns();
        let receipt_column_len = receipt_columns.len();

        TUI {
            block_list_height: None,

            block_list_top_block: None,
            block_list_selected_block: None,

            column_list_state: list_state_with_selection(Some(0)),

            txn_list_state: ListState::default(),
            txn_list_length: None,

            configuring_columns: false,
            pane_state: PaneState::JustBlocks,

            txn_columns: txn_columns,
            txn_column_len: txn_column_len,

            columns: columns,
            column_items_len: column_items_len,

            receipt_columns: receipt_columns,
            receipt_column_len: receipt_column_len,

            database: database,
        }
    }

    fn apply_progress(&mut self, progress: data::Response) {
        if let data::Response::NewBlock(ref block) = progress {
            // it may seem a little weird that we throw away most of the {block} struct
            // which is passed in but this block came from eth_subscription and a bunch of
            // Option's are None.
            let blocknum = block.number.unwrap().low_u64();
            debug!("UI received new block blocknum={}", blocknum);
            self.handle_new_block(blocknum);
        }

        self.database.apply_progress(progress);
    }

    fn column_count(&self) -> usize {
        match self.pane_state.focus() {
            FocusedPane::Blocks => self.column_items_len,
            FocusedPane::Transactions => self.txn_column_len + self.receipt_column_len,
            FocusedPane::Transaction => 0, // nothing to configure
        }
    }

    fn toggle_configuring_columns(&mut self) {
        // this intentionally does not reset column_list_state
        match self.configuring_columns {
            true => self.configuring_columns = false,
            false => self.configuring_columns = true,
        };
    }

    pub fn handle_key_right(&mut self) {
        // we're going to move the focus, if a pane exists, and create it if it does not
        use PaneState::*;

        match self.pane_state {
            JustBlocks => {
                if let Some(_) = self.block_list_selected_block {
                    // no need to worry about which block is selected, the UI will
                    // read it again when we draw on the next loop
                    self.pane_state = BlocksTransactions(1);
                }
            }

            BlocksTransactions(0) => self.pane_state = BlocksTransactions(1),
            BlocksTransactions(1) => match self.txn_list_state.selected() {
                None => {}
                Some(_) => {
                    // check that there exist any transactions
                    self.pane_state = BlocksTransactionsTransaction(2);
                }
            },
            BlocksTransactions(_) => unreachable!(),

            BlocksTransactionsTransaction(0) => {
                self.pane_state = BlocksTransactionsTransaction(1);
            }
            BlocksTransactionsTransaction(1) => {
                self.pane_state = BlocksTransactionsTransaction(2);
            }

            // we're already all the way to the right, nothing more to do
            BlocksTransactionsTransaction(2) => {}
            BlocksTransactionsTransaction(_) => unreachable!(),
        }
    }

    fn handle_key_left(&mut self) {
        if self.pane_state.focus() == FocusedPane::Blocks {
            self.pane_state = PaneState::JustBlocks;
            return;
        }

        self.pane_state = self.pane_state.prev();
        self.pane_state = self.pane_state.close_all_to_right();
    }

    fn scroll_block_list_up_one_page(&mut self) {
        if let None = self.block_list_top_block {
            return;
        }
        let top_block = self.block_list_top_block.unwrap();

        if let None = self.block_list_height {
            return;
        }
        let height = self.block_list_height.unwrap() as u64;

        let highest = self.database.get_highest_block();
        if let None = highest {
            return;
        }
        let highest = highest.unwrap();

        if let None = self.block_list_selected_block {
            let new_top = top_block + height;
            let new_top = cmp::min(highest, new_top);
            self.block_list_top_block = Some(new_top);
            return;
        }

        let selection = self.block_list_selected_block.unwrap();
        let new_selection = selection + height;
        let new_selection = cmp::min(highest, new_selection);
        self.select_block(Some(new_selection));
    }

    fn scroll_txn_list_up_one_page(&mut self) {
        if let None = self.block_list_height {
            return;
        }
        let height = self.block_list_height.unwrap();

        if !self.selected_block_has_transactions() {
            return;
        }

        match self.txn_list_state.selected() {
            None => {}
            Some(selection) => {
                self.txn_list_state
                    .select(Some(selection.saturating_sub(height)));
            }
        }
    }

    fn scroll_txn_list_down_one_page(&mut self) {
        if let None = self.block_list_height {
            return;
        }
        let height = self.block_list_height.unwrap();

        if !self.selected_block_has_transactions() {
            return;
        }

        match self.txn_list_state.selected() {
            None => {
                // this list behaves inconsistently with the block list,
                // the block list allows you to pgup/pgdn even if no block is selected
                // that is not possible here because tui::List was not designed to allow
                // it. In order to support this we'll need to copy the (MIT licensed) List
                // implementation and modify it to allow tweaking offset
            }
            Some(selection) => {
                let length = self.txn_list_length.unwrap();
                let candidate_selection = selection + height;
                let selection = cmp::min(candidate_selection, length - 1);
                self.txn_list_state.select(Some(selection));
            }
        }
    }

    fn handle_scroll_up_one_page(&mut self) {
        match self.pane_state.focus() {
            FocusedPane::Blocks => self.scroll_block_list_up_one_page(),
            FocusedPane::Transactions => self.scroll_txn_list_up_one_page(),
            FocusedPane::Transaction => {}
        }
    }

    fn handle_scroll_down_one_page(&mut self) {
        match self.pane_state.focus() {
            FocusedPane::Blocks => self.scroll_block_list_down_one_page(),
            FocusedPane::Transactions => self.scroll_txn_list_down_one_page(),
            FocusedPane::Transaction => {}
        }
    }

    fn scroll_block_list_down_one_page(&mut self) {
        if let None = self.block_list_top_block {
            return;
        }
        let top_block = self.block_list_top_block.unwrap();

        if let None = self.block_list_height {
            return;
        }
        let height = self.block_list_height.unwrap() as u64;

        if let None = self.block_list_selected_block {
            let new_top = top_block.saturating_sub(height);
            self.block_list_top_block = Some(new_top);
            return;
        }

        let selection = self.block_list_selected_block.unwrap();
        let new_selection = selection.saturating_sub(height);
        self.select_block(Some(new_selection));
    }

    fn handle_scroll_to_top(&mut self) {
        if self.configuring_columns {
            self.column_list_state.select(Some(0));
            return;
        }

        match self.pane_state.focus() {
            FocusedPane::Blocks => self.scroll_block_list_to_top(),
            FocusedPane::Transactions => {
                if self.selected_block_has_transactions() {
                    self.txn_list_state.select(Some(0));
                } else {
                    assert!(self.txn_list_state.selected().is_none());
                }
            }
            FocusedPane::Transaction => {}
        };
    }

    fn handle_scroll_to_bottom(&mut self) {
        if self.configuring_columns {
            let length = self.column_count();
            self.column_list_state
                .select(Some(length.saturating_sub(1)));
            return;
        }

        match self.pane_state.focus() {
            FocusedPane::Blocks => self.scroll_block_list_to_bottom(),
            FocusedPane::Transactions => {
                if self.selected_block_has_transactions() {
                    let length = self.txn_list_length.unwrap();
                    self.txn_list_state.select(Some(length.saturating_sub(1)))
                } else {
                    assert!(self.txn_list_state.selected().is_none());
                }
            }
            FocusedPane::Transaction => {}
        };
    }

    fn scroll_block_list_to_top(&mut self) {
        if let Some(highest) = self.database.get_highest_block() {
            self.select_block(Some(highest));
        }
    }

    fn scroll_block_list_to_bottom(&mut self) {
        self.select_block(Some(0));
    }

    fn handle_key_up(&mut self) {
        match self.configuring_columns {
            false => match self.pane_state.focus() {
                FocusedPane::Blocks => {
                    match self.block_list_selected_block {
                        None => {}
                        Some(selection) => {
                            if let Some(highest) = self.database.get_highest_block() {
                                let new_selection = cmp::min(highest, selection + 1);
                                self.select_block(Some(new_selection));
                            }
                        }
                    };
                }
                FocusedPane::Transactions => {
                    let item_count = self.txn_list_length.unwrap_or(0);
                    scroll_up_one(&mut self.txn_list_state, item_count);
                }
                FocusedPane::Transaction => {}
            },
            true => {
                let item_count = self.column_count();
                scroll_up_one(&mut self.column_list_state, item_count);
            }
        };
    }

    pub fn handle_key_down(&mut self) {
        match self.configuring_columns {
            true => {
                let col_count = self.column_count();
                scroll_down_one(&mut self.column_list_state, col_count);
            }
            false => match self.pane_state.focus() {
                FocusedPane::Blocks => {
                    match self.block_list_selected_block {
                        None => self.select_block(self.block_list_top_block),
                        Some(selected_block) => {
                            let new_selection = selected_block.saturating_sub(1);
                            self.select_block(Some(new_selection));
                        }
                    };
                }
                FocusedPane::Transactions => {
                    let item_count = self.txn_list_length.unwrap_or(0);
                    scroll_down_one(&mut self.txn_list_state, item_count);
                }
                FocusedPane::Transaction => {}
            },
        };
    }

    fn handle_key_space(&mut self) {
        match self.configuring_columns {
            false => (),
            true => {
                if let Some(i) = self.column_list_state.selected() {
                    // TODO(2021-09-11) this entire codebase is ugly but this is
                    //                  especially gnarly
                    match self.pane_state.focus() {
                        FocusedPane::Blocks => {
                            self.columns[i].enabled = !self.columns[i].enabled;
                        }
                        FocusedPane::Transactions => {
                            if i < self.txn_columns.len() {
                                self.txn_columns[i].enabled = !self.txn_columns[i].enabled;
                            } else {
                                let i = i - self.txn_columns.len();
                                self.receipt_columns[i].enabled = !self.receipt_columns[i].enabled;
                            }
                        }
                        FocusedPane::Transaction => {
                            // this pane has no columns to configure
                        }
                    };
                }
            }
        };
    }

    fn handle_tab(&mut self, forward: bool) {
        self.pane_state = match forward {
            true => self.pane_state.next(),
            false => self.pane_state.prev(),
        };
    }

    fn handle_new_block(&mut self, blocknum: u64) {
        // in the typical case this is a new block extending the canonical chain
        // however, during a reorg we will receive a sequence of new blocks which
        // overwrite existing blocks in the canonical chain. By removing the entries we
        // cause the UI to trigger new fetches next time a draw() happens.
        // this will cause the UI to temporarily be in an inconsistent state, the shown
        // sequence of blocks will not form a consistent chain, but that state of affairs
        // should only persist for a few frames.
        self.database.invalidate_block(blocknum);

        if let Some(top) = self.block_list_top_block {
            if let Some(highest) = self.database.get_highest_block() {
                if top == highest {
                    /*
                     * This might not be the best design, it relies on how some other part
                     * of the code works. During draw if there's a conflict between
                     * top_block and selected_block then selected_block takes priority.
                     *
                     * So, this line will cause us to scroll up as new blocks arrive, but
                     * only if that doesn't push the currently selected block off-screen
                     */
                    self.block_list_top_block = Some(blocknum);
                }
            }
        }

        self.database.bump_highest_block(blocknum);
    }

    fn draw_popup<B: Backend>(&mut self, frame: &mut Frame<B>) {
        let column_items: Vec<ListItem> = match self.pane_state.focus() {
            FocusedPane::Blocks => columns_to_list_items(&self.columns, 0),
            FocusedPane::Transactions => {
                let mut items = columns_to_list_items(&self.txn_columns, 0);
                items.extend(columns_to_list_items(
                    &self.receipt_columns,
                    self.txn_columns.len(),
                ));
                items
            }
            FocusedPane::Transaction => vec![ListItem::new(Span::raw("nothing to configure"))],
        };

        let popup = List::new(column_items.clone())
            .block(Block::default().title("Columns").borders(Borders::ALL))
            .highlight_style(Style::default().bg(Color::LightGreen));

        let frame_size = frame.size();
        let (popup_height, popup_width) = (15, 30);
        let area = centered_rect(frame_size, popup_height, popup_width);

        frame.render_widget(Clear, area);
        frame.render_stateful_widget(popup, area, &mut self.column_list_state);
    }

    fn txn_list_selected_txn_index(&mut self) -> Option<(u64, usize)> {
        let selected_block: Option<u64> = self.block_list_selected_block;

        if let None = selected_block {
            return None;
        }
        let selected_block: u64 = selected_block.unwrap();

        match self.txn_list_state.selected() {
            None => None,
            Some(offset) => Some((selected_block, offset)),
        }
    }

    fn txn_list_selected_txn(&mut self) -> Option<Transaction> {
        let selection = self.txn_list_selected_txn_index();
        if let None = selection {
            return None;
        }
        let (block, offset) = selection.unwrap();

        use data::RequestStatus::*;
        let fetch = self.database.get_block_with_transactions(block);
        match fetch {
            Waiting() | Started() => return None,
            Completed(block) => {
                if offset >= block.transactions.len() {
                    warn!("offset should have been reset");
                    return None;
                }

                return Some(block.transactions[offset].clone());
            }
        }
    }

    fn txn_list_selected_receipt(&mut self) -> Option<TransactionReceipt> {
        let selection = self.txn_list_selected_txn_index();
        if let None = selection {
            return None;
        }
        let (block, offset) = selection.unwrap();

        use data::RequestStatus::*;
        // let fetch = self.database.get_block_receipts(block);
        let fetch = self
            .database
            .get_block_receipt(block, offset)
            .expect("inconsistent state");
        match fetch {
            Waiting() | Started() => return None,
            Completed(receipt) => {
                // TODO: the value was already cloned by get_block_receipts...
                return Some(receipt.clone());
            }
        }
    }

    fn draw_transaction_details<B: Backend>(&mut self, frame: &mut Frame<B>, area: Rect) {
        let selected_txn: Option<Transaction> = self.txn_list_selected_txn();

        let title = selected_txn
            .as_ref()
            .map(|txn| util::format_block_hash(txn.hash.as_bytes()))
            .unwrap_or("Transaction".to_string());

        let text = if let Some(txn) = selected_txn {
            let receipt = self.txn_list_selected_receipt();
            let receipt_spans = match receipt {
                None => vec![Spans::from("receipt: (fetching)")],
                Some(receipt) => {
                    //TODO(2021-09-15) no reason to build a new one every time
                    //                 how do rust globals work? thread locals?
                    let columns = default_receipt_columns();

                    let mut result = vec![Spans::from("receipt:")];

                    result.extend::<Vec<Spans>>(
                        columns
                            .iter()
                            .map(|col| {
                                Spans::from(format!("  {}: {}", col.name, (col.render)(&receipt)))
                            })
                            .collect(),
                    );

                    result
                }
            };

            let mut txn_spans = vec![
                Spans::from("txn:"),
                //TODO: should probably unify these with the txn columns
                //TODO: widen the hash based on available space
                Spans::from(format!("  hash: {}", txn.hash.to_string())),
                Spans::from(format!("  from: {}", txn.from.to_string())),
                Spans::from(format!("    nonce: {}", txn.nonce.to_string())),
                Spans::from(format!(
                    "  to: {}",
                    txn.to
                        .map(|hash| hash.to_string())
                        .unwrap_or("None".to_string())
                )),
                Spans::from(format!("  value: {}", txn.value.to_string())),
                // TODO(2021-09-15) implement this
                // Spans::from(format!("  data: {}", txn.input.to_string())),
                Spans::from(format!(
                    "  gas price: {}",
                    txn.gas_price
                        .map(|price| price.to_string())
                        .unwrap_or("None".to_string())
                )),
                Spans::from(format!(
                    "  max priority fee: {}",
                    txn.max_priority_fee_per_gas
                        .map(|fee| fee.to_string())
                        .unwrap_or("None".to_string())
                )),
                Spans::from(format!(
                    "  max gas fee: {}",
                    txn.max_fee_per_gas
                        .map(|fee| fee.to_string())
                        .unwrap_or("None".to_string())
                )),
            ];
            txn_spans.extend(receipt_spans);

            Text::from(txn_spans)
        } else {
            Text::raw("")
        };

        let is_focused = self.pane_state.focus() == FocusedPane::Transaction;
        let widget = Paragraph::new(text).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color(is_focused)))
                .title(title),
        );
        frame.render_widget(widget, area);
    }

    fn draw_block_list<B: Backend>(&mut self, frame: &mut Frame<B>, area: Rect) {
        assert!(!self.database.get_highest_block().is_none());
        assert!(!self.block_list_top_block.is_none());

        let target_height = area.height.saturating_sub(BLOCK_LIST_BORDER_HEIGHT as u16);

        // TODO: will this ever panic? u16 into a usize
        self.block_list_height = Some(target_height.into());

        if target_height <= 0 {
            return;
        }

        let target_height = target_height as u64;
        let (bottom_block, top_block) = block_list_bounds(
            target_height,
            self.block_list_top_block.unwrap(),
            self.block_list_selected_block,
        );
        self.block_list_top_block = Some(top_block);

        let mut block_list_state = ListState::default();
        if let Some(selection) = self.block_list_selected_block {
            //TODO: error handling?
            let offset = top_block - selection;
            block_list_state.select(Some(offset as usize));
        }

        let header = columns_to_header(&self.columns);

        let block_range = (bottom_block)..(top_block + 1);
        let block_lines = {
            let block_lines: Vec<ListItem> = block_range
                .rev()
                .map(|blocknum| (blocknum, self.database.get_block(blocknum)))
                // if we do not do this rust complains there are multiple active closures
                // which reference self which... might be a legitimate complaint?
                // TODO(2021-09-16) any better ways to fix this problem?
                .collect::<Vec<(u64, data::RequestStatus<EthBlock<TxHash>>)>>()
                .iter()
                .map(|(height, fetch)| {
                    use data::RequestStatus::*;
                    let formatted = match fetch {
                        Waiting() => format!("{} waiting", height),
                        Started() => format!("{} fetching", height),
                        Completed(block) => render_item_with_cols(&self.columns, &block),
                    };
                    ListItem::new(Span::raw(formatted))
                })
                .collect();
            block_lines
        };

        let highest_block = self.database.get_highest_block().unwrap();
        let is_behind = top_block != highest_block;
        let title = match is_behind {
            false => "Blocks".to_string(),
            true => format!("Blocks ({} behind)", highest_block - top_block),
        };

        let is_focused = self.pane_state.focus() == FocusedPane::Blocks;
        let block_list = HeaderList::new(block_lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border_color(is_focused)))
                    .title(title),
            )
            .highlight_style(Style::default().bg(selection_color(is_focused)))
            .header(header);
        frame.render_stateful_widget(block_list, area, &mut block_list_state);
    }

    fn txn_list_title(&mut self) -> &'static str {
        const READY: &str = "Transactions";
        const FETCHING_TXNS: &str = "Transactions (fetching)";
        // const FETCHING_RECEIPTS: &str = "Transactions (fetching receipts)";

        let block = self.block_list_selected_block;
        if let None = block {
            return READY;
        }
        let block = block.unwrap();

        use data::RequestStatus::*;
        let blockfetch = self.database.get_block_with_transactions(block);
        match blockfetch {
            Waiting() | Started() => return FETCHING_TXNS,
            Completed(_) => (),
        }

        return READY;

        /*
        let receiptsfetch = self.database.get_block_receipts(block);
        match receiptsfetch {
            Waiting() | Started() => return FETCHING_RECEIPTS,
            Completed(_) => return READY,
        }
        */
    }

    fn draw_txn_list<B: Backend>(&mut self, frame: &mut Frame<B>, area: Rect) {
        let block_selection = self.block_list_selected_block;

        let txn_items = if let Some(block_at_offset) = block_selection {
            let block_fetch = self.database.get_block_with_transactions(block_at_offset);

            self.txn_list_length = None;
            use data::RequestStatus::*;
            match block_fetch {
                Waiting() => {
                    self.txn_list_state.select(None);

                    vec![ListItem::new(Span::raw(format!(
                        "{} waiting",
                        block_at_offset
                    )))]
                }
                Started() => {
                    self.txn_list_state.select(None);

                    vec![ListItem::new(Span::raw(format!(
                        "{} fetching",
                        block_at_offset
                    )))]
                }
                Completed(block) => {
                    let has_transactions = block.transactions.len() > 0;
                    let nothing_selected = self.txn_list_state.selected().is_none();
                    if nothing_selected && has_transactions {
                        self.txn_list_state.select(Some(0));
                    }

                    let receipts: Vec<data::RequestStatus<TransactionReceipt>> = block
                        .transactions
                        .iter()
                        .map(|txn| txn.hash)
                        .map(|txhash| self.database.get_transaction_receipt(txhash))
                        .collect();

                    self.txn_list_length = Some(block.transactions.len());
                    block_to_txn_list_items(
                        &self.txn_columns,
                        &self.receipt_columns,
                        &block,
                        &receipts,
                    )
                }
            }
        } else {
            Vec::new()
        };

        let header = {
            let mut txn_header: Spans = columns_to_header(&self.txn_columns);
            let receipt_header: Spans = columns_to_header(&self.receipt_columns);
            txn_header.0.push(Span::raw(" "));
            txn_header.0.extend(receipt_header.0);
            txn_header
        };

        let title = self.txn_list_title();

        let is_focused = self.pane_state.focus() == FocusedPane::Transactions;
        let txn_list = HeaderList::new(txn_items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border_color(is_focused)))
                    .title(title),
            )
            .highlight_style(Style::default().bg(selection_color(is_focused)))
            .header(header);
        frame.render_stateful_widget(txn_list, area, &mut self.txn_list_state);
    }

    pub fn draw<B: Backend>(&mut self, frame: &mut Frame<B>) {
        let waiting_for_initial_block = self.database.get_highest_block().is_none();
        if waiting_for_initial_block {
            frame.render_widget(
                Paragraph::new(Span::raw("fetching current block number")),
                frame.size(),
            );
            return;
        }

        if let None = self.block_list_top_block {
            self.block_list_top_block = self.database.get_highest_block();
            assert!(!self.block_list_top_block.is_none());
        };

        let vert_chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([Constraint::Min(0), Constraint::Length(2)].as_ref())
            .split(frame.size());

        let pane_chunk = vert_chunks[0];

        match self.pane_state {
            PaneState::JustBlocks => {
                self.draw_block_list(frame, pane_chunk);
            }

            PaneState::BlocksTransactions(_) => {
                let (blocks_width, txns_width) = self.allocate_width_two_panes(pane_chunk.width);

                let horiz_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Length(blocks_width),
                        Constraint::Length(txns_width),
                    ])
                    .split(pane_chunk);

                self.draw_block_list(frame, horiz_chunks[0]);
                self.draw_txn_list(frame, horiz_chunks[1]);
            }

            PaneState::BlocksTransactionsTransaction(_) => {
                let horiz_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Min(0), Constraint::Length(40)])
                    .split(pane_chunk);

                let (blocks_width, txns_width) =
                    self.allocate_width_two_panes(horiz_chunks[0].width);
                let txn_pane_chunk = horiz_chunks[1];

                let horiz_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Length(blocks_width),
                        Constraint::Length(txns_width),
                    ])
                    .split(horiz_chunks[0]);

                self.draw_block_list(frame, horiz_chunks[0]);
                self.draw_txn_list(frame, horiz_chunks[1]);
                self.draw_transaction_details(frame, txn_pane_chunk);
            }
        }

        let bold_title = Span::styled("turtlescan", Style::default().add_modifier(Modifier::BOLD));

        let status_string = match self.configuring_columns {
            false => "  (q) quit - (c) configure columns - (←/→) open/close panes - (tab) change focused pane - (g/G) jump to top/bottom",
            true => "  (c) close col popup - (space) toggle column - (↑/↓) choose column",
        };

        let status_line = Paragraph::new(status_string).block(Block::default().title(bold_title));
        frame.render_widget(status_line, vert_chunks[1]);

        if self.configuring_columns {
            self.draw_popup(frame);
        }
    }

    fn allocate_width_two_panes(&self, available_width: u16) -> (u16, u16) {
        const HEADERS: u16 = 2;
        let blocks_desired_width = HEADERS + columns_to_desired_width(&self.columns) as u16;
        let txns_desired_width = HEADERS + {
            let txn_width = columns_to_desired_width(&self.txn_columns) as u16;
            let receipt_width = columns_to_desired_width(&self.receipt_columns) as u16;

            // we only need a spacer if both sources have enabled columns
            if cmp::min(txn_width, receipt_width) == 0 {
                txn_width + receipt_width
            } else {
                txn_width + receipt_width + 1
            }
        };

        // just enough room for the header to remain visible
        let blocks_min_width = 10;
        let transactions_min_width = 15;

        if available_width > blocks_desired_width + txns_desired_width {
            return (blocks_desired_width, txns_desired_width);
        }

        // there is not enough room for everybody to fit
        // the focused pane should get priority

        if self.pane_state.focus() == FocusedPane::Blocks {
            if available_width < blocks_min_width + transactions_min_width {
                return (available_width, 0);
            }

            let remaining_width = available_width.saturating_sub(blocks_desired_width);
            let txn_width = cmp::max(remaining_width, transactions_min_width);
            let block_width = available_width.saturating_sub(txn_width);
            return (block_width, txn_width);
        }

        if available_width < blocks_min_width + transactions_min_width {
            return (0, available_width);
        }

        let remaining_width = available_width.saturating_sub(txns_desired_width);
        let block_width = cmp::max(remaining_width, blocks_min_width);
        let txn_width = available_width.saturating_sub(block_width);
        return (block_width, txn_width);
    }

    fn select_block(&mut self, opt_blocknum: Option<u64>) {
        let selection_changed = opt_blocknum != self.block_list_selected_block;
        self.block_list_selected_block = opt_blocknum;

        // there's no need to change the scroll here (to adjust top_block), draw() will do
        // that for us

        if selection_changed {
            // it doesn't make sense to persist this if we're looking at
            // txns for a new block. In the far future maybe this should
            // track per-block scroll state?
            self.txn_list_state.select(None);
        }
    }

    fn selected_block_has_transactions(&self) -> bool {
        if let Some(length) = self.txn_list_length {
            return length > 0;
        }
        return false;
    }
}

// TODO(2021-08-27) why does the following line not work?
// fn run_tui() -> Result<(), Box<io::Error>> {
pub fn run_tui(provider: Provider<Ws>, cache_path: path::PathBuf) -> Result<(), Box<dyn Error>> {
    let stdout = io::stdout().into_raw_mode()?;
    let stdout = AlternateScreen::from(stdout);
    let backend = TermionBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (tx, rx) = crossbeam::channel::unbounded(); // tell the UI thread (this one) what to do

    // doing this in the background saves us from needing to do any kind of select!(),
    // all the UI thread needs to do is listen on its channel and all important events
    // will come in on that channel.
    let keys_tx = tx.clone();
    thread::spawn(move || {
        let stdin = io::stdin().keys();

        for key in stdin {
            let mapped = key.map(|k| UIMessage::Key(k));
            keys_tx.send(mapped).unwrap();
        }
    });

    // Immediately redraw when the terminal window resizes
    let winch_tx = tx.clone();
    thread::spawn(move || {
        let mut signals = Signals::new(&[SIGWINCH]).unwrap();

        for _signal in signals.forever() {
            winch_tx.send(Ok(UIMessage::Refresh())).unwrap();
        }
    });

    let mut database = data::Database::start(provider, cache_path);
    let database_results_rx = database.results_rx.clone();
    let mut tui = TUI::new(&mut database);

    // this loop could be easier to understand. it's convoluted because it attempts to
    // process all pending messages before it does any redraws, because redraws are
    // relatively expensive. this is one of the few times where the control flow would
    // be easier to understand if we were allowed to use goto
    let mut queue_was_empty = true;
    'main: loop {
        let message = if queue_was_empty {
            debug!("started draw");
            {
                let start = Instant::now();
                terminal.draw(|mut f| {
                    tui.draw(&mut f);
                })?;
                let duration = start.elapsed();

                debug!(" finished draw elapsed={:?}", duration);
            }

            crossbeam::channel::select! {
                recv(rx) -> msg_result => {
                    // first unwrap is crossbeam::channel::RecvError
                    // second unwrap is std::io::Error
                    msg_result.unwrap().unwrap()
                }
                recv(database_results_rx) -> msg_result => {
                    UIMessage::Response(msg_result.unwrap())
                }
            }
        } else {
            crossbeam::channel::select! {
                recv(rx) -> msg_result => {
                    let msg = msg_result.unwrap();
                    msg.unwrap()  // remove the io::Error
                }
                recv(database_results_rx) -> msg_result => {
                    let msg = msg_result.unwrap();
                    UIMessage::Response(msg)
                }
                default => {
                    queue_was_empty = true;
                    continue;
                }
            }
        };
        queue_was_empty = false;

        match message {
            UIMessage::Key(key) => match key {
                Key::Char('q') | Key::Esc | Key::Ctrl('c') => break 'main,
                Key::Char('c') => tui.toggle_configuring_columns(),
                Key::Up => tui.handle_key_up(),
                Key::Down => tui.handle_key_down(),
                Key::Right => tui.handle_key_right(),
                Key::Left => tui.handle_key_left(),
                Key::PageUp => tui.handle_scroll_up_one_page(),
                Key::PageDown => tui.handle_scroll_down_one_page(),
                Key::Char(' ') => tui.handle_key_space(),
                Key::Char('g') => tui.handle_scroll_to_top(),
                Key::Char('G') => tui.handle_scroll_to_bottom(),
                Key::Char('\t') => tui.handle_tab(true),
                Key::BackTab => tui.handle_tab(false),
                key => {
                    debug!("unhandled key press: {:?}", key)
                }
            },
            UIMessage::Refresh() => {}
            UIMessage::Response(progress) => {
                tui.apply_progress(progress);
            }
        };
    }

    Ok(())
}

fn centered_rect(frame_size: Rect, desired_height: u16, desired_width: u16) -> Rect {
    let height = cmp::min(desired_height, frame_size.height);
    let width = cmp::min(desired_width, frame_size.width);

    Rect {
        x: frame_size.x + (frame_size.width - width) / 2,
        y: frame_size.y + (frame_size.height - height) / 2,
        width: width,
        height: height,
    }
}

struct HeaderList<'a> {
    /*
     * a List where the first row is a header and does not participate in
     * scrolling or selection
     */
    // we need a lifetime because the Title uses &str to hold text
    block: Option<Block<'a>>,
    highlight_style: Style,

    header: Option<Spans<'a>>,
    items: Vec<ListItem<'a>>,
}

impl<'a> HeaderList<'a> {
    fn new(items: Vec<ListItem<'a>>) -> HeaderList<'a> {
        HeaderList {
            block: None,
            highlight_style: Style::default(),
            items: items,
            header: None,
        }
    }

    fn block(mut self, block: Block<'a>) -> HeaderList<'a> {
        self.block = Some(block);
        self
    }

    fn highlight_style(mut self, style: Style) -> HeaderList<'a> {
        self.highlight_style = style;
        self
    }

    fn header(mut self, header: Spans<'a>) -> HeaderList<'a> {
        self.header = Some(header);
        self
    }
}

fn columns_to_desired_width<T>(columns: &Vec<Column<T>>) -> usize {
    let spaces = columns.len().saturating_sub(1);
    let width = columns
        .iter()
        .filter(|col| col.enabled)
        .fold(0, |accum, column| accum + column.width);

    width + spaces
}

fn columns_to_header<T>(columns: &Vec<Column<T>>) -> Spans<'static> {
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

impl<'a> StatefulWidget for HeaderList<'a> {
    type State = ListState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let inner_area = match self.block {
            None => area,
            Some(block) => {
                let inner = block.inner(area);
                block.render(area, buf);
                inner
            }
        };

        if inner_area.height < 1 || inner_area.width < 1 {
            return;
        }

        let inner_area = match self.header {
            None => inner_area,
            Some(spans) => {
                let paragraph_area = Rect {
                    x: inner_area.x,
                    y: inner_area.y,
                    width: inner_area.width,
                    height: 1,
                };
                Paragraph::new(spans).render(paragraph_area, buf);

                // return the trimmed area
                Rect {
                    x: inner_area.x,
                    y: inner_area.y.saturating_add(1).min(inner_area.bottom()),
                    width: inner_area.width,
                    height: inner_area.height.saturating_sub(1),
                }
            }
        };

        if inner_area.height < 1 {
            return;
        }

        let inner_list = List::new(self.items).highlight_style(self.highlight_style);
        StatefulWidget::render(inner_list, inner_area, buf, state);
    }
}
