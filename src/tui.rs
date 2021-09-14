use crate::util;

use std::cmp;
use std::io;
use std::sync::{Arc, Mutex};
use termion::raw::IntoRawMode;
use termion::screen::AlternateScreen;
use tui::backend::TermionBackend;
use tui::text::{Span, Spans};
use tui::Terminal;

use tui::backend::Backend;
use tui::layout::{Constraint, Direction, Layout, Rect};
use tui::style::{Color, Modifier, Style};
use tui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Paragraph, StatefulWidget, Widget,
};
use tui::Frame;

use std::thread;

use ethers_providers::{JsonRpcClient, Middleware, Provider, Ws};
use log::debug;
use std::error::Error;
use termion::event::Key;
use termion::input::TermRead;

use std::collections::{HashMap, VecDeque};

use ethers_core::types::Block as EthBlock;
use ethers_core::types::Transaction;
use ethers_core::types::TxHash;

use std::sync::mpsc;
use tokio::sync::mpsc as tokio_mpsc;

use signal_hook::consts::signal::*;
use signal_hook::iterator::Signals;

use chrono::{DateTime, NaiveDateTime, Utc};
use std::convert::TryInto;

enum UIMessage {
    // the user has given us some input over stdin
    Key(termion::event::Key),

    // something in the background has updated state and wants the UI to rerender
    Refresh(),

    // networking has noticed a new block and wants the UI to show it
    // TODO(2021-09-09) we really only need the block number
    NewBlock(EthBlock<TxHash>),
}

enum BlockFetch<T> {
    Waiting(u32),
    Started(u32),
    Completed(EthBlock<T>),
    // Failed(io::Error),
}

// TODO: is there a way to drop this redundency?
type ArcFetch = Arc<Mutex<BlockFetch<TxHash>>>;
type ArcFetchTxns = Arc<Mutex<BlockFetch<Transaction>>>;

// TODO: I think these layers are in the wrong order
enum NetworkRequest {
    Block(ArcFetch),
    BlockWithTxns(ArcFetchTxns),
}

