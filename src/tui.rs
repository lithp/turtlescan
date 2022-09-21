use crate::data;
use crate::pane_txn_details;
use crate::pane_txn_list;
use crate::pane_block_list;
use crate::column;
use crate::column::Column;

use ethers_core::types::{Block as EthBlock, Transaction, TransactionReceipt, TxHash};
use ethers_providers::{Provider, Ws};
use log::{debug, warn};
use signal_hook::consts::signal::*;
use signal_hook::iterator::Signals;
use std::cmp;
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
use tui::layout::{Constraint, Direction, Layout, Rect};
use tui::style::{Color, Modifier, Style};
use tui::text::Span;
use tui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Paragraph
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

    // this used to be called by handle_key_left but that interaction is a little confusing
    // leaving it in place in case I want to resurrect it under a new keybinding
    #[allow(dead_code)]
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

    txn_list_top: Option<usize>, // the index of the topmost transaction in view
    txn_list_selected: Option<usize>, // the index of the selected transaction
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
        let txn_columns = column::default_txn_columns();
        let txn_column_len = txn_columns.len();

        let columns = column::default_columns();
        let column_items_len = columns.len();

        let receipt_columns = column::default_receipt_columns();
        let receipt_column_len = receipt_columns.len();

        TUI {
            block_list_height: None,

            block_list_top_block: None,
            block_list_selected_block: None,

            column_list_state: list_state_with_selection(Some(0)),

            txn_list_top: None,
            txn_list_selected: None,
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
            BlocksTransactions(1) => match self.txn_list_selected {
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
        // the arrow keys take you left and right but if you try to scroll past the edge
        // you close all the other panes
        if self.pane_state.focus() == FocusedPane::Blocks {
            self.pane_state = PaneState::JustBlocks;
            return;
        }

        self.pane_state = self.pane_state.prev();
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

        match self.txn_list_selected {
            None => {}
            Some(selection) => {
                self.txn_list_selected = Some(selection.saturating_sub(height));
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

        match self.txn_list_selected {
            None => {}
            Some(selection) => {
                let length = self.txn_list_length.unwrap();
                let candidate_selection = selection + height;
                let selection = cmp::min(candidate_selection, length - 1);
                self.txn_list_selected = Some(selection);
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
                    self.txn_list_selected = Some(0);
                } else {
                    assert!(self.txn_list_selected.is_none());
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
                    self.txn_list_selected = Some(length.saturating_sub(1));
                } else {
                    assert!(self.txn_list_selected.is_none());
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
                    if let Some(selection) = self.txn_list_selected {
                        self.txn_list_selected = Some(selection.saturating_sub(1));
                    }
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
                    if let Some(selection) = self.txn_list_selected {
                        let txn_count = self.txn_list_length.unwrap_or(0);
                        let new_selection = cmp::min(txn_count.saturating_sub(1), selection + 1);
                        self.txn_list_selected = Some(new_selection);
                    }
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

        if let Some(selection) = self.txn_list_selected {
            Some((selected_block, selection))
        } else {
            None
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
        let pane = pane_txn_details::PaneTransactionDetails {
            txn: self.txn_list_selected_txn(),
            receipt: self.txn_list_selected_receipt(),
            is_focused: self.pane_state.focus() == FocusedPane::Transaction,
            area: area,
        };
        pane.draw(frame);
    }

    fn draw_block_list<B: Backend>(&mut self, frame: &mut Frame<B>, area: Rect) {
        assert!(self.database.get_highest_block().is_some());
        assert!(self.block_list_top_block.is_some());

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

        let is_focused = self.pane_state.focus() == FocusedPane::Blocks;
        let highest_block = self.database.get_highest_block().unwrap();
        let pane = pane_block_list::PaneBlockList {
            highest_block,
            selected_block: self.block_list_selected_block,
            top_block, bottom_block,
            columns: &self.columns,
            is_focused,
        };
        pane.draw(frame, area, self.database);
    }

    fn reset_txn_list_scroll(&mut self) {
        self.txn_list_top = None;
        self.txn_list_selected = None;
    }

    fn draw_txn_list<B: Backend>(&mut self, frame: &mut Frame<B>, area: Rect) {
        let pane = pane_txn_list::PaneTxnList {
            block_selection: self.block_list_selected_block,
            block_fetch: match self.block_list_selected_block {
                None => None,
                Some(blocknum) => Some(self.database.get_block_with_transactions(blocknum)),
            },
            is_focused: self.pane_state.focus() == FocusedPane::Transactions,
            txn_selection: self.txn_list_selected,
            
            txn_columns: &self.txn_columns,
            receipt_columns: &self.receipt_columns,
        };
        let mut state = pane_txn_list::State { 
            txn_list_top: self.txn_list_top,
            txn_list_length: self.txn_list_length,
            txn_list_selected: self.txn_list_selected,
        };
        pane.draw(frame, area, &mut state, self.database);
        
        // it would be cleaner to hold the State in self instead of copying back and forth
        self.txn_list_top = state.txn_list_top;
        self.txn_list_length = state.txn_list_length;
        self.txn_list_selected = state.txn_list_selected;
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
            false => "  (q) quit - (c) configure columns - (←/→/tab) change focused pane - (g/G) jump to top/bottom",
            true => "  (q/c) close col popup - (space) toggle column - (↑/↓) choose column",
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
            // it doesn't make sense to persist the scroll if we're looking at
            // txns for a new block. In the far future maybe this should
            // track per-block scroll state?
            self.reset_txn_list_scroll();
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
                Key::Ctrl('c') => break 'main,
                Key::Char('q') | Key::Esc => {
                    if tui.configuring_columns {
                        tui.toggle_configuring_columns()
                    } else {
                        break 'main
                    }
                },
                Key::Char('c') => tui.toggle_configuring_columns(),
                Key::Up | Key::Char('k') => tui.handle_key_up(),
                Key::Down | Key::Char('j') => tui.handle_key_down(),
                Key::Right | Key::Char('l') => tui.handle_key_right(),
                Key::Left | Key::Char('h') => tui.handle_key_left(),
                Key::PageUp | Key::Ctrl('u') => tui.handle_scroll_up_one_page(),
                Key::PageDown | Key::Ctrl('d') => tui.handle_scroll_down_one_page(),
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


fn columns_to_desired_width<T>(columns: &Vec<Column<T>>) -> usize {
    let spaces = columns.len().saturating_sub(1);
    let width = columns
        .iter()
        .filter(|col| col.enabled)
        .fold(0, |accum, column| accum + column.width);

    width + spaces
}
