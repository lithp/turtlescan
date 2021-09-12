use crate::util;

use std::cmp;
use std::io;
use std::sync::{Arc, Mutex};
use termion::raw::IntoRawMode;
use termion::screen::AlternateScreen;
// use tokio::time::sleep as async_sleep;
use tui::backend::TermionBackend;
use tui::text::{Span, Spans};
use tui::Terminal;

use tui::layout::{Constraint, Direction, Layout, Rect};
use tui::style::{Color, Modifier, Style};
use tui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph /*Wrap*/};

use std::thread;
// use std::time;

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
            name: "blk hash",
            width: 12,
            render: Box::new(|block| match block.hash {
                Some(hash) => util::format_block_hash(hash.as_bytes()),
                None => "unknown".to_string(),
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
    // add a header?

    // TODO: code duplication
    let underline_style = Style::default().add_modifier(Modifier::UNDERLINED);
    let spans = Spans::from(columns.iter().filter(|col| col.enabled).fold(
        Vec::new(),
        |mut accum, column| {
            // soon rust iterators will have an intersperse method
            if accum.len() != 0 {
                accum.push(Span::raw(" "));
            }
            let filled = format!("{:<width$}", column.name, width = column.width);
            accum.push(Span::styled(filled, underline_style));
            accum
        },
    ));
    let header = ListItem::new(spans);

    let mut res = vec![header];

    let mut txn_lines: Vec<ListItem> = block
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
        res.push(ListItem::new(Span::raw("this block has no transactions")))
    }

    res.append(&mut txn_lines);

    res
}

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

    // if we need it: prevents us from blocking when we check for more input
    // let mut stdin = termion::async_stdin().keys();

    let mut blocks: VecDeque<ArcFetch> = VecDeque::new();

    // TODO(2021-09-10) currently this leaks memory, use an lru cache or something
    let mut blocks_to_txns: HashMap<u32, ArcFetchTxns> = HashMap::new();

    let highest_block: Arc<Mutex<Option<u32>>> = Arc::new(Mutex::new(None));

    let mut block_list_state = ListState::default();
    let mut block_list_height: Option<u16> = None;

    let mut column_list_state = ListState::default();

    // TODO(2021-09-11) I've written this scroll code three times now and it's finally
    //                  becoming tedious, this needs to be pulled out into a struct
    let mut txn_list_state = ListState::default();
    let mut txn_list_length: Option<usize> = None;

    let mut configuring_columns: bool = false;
    let mut showing_transactions: bool = false;

    let mut txn_columns = default_txn_columns();
    let txn_column_len = txn_columns.len();

    let mut columns = default_columns();
    let column_items_len = columns.len();

    let mut focused_pane = FocusedPane::Blocks();

    let column_count = |focus: &FocusedPane| match focus {
        FocusedPane::Blocks() => column_items_len,
        FocusedPane::Transactions() => txn_column_len,
    };

    // let's do some networking in the background
    // no real need to hold onto this handle, the thread will be killed when this main
    // thread exits.
    let highest_block_clone = highest_block.clone();
    let block_fetcher = Networking::start(provider, highest_block_clone, tx);

    loop {
        let waiting_for_initial_block = {
            let block_number_opt = highest_block.lock().unwrap();

            if let Some(_) = *block_number_opt {
                false
            } else {
                true
            }
        };

        if waiting_for_initial_block {
            terminal.draw(|f| {
                f.render_widget(
                    Paragraph::new(Span::raw("fetching current block number")),
                    f.size(),
                );
            })?;
        } else {
            terminal.draw(|f| {
                let vert_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .margin(1)
                    .constraints([Constraint::Min(0), Constraint::Length(2)].as_ref())
                    .split(f.size());

                let horiz_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .split(vert_chunks[0]);

                let target_height = vert_chunks[0].height;
                let target_height = {
                    // the border consumes 2 lines
                    // TODO: this really should be 3, we also need a header to label the
                    //       columns
                    if target_height > 2 {
                        target_height - 2
                    } else {
                        target_height
                    }
                };

                // the size we will give the block list widget
                while target_height > blocks.len() as u16 {
                    let highest_block_number = {
                        let block_number_opt = highest_block.lock().unwrap();
                        block_number_opt.unwrap()
                    };
                    let new_fetch = block_fetcher.fetch_block(
                        // TODO: if the chain is very young this could underflow
                        highest_block_number - blocks.len() as u32,
                    );
                    blocks.push_back(new_fetch);
                }

                let underline_style = Style::default().add_modifier(Modifier::UNDERLINED);
                let spans = Spans::from(columns.iter().filter(|col| col.enabled).fold(
                    Vec::new(),
                    |mut accum, column| {
                        // soon rust iterators will have an intersperse method
                        if accum.len() != 0 {
                            accum.push(Span::raw(" "));
                        }
                        let filled = format!("{:<width$}", column.name, width = column.width);
                        accum.push(Span::styled(filled, underline_style));
                        accum
                    },
                ));
                let header = ListItem::new(spans);

                let block_lines = {
                    let mut res = vec![header];

                    let mut block_lines: Vec<ListItem> = blocks
                        .iter()
                        .map(|arcfetch| {
                            let fetch = arcfetch.lock().unwrap();

                            use BlockFetch::*;
                            let formatted = match &*fetch {
                                Waiting(height) => format!("{} waiting", height),
                                Started(height) => format!("{} fetching", height),
                                Completed(block) => render_block(&columns, block),
                            };
                            ListItem::new(Span::raw(formatted))
                        })
                        .collect();

                    res.append(&mut block_lines);
                    res
                };

                let block_list_chunk = if showing_transactions {
                    horiz_chunks[0]
                } else {
                    vert_chunks[0]
                };

                let block_list = List::new(block_lines)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(Style::default().fg(focused_pane.blocks_border_color()))
                            .title("Blocks"),
                    )
                    .highlight_style(Style::default().bg(focused_pane.blocks_selection_color()));
                f.render_stateful_widget(block_list, block_list_chunk, &mut block_list_state);
                block_list_height = Some(vert_chunks[0].height);

                if showing_transactions {
                    let txn_items = if let Some(offset) = block_list_state.selected() {
                        let offset = offset - 1; // skip the header
                        let highest_block_number = {
                            let block_number_opt = highest_block.lock().unwrap();
                            block_number_opt.unwrap()
                        };
                        let block_at_offset = highest_block_number - (offset as u32);

                        let arcfetch = match blocks_to_txns.get(&block_at_offset) {
                            None => {
                                let new_fetch =
                                    block_fetcher.fetch_block_with_txns(block_at_offset);
                                debug!("fired new request for block {}", block_at_offset);
                                blocks_to_txns.insert(block_at_offset, new_fetch);

                                blocks_to_txns.get(&block_at_offset).unwrap()
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

                            txn_list_length = None;
                            use BlockFetch::*;
                            match &mut *fetch {
                                Waiting(height) => {
                                    vec![ListItem::new(Span::raw(format!("{} waiting", height)))]
                                }
                                Started(height) => {
                                    vec![ListItem::new(Span::raw(format!("{} fetching", height)))]
                                }
                                Completed(block) => {
                                    txn_list_length = Some(block.transactions.len());
                                    block_to_txn_list_items(&txn_columns, &block)
                                }
                            }
                        }
                    } else {
                        Vec::new()
                    };

                    let txn_list = List::new(txn_items)
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .border_style(Style::default().fg(focused_pane.txns_border_color()))
                                .title("Transactions"),
                        )
                        // TODO: this should be txns_selection_color()
                        .highlight_style(Style::default().bg(focused_pane.txns_selection_color()));

                    // TODO(2021-09-11) the header disappears when we scroll past the end
                    f.render_stateful_widget(txn_list, horiz_chunks[1], &mut txn_list_state);
                }

                let bold_title =
                    Span::styled("turtlescan", Style::default().add_modifier(Modifier::BOLD));

                let status_string = match configuring_columns {
                    false => "  (q) quit - (c) configure columns - (t) toggle transactions view",
                    true => "  (c) close col popup - (space) toggle column - (↑/↓) choose column",
                };

                let status_line =
                    Paragraph::new(status_string).block(Block::default().title(bold_title));
                f.render_widget(status_line, vert_chunks[1]);

                if configuring_columns {
                    let column_items: Vec<ListItem> = match focused_pane {
                        FocusedPane::Blocks() => columns_to_list_items(&columns),
                        FocusedPane::Transactions() => columns_to_list_items(&txn_columns),
                    };

                    let popup = List::new(column_items.clone())
                        .block(Block::default().title("Columns").borders(Borders::ALL))
                        .highlight_style(Style::default().bg(Color::LightGreen));

                    let frame_size = f.size();
                    let (popup_height, popup_width) = (15, 30);
                    let area = centered_rect(frame_size, popup_height, popup_width);

                    f.render_widget(Clear, area);
                    f.render_stateful_widget(popup, area, &mut column_list_state);
                }
            })?;
        }

        let input = rx.recv().unwrap()?; // blocks until we have more input

        match input {
            UIMessage::Key(key) => match key {
                Key::Char('q') | Key::Esc | Key::Ctrl('c') => break,
                Key::Char('c') => match configuring_columns {
                    // this intentionally does not reset column_list_state
                    true => configuring_columns = false,
                    false => configuring_columns = true,
                },
                Key::Char('t') => match showing_transactions {
                    true => {
                        showing_transactions = false;
                        focused_pane = FocusedPane::Blocks();
                    }
                    false => {
                        showing_transactions = true;
                        focused_pane = FocusedPane::Transactions();
                    }
                },
                Key::Up => match configuring_columns {
                    false => match focused_pane {
                        FocusedPane::Blocks() => {
                            if let Some(height) = block_list_height {
                                match block_list_state.selected() {
                                    None => {
                                        block_list_state.select(Some((height - 3).into()));
                                    }
                                    Some(i) => {
                                        if i <= 1 {
                                            block_list_state.select(Some((height - 3).into()));
                                        } else {
                                            block_list_state.select(Some(i - 1));
                                        }
                                    }
                                }
                            }

                            // it doesn't make sense to persist this if we're looking at
                            // txns for a new block. In the far future maybe this should
                            // track per-block scroll state?
                            txn_list_state.select(None);
                        }
                        FocusedPane::Transactions() => {
                            match txn_list_state.selected() {
                                None => {
                                    match txn_list_length {
                                        None => {} // there is nothing to select
                                        Some(txn_count) => {
                                            // the last item
                                            txn_list_state.select(Some(txn_count));
                                        }
                                    }
                                }

                                Some(current_selection) => {
                                    match txn_list_length {
                                        None => {
                                            txn_list_state.select(None);
                                        }
                                        // TODO: I don't think this correctly handles the
                                        //       case where the list of txns has changed
                                        //       out from under us
                                        Some(txn_count) => {
                                            if current_selection <= 1 {
                                                txn_list_state.select(Some(txn_count));
                                            } else {
                                                txn_list_state.select(Some(current_selection - 1));
                                            }
                                        }
                                    }
                                }
                            };
                        }
                    },
                    true => match column_list_state.selected() {
                        None => {
                            column_list_state.select(Some(column_count(&focused_pane) - 1));
                        }
                        Some(0) => {
                            column_list_state.select(Some(column_count(&focused_pane) - 1));
                        }
                        Some(i) => {
                            column_list_state.select(Some(i - 1));
                        }
                    },
                },
                Key::Down => match configuring_columns {
                    false => match focused_pane {
                        FocusedPane::Blocks() => {
                            match block_list_state.selected() {
                                None => {
                                    // 0 is the header, we start selecting at 1
                                    block_list_state.select(Some(1));
                                }
                                Some(i) => {
                                    // NB. this duplicates logic found in  NewBlock(), any changes
                                    //     here likely also need to be applied there
                                    if let Some(height) = block_list_height {
                                        if i >= (height - 3).into() {
                                            block_list_state.select(Some(1));
                                        } else {
                                            block_list_state.select(Some(i + 1));
                                        }
                                    }
                                }
                            }

                            // it doesn't make sense to persist this if we're looking at txns
                            // for a new block
                            txn_list_state.select(None);
                        }
                        FocusedPane::Transactions() => {
                            // scroll down
                            match txn_list_state.selected() {
                                None => {
                                    match txn_list_length {
                                        None | Some(0) => {} // there is nothing to select
                                        Some(_txn_count) => {
                                            // 1 b/c the header is sitting at 0
                                            txn_list_state.select(Some(1));
                                        }
                                    }
                                }

                                Some(current_selection) => {
                                    match txn_list_length {
                                        None | Some(0) => {
                                            txn_list_state.select(None);
                                        }
                                        // TODO: I don't think this correctly handles the
                                        //       case where the list of txns has changed
                                        //       out from under us
                                        Some(txn_count) => {
                                            if current_selection >= txn_count {
                                                txn_list_state.select(Some(1));
                                            } else {
                                                txn_list_state.select(Some(current_selection + 1));
                                            }
                                        }
                                    }
                                }
                            };
                        }
                    },
                    true => match column_list_state.selected() {
                        None => {
                            column_list_state.select(Some(0));
                        }
                        Some(i) => {
                            if i >= column_count(&focused_pane) - 1 {
                                column_list_state.select(Some(0));
                            } else {
                                column_list_state.select(Some(i + 1));
                            }
                        }
                    },
                },
                Key::Char(' ') => match configuring_columns {
                    false => (),
                    true => {
                        if let Some(i) = column_list_state.selected() {
                            // TODO(2021-09-11) this entire codebase is ugly but this is
                            //                  especially gnarly
                            match focused_pane {
                                FocusedPane::Blocks() => {
                                    columns[i].enabled = !columns[i].enabled;
                                }
                                FocusedPane::Transactions() => {
                                    txn_columns[i].enabled = !txn_columns[i].enabled;
                                }
                            };
                        }
                    }
                },
                Key::Char('\t') | Key::BackTab => {
                    if showing_transactions {
                        focused_pane = focused_pane.toggle();
                    }
                }
                key => {
                    debug!("key hit: {:?}", key)
                }
            },
            UIMessage::Refresh() => {}
            UIMessage::NewBlock(block) => {
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
                blocks.push_front(new_fetch);

                // TODO(2021-09-11): I think a lot of this logic will become easier if the
                //                   selection were stored as a highlighted block number
                //                   rather than an offset

                // when a new block comes in we want the same block to remain selected,
                // unless we're already at the end of the list
                match block_list_state.selected() {
                    // NB. this duplicates logic found in Key::Down, any changes should
                    //      be made there as well
                    None => {} // there is no selection to update
                    Some(i) => {
                        if let Some(height) = block_list_height {
                            // if there is a populated block list to scroll (there is)
                            if i < (height - 3).into() {
                                // if we're not already at the end of the list
                                block_list_state.select(Some(i + 1));
                            } else {
                                // the selected block has changed
                                txn_list_state.select(None);
                            }
                        }
                    }
                };

                {
                    let mut highest_block_number_opt = highest_block.lock().unwrap();
                    if let Some(highest_block_number) = *highest_block_number_opt {
                        if block_num > highest_block_number {
                            *highest_block_number_opt = Some(block_num);
                        }
                    }
                }
            }
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
    provider: Provider<Ws>, // Ws is required because we watch for new blocks
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