struct Column<T> {
    name: &'static str,
    width: usize,
    enabled: bool,
    render: Box<dyn Fn(&T) -> String>,
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
            enabled: true,
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
    // TODO(2021-09-09) also include the block timestamp
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
            name: "block time (UTC)",
            width: 19,
            render: Box::new(|block| {
                let timestamp = block.timestamp;
                let low64 = timestamp.as_u64(); // TODO: panics if too big
                let low64signed = low64.try_into().unwrap(); // TODO: panic
                let naive_time = NaiveDateTime::from_timestamp(low64signed, 0);
                let time = DateTime::<Utc>::from_utc(naive_time, Utc);
                time.format("%Y-%m-%d %H:%M:%S").to_string()
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
            render: Box::new(|block| {
                let base_fee = block.base_fee_per_gas.expect("block has no base fee");
                util::humanize_u256(base_fee)
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

fn render_block(columns: &Vec<Column<EthBlock<TxHash>>>, block: &EthBlock<TxHash>) -> String {
    columns
        .iter()
        .filter(|col| col.enabled)
        .fold(String::new(), |mut accum, column| {
            if accum.len() != 0 {
                accum.push_str(" ");
            }
            let rendered = (column.render)(block);
            let filled = format!("{:>width$}", rendered, width = column.width);

            accum.push_str(&filled);
            accum
        })
}

fn columns_to_list_items<T>(columns: &Vec<Column<T>>) -> Vec<ListItem<'static>> {
    columns
        .iter()
        .fold((Vec::new(), 0), |(mut result, count), col| {
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

fn block_to_txn_list_items<'a>(
    columns: &Vec<Column<Transaction>>,
    block: &'a EthBlock<Transaction>,
) -> Vec<ListItem<'static>> {
    let txn_lines: Vec<ListItem> = block
        .transactions
        .iter()
        .map(|txn| {
            let formatted = columns.iter().filter(|col| col.enabled).fold(
                String::new(),
                |mut accum, column| {
                    if accum.len() != 0 {
                        accum.push_str(" ");
                    }
                    let rendered = (column.render)(txn);
                    let filled = format!("{:>width$}", rendered, width = column.width);

                    accum.push_str(&filled);
                    accum
                },
            );
            ListItem::new(Span::raw(formatted))
        })
        .collect();

    if txn_lines.len() == 0 {
        vec![ListItem::new(Span::raw("this block has no transactions"))]
    } else {
        txn_lines
    }
}

#[derive(Copy, Clone)]
enum FocusedPane {
    Blocks(),
    Transactions(),
}

impl FocusedPane {
    fn toggle(self) -> FocusedPane {
        use FocusedPane::*;
        match self {
            Blocks() => Transactions(),
            Transactions() => Blocks(),
        }
    }

    const FOCUSED_BORDER: Color = Color::Gray;
    const UNFOCUSED_BORDER: Color = Color::DarkGray;

    fn blocks_border_color(&self) -> Color {
        use FocusedPane::*;
        match self {
            Blocks() => FocusedPane::FOCUSED_BORDER,
            Transactions() => FocusedPane::UNFOCUSED_BORDER,
        }
    }

    fn txns_border_color(&self) -> Color {
        use FocusedPane::*;
        match self {
            Blocks() => FocusedPane::UNFOCUSED_BORDER,
            Transactions() => FocusedPane::FOCUSED_BORDER,
        }
    }

    const FOCUSED_SELECTION: Color = Color::LightGreen;
    const UNFOCUSED_SELECTION: Color = Color::Green;

    fn blocks_selection_color(&self) -> Color {
        use FocusedPane::*;
        match self {
            Blocks() => FocusedPane::FOCUSED_SELECTION,
            Transactions() => FocusedPane::UNFOCUSED_SELECTION,
        }
    }

    fn txns_selection_color(&self) -> Color {
        // TODO: I'm missing an abstraction which removes this tedium
        use FocusedPane::*;
        match self {
            Blocks() => FocusedPane::UNFOCUSED_SELECTION,
            Transactions() => FocusedPane::FOCUSED_SELECTION,
        }
    }
}

const BLOCK_LIST_BORDER_HEIGHT: usize = 3;

struct TUI {
    /* UI state */
    block_list_state: ListState,
    block_list_height: Option<usize>,

    column_list_state: ListState,

    txn_list_state: ListState,
    txn_list_length: Option<usize>,

    configuring_columns: bool,
    showing_transactions: bool,
    focused_pane: FocusedPane,

    txn_columns: Vec<Column<Transaction>>,
    txn_column_len: usize,

    columns: Vec<Column<EthBlock<TxHash>>>,
    column_items_len: usize,

    /* shared state */
    blocks: VecDeque<ArcFetch>,

    // TODO(2021-09-10) currently this leaks memory, use an lru cache or something
    blocks_to_txns: HashMap<u32, ArcFetchTxns>,
    highest_block: Arc<Mutex<Option<u32>>>,
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

impl TUI {
    fn new() -> TUI {
        let txn_columns = default_txn_columns();
        let txn_column_len = txn_columns.len();

        let columns = default_columns();
        let column_items_len = columns.len();

        TUI {
            block_list_state: ListState::default(),
            block_list_height: None,

            column_list_state: list_state_with_selection(Some(0)),

            txn_list_state: ListState::default(),
            txn_list_length: None,

            configuring_columns: false,
            showing_transactions: false,
            focused_pane: FocusedPane::Blocks(),

            txn_columns: txn_columns,
            txn_column_len: txn_column_len,

            columns: columns,
            column_items_len: column_items_len,

            blocks: VecDeque::new(),
            blocks_to_txns: HashMap::new(),
            highest_block: Arc::new(Mutex::new(None)),
        }
    }

    fn column_count(&self) -> usize {
        match self.focused_pane {
            FocusedPane::Blocks() => self.column_items_len,
            FocusedPane::Transactions() => self.txn_column_len,
        }
    }

    fn toggle_configuring_columns(&mut self) {
        // this intentionally does not reset column_list_state
        match self.configuring_columns {
            true => self.configuring_columns = false,
            false => self.configuring_columns = true,
        };
    }

    fn toggle_showing_transactions(&mut self) {
        match self.showing_transactions {
            true => {
                self.showing_transactions = false;
                self.focused_pane = FocusedPane::Blocks();
            }
            false => {
                self.showing_transactions = true;
                self.focused_pane = FocusedPane::Transactions();
            }
        };
    }

    fn handle_key_up(&mut self) {
        match self.configuring_columns {
            false => match self.focused_pane {
                FocusedPane::Blocks() => {
                    let item_count = self
                        .block_list_height
                        .unwrap_or(0)
                        .saturating_sub(BLOCK_LIST_BORDER_HEIGHT);
                    scroll_up_one(&mut self.block_list_state, item_count);

                    // it doesn't make sense to persist this if we're looking at
                    // txns for a new block. In the far future maybe this should
                    // track per-block scroll state?
                    self.txn_list_state.select(None);
                }
                FocusedPane::Transactions() => {
                    let item_count = self.txn_list_length.unwrap_or(0);
                    scroll_up_one(&mut self.txn_list_state, item_count);
                }
            },
            true => {
                let item_count = self.column_count();
                scroll_up_one(&mut self.column_list_state, item_count);
            }
        };
    }

    fn handle_key_down(&mut self) {
        match self.configuring_columns {
            true => {
                let col_count = self.column_count();
                scroll_down_one(&mut self.column_list_state, col_count);
            }
            false => match self.focused_pane {
                FocusedPane::Blocks() => {
                    let item_count = self
                        .block_list_height
                        .unwrap_or(0)
                        .saturating_sub(BLOCK_LIST_BORDER_HEIGHT);
                    scroll_down_one(&mut self.block_list_state, item_count);

                    // it doesn't make sense to persist this if we're looking at txns
                    // for a new block.
                    // TODO(bug): techincally we should not throw away the state if the
                    // selection did not change, such as if there is only one block in
                    // the list
                    self.txn_list_state.select(None);
                }
                FocusedPane::Transactions() => {
                    let item_count = self.txn_list_length.unwrap_or(0);
                    scroll_down_one(&mut self.txn_list_state, item_count);
                }
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
                    match self.focused_pane {
                        FocusedPane::Blocks() => {
                            self.columns[i].enabled = !self.columns[i].enabled;
                        }
                        FocusedPane::Transactions() => {
                            self.txn_columns[i].enabled = !self.txn_columns[i].enabled;
                        }
                    };
                }
            }
        };
    }

    fn handle_tab(&mut self, _forward: bool) {
        if self.showing_transactions {
            // TODO: this turns into a copy? why?
            self.focused_pane = self.focused_pane.toggle();
        }
    }

    fn handle_new_block(&mut self, block: EthBlock<TxHash>, block_fetcher: &Networking) {
        debug!("UI received new block {}", block.number.unwrap());

        // we cannot add this block directly because it is missing a bunch of
        // fields that we would like to render so instead we add a placeholder and
        // ask the networking thread to give us a better block
        // let new_fetch = Arc::new(Mutex::new(BlockFetch::Completed(block)));

        // TODO(2021-09-09) this is not necessarily a brand new block
        //                  there could have been a reorg, and it's possible
        //                  this block is replacing a previous one. we should
        //                  insert this block fetch into the correct location
        // TODO(2021-09-10) we should also update blocks_to_txns when we detect a
        //                  reorg
        let block_num = block.number.unwrap().low_u32();
        let new_fetch = block_fetcher.fetch_block(block_num);
        self.blocks.push_front(new_fetch);

        // TODO(2021-09-11): I think a lot of this logic will become easier if the
        //                   selection were stored as a highlighted block number
        //                   rather than an offset

        // when a new block comes in we want the same block to remain selected,
        // unless we're already at the end of the list
        match self.block_list_state.selected() {
            None => {} // there is no selection to update
            Some(i) => {
                if let Some(height) = self.block_list_height {
                    // if there is a populated block list to scroll (there is)
                    if i < (height - BLOCK_LIST_BORDER_HEIGHT).into() {
                        // if we're not already at the end of the list
                        self.block_list_state.select(Some(i + 1));
                    } else {
                        // the selected block has changed
                        self.txn_list_state.select(None);
                    }
                }
            }
        };

        {
            let mut highest_block_number_opt = self.highest_block.lock().unwrap();
            if let Some(highest_block_number) = *highest_block_number_opt {
                if block_num > highest_block_number {
                    *highest_block_number_opt = Some(block_num);
                }
            }
        }
    }

    fn draw_popup<B: Backend>(&mut self, frame: &mut Frame<B>) {
        let column_items: Vec<ListItem> = match self.focused_pane {
            FocusedPane::Blocks() => columns_to_list_items(&self.columns),
            FocusedPane::Transactions() => columns_to_list_items(&self.txn_columns),
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

    fn draw_block_list<B: Backend>(
        &mut self,
        frame: &mut Frame<B>,
        area: Rect,
        block_fetcher: &Networking,
    ) {
        let target_height = area.height;
        let target_height = {
            // the border consumes 2 lines
            if target_height > 2 {
                target_height - 2
            } else {
                target_height
            }
        };

        // the size we will give the block list widget
        while target_height > self.blocks.len() as u16 {
            let highest_block_number = {
                let block_number_opt = self.highest_block.lock().unwrap();
                block_number_opt.unwrap()
            };
            let new_fetch = block_fetcher.fetch_block(
                // TODO: if the chain is very young this could underflow
                highest_block_number - self.blocks.len() as u32,
            );
            self.blocks.push_back(new_fetch);
        }

        let header = columns_to_header(&self.columns);

        let block_lines = {
            let block_lines: Vec<ListItem> = self
                .blocks
                .iter()
                .map(|arcfetch| {
                    let fetch = arcfetch.lock().unwrap();

                    use BlockFetch::*;
                    let formatted = match &*fetch {
                        Waiting(height) => format!("{} waiting", height),
                        Started(height) => format!("{} fetching", height),
                        Completed(block) => render_block(&self.columns, block),
                    };
                    ListItem::new(Span::raw(formatted))
                })
                .collect();
            block_lines
        };

        let block_list = HeaderList::new(block_lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.focused_pane.blocks_border_color()))
                    .title("Blocks"),
            )
            .highlight_style(Style::default().bg(self.focused_pane.blocks_selection_color()))
            .header(header);
        frame.render_stateful_widget(block_list, area, &mut self.block_list_state);

        // TODO: will this ever panic? u16 into a usize
        self.block_list_height = Some(area.height.into());
    }

    fn draw_txn_list<B: Backend>(
        &mut self,
        frame: &mut Frame<B>,
        area: Rect,
        block_fetcher: &Networking,
    ) {
        let txn_items = if let Some(offset) = self.block_list_state.selected() {
            let highest_block_number = {
                let block_number_opt = self.highest_block.lock().unwrap();
                block_number_opt.unwrap()
            };
            let block_at_offset = highest_block_number - (offset as u32);

            let arcfetch = match self.blocks_to_txns.get(&block_at_offset) {
                None => {
                    let new_fetch = block_fetcher.fetch_block_with_txns(block_at_offset);
                    debug!("fired new request for block {}", block_at_offset);
                    self.blocks_to_txns.insert(block_at_offset, new_fetch);

                    self.blocks_to_txns.get(&block_at_offset).unwrap()
                }
                Some(arcfetch) => arcfetch,
            };

            {
                let mut fetch = arcfetch.lock().unwrap();

                // TODO: call block_to_txn_list_items when not inside a critical
                // section. This is low priority, because by the time we're calling
                // block_to_txn_list_items we're the only thread trying to
                // access this block, we only have it because the networking
                // thread has finished.

                self.txn_list_length = None;
                use BlockFetch::*;
                match &mut *fetch {
                    Waiting(height) => {
                        vec![ListItem::new(Span::raw(format!("{} waiting", height)))]
                    }
                    Started(height) => {
                        vec![ListItem::new(Span::raw(format!("{} fetching", height)))]
                    }
                    Completed(block) => {
                        self.txn_list_length = Some(block.transactions.len());
                        block_to_txn_list_items(&self.txn_columns, &block)
                    }
                }
            }
        } else {
            Vec::new()
        };

        let txn_list = HeaderList::new(txn_items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.focused_pane.txns_border_color()))
                    .title("Transactions"),
            )
            .highlight_style(Style::default().bg(self.focused_pane.txns_selection_color()))
            .header(columns_to_header(&self.txn_columns));
        frame.render_stateful_widget(txn_list, area, &mut self.txn_list_state);
    }

    fn draw<B: Backend>(&mut self, frame: &mut Frame<B>, block_fetcher: &Networking) {
        let waiting_for_initial_block = {
            let block_number_opt = self.highest_block.lock().unwrap();

            if let Some(_) = *block_number_opt {
                false
            } else {
                true
            }
        };

        if waiting_for_initial_block {
            frame.render_widget(
                Paragraph::new(Span::raw("fetching current block number")),
                frame.size(),
            );
        } else {
            let vert_chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([Constraint::Min(0), Constraint::Length(2)].as_ref())
                .split(frame.size());

            let horiz_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(vert_chunks[0]);

            let block_list_chunk = if self.showing_transactions {
                horiz_chunks[0]
            } else {
                vert_chunks[0]
            };

            self.draw_block_list(frame, block_list_chunk, block_fetcher);

            if self.showing_transactions {
                self.draw_txn_list(frame, horiz_chunks[1], block_fetcher);
            }

            let bold_title =
                Span::styled("turtlescan", Style::default().add_modifier(Modifier::BOLD));

            // TODO: if both panes are active show (Tab) focus {the other pane}

            let status_string = match self.configuring_columns {
                false => "  (q) quit - (c) configure columns - (t) toggle transactions view",
                true => "  (c) close col popup - (space) toggle column - (↑/↓) choose column",
            };

            let status_line =
                Paragraph::new(status_string).block(Block::default().title(bold_title));
            frame.render_widget(status_line, vert_chunks[1]);

            if self.configuring_columns {
                self.draw_popup(frame);
            }
        }
    }
}

// TODO(2021-08-27) why does the following line not work?
// fn run_tui() -> Result<(), Box<io::Error>> {
pub fn run_tui(provider: Provider<Ws>) -> Result<(), Box<dyn Error>> {
    let stdout = io::stdout().into_raw_mode()?;
    let stdout = AlternateScreen::from(stdout);
    let backend = TermionBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (tx, rx) = mpsc::channel(); // tell the UI thread (this one) what to do

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

    let mut tui = TUI::new();

    // let's do some networking in the background
    // no real need to hold onto this handle, the thread will be killed when this main
    // thread exits.
    let highest_block_clone = tui.highest_block.clone();
    let block_fetcher = Networking::start(provider, highest_block_clone, tx);

    loop {
        terminal.draw(|mut f| {
            tui.draw(&mut f, &block_fetcher);
        })?;

        let input = rx.recv().unwrap()?; // blocks until we have more input

        match input {
            UIMessage::Key(key) => match key {
                Key::Char('q') | Key::Esc | Key::Ctrl('c') => break,
                Key::Char('c') => tui.toggle_configuring_columns(),
                Key::Char('t') => tui.toggle_showing_transactions(),
                Key::Up => tui.handle_key_up(),
                Key::Down => tui.handle_key_down(),
                Key::Char(' ') => tui.handle_key_space(),
                Key::Char('\t') => tui.handle_tab(true),
                Key::BackTab => tui.handle_tab(false),
                key => {
                    debug!("unhandled key press: {:?}", key)
                }
            },
            UIMessage::Refresh() => {}
            UIMessage::NewBlock(block) => tui.handle_new_block(block, &block_fetcher),
        }
    }

    Ok(())
}

struct Networking {
    _bg_thread: thread::JoinHandle<()>,
    network_tx: tokio_mpsc::UnboundedSender<NetworkRequest>, // tell network what to fetch
                                                             // network_rx: mpsc::Receiver<ArcFetch>,
}

impl Networking {
    fn start(
        provider: Provider<Ws>, // Ws is required because we watch for new blocks
        highest_block: Arc<Mutex<Option<u32>>>,
        tx: mpsc::Sender<Result<UIMessage, io::Error>>,
    ) -> Networking {
        let (network_tx, mut network_rx) = tokio_mpsc::unbounded_channel();

        let handle = thread::spawn(move || {
            run_networking(provider, highest_block, tx, &mut network_rx);
        });

        Networking {
            _bg_thread: handle,
            network_tx: network_tx,
            // network_rx: network_rx,
        }
    }

    // TODO: return error
    fn fetch_block(&self, block_number: u32) -> ArcFetch {
        let new_fetch = Arc::new(Mutex::new(BlockFetch::Waiting(block_number)));

        let sent_fetch = new_fetch.clone();

        if let Err(_) = self.network_tx.send(NetworkRequest::Block(sent_fetch)) {
            // TODO(2021-09-09): fetch_block() should return a Result
            // Can't use expect() or unwrap() b/c SendError does not implement Debug
            panic!("remote end closed?");
        }

        new_fetch
    }

    fn fetch_block_with_txns(&self, block_number: u32) -> ArcFetchTxns {
        let new_fetch = Arc::new(Mutex::new(BlockFetch::Waiting(block_number)));

        let sent_fetch = new_fetch.clone();

        if let Err(_) = self
            .network_tx
            .send(NetworkRequest::BlockWithTxns(sent_fetch))
        {
            // TODO(2021-09-09): fetch_block() should return a Result
            // Can't use expect() or unwrap() b/c SendError does not implement Debug
            panic!("remote end closed?");
        }

        new_fetch
    }
}

#[tokio::main(worker_threads = 1)]
async fn run_networking(
    /*
     * Ws is required because we watch for new blocks
     * TODO(2021-09-14) document this limitation somewhere visible
     */
    provider: Provider<Ws>,
    highest_block: Arc<Mutex<Option<u32>>>,
    tx: mpsc::Sender<Result<UIMessage, io::Error>>,
    network_rx: &mut tokio_mpsc::UnboundedReceiver<NetworkRequest>,
) {
    debug!("started networking thread");
    let block_number_opt = provider.get_block_number().await;
    match block_number_opt {
        Err(error) => debug!("{:}", error),
        Ok(number) => {
            let mut block_number = highest_block.lock().unwrap();
            *block_number = Some(number.low_u32());
        }
    }
    tx.send(Ok(UIMessage::Refresh())).unwrap();
    debug!("updated block number");

    let loop_tx = tx.clone();
    let loop_fut = loop_on_network_commands(&provider, loop_tx, network_rx);

    let watch_fut = watch_new_blocks(&provider, tx);

    tokio::join!(loop_fut, watch_fut); // neither will exit so this should block forever
}

async fn loop_on_network_commands<T: JsonRpcClient>(
    provider: &Provider<T>,
    tx: mpsc::Sender<Result<UIMessage, io::Error>>,
    network_rx: &mut tokio_mpsc::UnboundedReceiver<NetworkRequest>,
) {
    loop {
        let request = network_rx.recv().await.unwrap(); // blocks until we have more input

        // TODO(2021-09-10): some gnarly logic duplication
        match request {
            NetworkRequest::Block(arc_fetch) => {
                let block_number = {
                    // we're blocking the thread but these critical sections are kept as short as
                    // possible (here and elsewhere in the program)
                    let mut fetch = arc_fetch.lock().unwrap();

                    if let BlockFetch::Waiting(block_number) = *fetch {
                        // tell the UI we're handling this fetch
                        *fetch = BlockFetch::Started(block_number);

                        block_number
                    } else {
                        continue;
                    }
                } as u64;
                tx.send(Ok(UIMessage::Refresh())).unwrap();

                let complete_block = provider.get_block(block_number).await.unwrap().unwrap();

                {
                    let mut fetch = arc_fetch.lock().unwrap();
                    *fetch = BlockFetch::Completed(complete_block);
                }
                tx.send(Ok(UIMessage::Refresh())).unwrap();
            }
            NetworkRequest::BlockWithTxns(arc_fetch) => {
                let block_number = {
                    let mut fetch = arc_fetch.lock().unwrap();

                    if let BlockFetch::Waiting(block_number) = *fetch {
                        *fetch = BlockFetch::Started(block_number);

                        block_number
                    } else {
                        continue;
                    }
                } as u64;
                tx.send(Ok(UIMessage::Refresh())).unwrap();

                // the first unwrap is b/c the network request might have failed
                // the second unwrap is b/c the requested block number might not exist
                let complete_block = provider
                    .get_block_with_txs(block_number)
                    .await
                    .unwrap()
                    .unwrap();

                {
                    let mut fetch = arc_fetch.lock().unwrap();
                    *fetch = BlockFetch::Completed(complete_block);
                }
                tx.send(Ok(UIMessage::Refresh())).unwrap();
            }
        }
    }
}

use ethers_providers::StreamExt;

async fn watch_new_blocks(provider: &Provider<Ws>, tx: mpsc::Sender<Result<UIMessage, io::Error>>) {
    let mut stream = provider.subscribe_blocks().await.unwrap();
    while let Some(block) = stream.next().await {
        debug!("new block {}", block.number.unwrap());
        tx.send(Ok(UIMessage::NewBlock(block))).unwrap();
    }
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

fn columns_to_header<T>(columns: &Vec<Column<T>>) -> Spans {
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

use tui::buffer::Buffer;

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
