use ethers_core::types::{Block as EthBlock, Transaction, TransactionReceipt};
use tui::{backend::Backend, layout::Rect, Frame, text::{Spans, Span}, widgets::{ListState, Borders, ListItem, Block}, style::Style};

use crate::{data::RequestStatus, column::Column};
use crate::header_list::HeaderList;
use crate::style;
use crate::column;
use crate::data;

pub struct PaneTxnList<'a> {
    pub block_selection: Option<u64>,
    pub block_fetch: Option<RequestStatus<EthBlock<Transaction>>>,
    pub is_focused: bool,
    pub txn_selection: Option<usize>,  // the index of the selected transaction
    
    pub txn_columns: &'a Vec<Column<Transaction>>,
    pub receipt_columns: &'a Vec<Column<TransactionReceipt>>,
}

pub struct State
{
    pub txn_list_top: Option<usize>,
    pub txn_list_length: Option<usize>,
    pub txn_list_selected: Option<usize>,
}

impl State {
    // this duplicates TUI.reset_txn_list_scroll but it is difficult to get the borrow checker
    // to agree to let us call that method directly
    fn reset_scroll(&mut self) {
        self.txn_list_top = None;
        self.txn_list_selected = None;
    }
}

pub const TXN_LIST_BORDER_HEIGHT: usize = 3; // two for the border, one for the header

impl<'a> PaneTxnList<'a> {
    pub fn draw<B: Backend, T: data::Data>(
        &self,
        frame: &mut Frame<B>,
        area: Rect,
        state: &mut State,
        database: &mut T,
    ) {
        let target_height = (area.height as usize).saturating_sub(TXN_LIST_BORDER_HEIGHT);

        if target_height <= 0 {
            // nothing to draw
            return;
        }

        if let None = state.txn_list_top {
            state.txn_list_top = Some(0);
        }

        let top = txn_list_bounds(
            target_height,
            state.txn_list_top.unwrap(),
            self.txn_selection,
        );
        state.txn_list_top = Some(top);
            
        let txn_items = self.txn_items(database, state, top, target_height);

        let header = {
            let mut txn_header: Spans = column::columns_to_header(&self.txn_columns);
            let receipt_header: Spans = column::columns_to_header(&self.receipt_columns);
            txn_header.0.push(Span::raw(" "));
            txn_header.0.extend(receipt_header.0);
            txn_header
        };

        let mut txn_list_state = ListState::default();
        if let Some(selection) = self.txn_selection {
            let offset = selection.saturating_sub(top);
            txn_list_state.select(Some(offset));
        };

        let txn_list = HeaderList::new(txn_items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(style::border_color(self.is_focused)))
                    .title(self.title()),
            )
            .highlight_style(Style::default().bg(style::selection_color(self.is_focused)))
            .header(header);
        frame.render_stateful_widget(txn_list, area, &mut txn_list_state);
    }
    
    fn title(&self) -> &'static str {
        const READY: &str = "Transactions";
        const FETCHING_TXNS: &str = "Transactions (fetching)";
        // const FETCHING_RECEIPTS: &str = "Transactions (fetching receipts)";
        
        use data::RequestStatus::*;
        match self.block_fetch {
            None => READY,  // a weird state: the pane is open but no block is selected
            Some(Waiting()) | Some(Started()) => FETCHING_TXNS,
            Some(Completed(_)) => READY,
        }
    }
    
    fn txn_items<T: data::Data>(&self, database: &mut T, state: &mut State, top: usize, target_height: usize) -> Vec<ListItem> {
        if let Some(block_at_offset) = self.block_selection {
            // if the block selection contains a block then we were given a fetch
            // TODO: there's no need to also pass the block_selection
            let block_fetch = self.block_fetch.as_ref().unwrap();

            state.txn_list_length = None;
            use data::RequestStatus::*;
            match block_fetch {
                Waiting() => {
                    state.reset_scroll();
                    vec![ListItem::new(Span::raw(format!(
                        "{} waiting",
                        block_at_offset
                    )))]
                }
                Started() => {
                    state.reset_scroll();
                    vec![ListItem::new(Span::raw(format!(
                        "{} fetching",
                        block_at_offset
                    )))]
                }
                Completed(block) => {
                    let has_transactions = block.transactions.len() > 0;
                    let nothing_selected = state.txn_list_selected.is_none();
                    if nothing_selected && has_transactions {
                        state.txn_list_selected = Some(0);
                    }
                    
                    assert!(top <= block.transactions.len());

                    let receipts: Vec<data::RequestStatus<TransactionReceipt>> = block
                        .transactions
                        .iter()
                        // scrolling
                        .skip(top)
                        .take(target_height)
                        .map(|txn| txn.hash)
                        .map(|txhash| database.get_transaction_receipt(txhash))
                        .collect();

                    let transactions: Vec<&Transaction> = block
                        .transactions
                        .iter()
                        .skip(top)
                        .take(target_height)
                        .collect();

                    state.txn_list_length = Some(block.transactions.len());
                    block_to_txn_list_items(
                        &self.txn_columns,
                        &self.receipt_columns,
                        &transactions,
                        &receipts,
                    )
                }
            }
        } else {
            Vec::new()
        }
    }
}

fn block_to_txn_list_items(
    txn_columns: &Vec<Column<Transaction>>,
    receipt_columns: &Vec<Column<TransactionReceipt>>,
    transactions: &Vec<&Transaction>,
    receipts: &Vec<data::RequestStatus<TransactionReceipt>>,
) -> Vec<ListItem<'static>> {
    if transactions.len() == 0 {
        return vec![ListItem::new(Span::raw("this block has no transactions"))];
    }

    assert!(transactions.len() == receipts.len());

    let txn_spans: Vec<Span> = transactions
        .iter()
        .map(|txn| {
            let formatted = column::render_item_with_cols(txn_columns, txn);
            Span::raw(formatted)
        })
        .collect();

    use data::RequestStatus::*;
    let receipt_spans: Vec<Span> = receipts
        .iter()
        .map(|status| match status {
            Waiting() | Started() => Span::raw("fetching"),
            Completed(receipt) => {
                let formatted = column::render_item_with_cols(receipt_columns, receipt);
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

/// decides which transaction which should be at the top of the list
fn txn_list_bounds(height: usize, top_txn: usize, selected_txn: Option<usize>) -> usize {
    let height_offset = height.saturating_sub(1);

    match selected_txn {
        None => {
            // no adjustment needed
            return top_txn;
        }
        Some(selection) => {
            let bottom = top_txn + height_offset;
            if selection > bottom {
                // we need to scroll down
                return selection.saturating_sub(height_offset);
            }

            if selection < top_txn {
                // we need to scroll up!
                return selection;
            }

            return top_txn;
        }
    }
}