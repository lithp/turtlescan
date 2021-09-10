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

use std::collections::VecDeque;

use ethers_core::types::Block as EthBlock;
use ethers_core::types::TxHash;

use std::sync::mpsc;
use tokio::sync::mpsc as tokio_mpsc;

use signal_hook::consts::signal::*;
use signal_hook::iterator::Signals;

/*
 * This has the promise to be a pretty cool TUI but at the moment it's just a demo.
 *
 * Currently it is a very complicated and unintuitive way to fetch the current block
 * number from infura.
 */

enum UIMessage {
    // the user has given us some input over stdin
    Key(termion::event::Key),

    // something in the background has updated state and wants the UI to rerender
    Refresh(),

    // networking has noticed a new block and wants the UI to show it
    // TODO(2021-09-09) we really only need the block number
    NewBlock(EthBlock<TxHash>),
}

enum BlockFetch {
    Waiting(u32),
    Started(u32),
    Completed(EthBlock<TxHash>),
    // Failed(io::Error),
}

type ArcFetch = Arc<Mutex<BlockFetch>>;

struct Column {
    name: &'static str,
    width: usize,
    enabled: bool,
    render: Box<dyn Fn(&EthBlock<TxHash>) -> String>,
}

fn default_columns() -> Vec<Column> {
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

fn render_block(columns: &Vec<Column>, block: &EthBlock<TxHash>) -> String {
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

    let highest_block: Arc<Mutex<Option<u32>>> = Arc::new(Mutex::new(None));

    let mut block_list_state = ListState::default();
    let mut column_list_state = ListState::default();

    let mut configuring_columns: bool = false;

    let mut columns = default_columns();
    let column_items_len = columns.len();

    // let's do some networking in the background
    // no real need to hold onto this handle, the thread will be killed when this main
    // thread exits.
    let highest_block_clone = highest_block.clone();
    let block_fetcher = BlockFetcher::start(provider, highest_block_clone, tx);

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
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .margin(1)
                    .constraints([Constraint::Min(0), Constraint::Length(2)].as_ref())
                    .split(f.size());

                let target_height = chunks[0].height;
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
                    let new_fetch = block_fetcher.fetch(
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
                                // TODO: format using our column render methods
                                // Completed(block) => util::format_block(block),
                                Completed(block) => render_block(&columns, block),
                                // Failed(_) => "failed".to_string(),
                            };
                            ListItem::new(Span::raw(formatted))
                        })
                        .collect();

                    res.append(&mut block_lines);
                    res
                };

                let block_list = List::new(block_lines)
                    .block(Block::default().borders(Borders::ALL).title("Blocks"))
                    .highlight_style(Style::default().bg(Color::LightGreen));
                f.render_stateful_widget(block_list, chunks[0], &mut block_list_state);

                let bold_title =
                    Span::styled("turtlescan", Style::default().add_modifier(Modifier::BOLD));

                let status_string = match configuring_columns {
                    false => "  (q) quit - (c) configure columns",
                    true => "  (c) close col popup - (space) toggle column - (↑/↓) choose column",
                };

                let status_line =
                    Paragraph::new(status_string).block(Block::default().title(bold_title));
                f.render_widget(status_line, chunks[1]);

                if configuring_columns {
                    let column_items: Vec<ListItem> = columns
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
                        .0;

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
                Key::Char('q') => break,
                Key::Char('c') => match configuring_columns {
                    // this intentionally does not reset column_list_state
                    true => configuring_columns = false,
                    false => configuring_columns = true,
                },
                Key::Up => match configuring_columns {
                    false => (),
                    true => match column_list_state.selected() {
                        None => {
                            column_list_state.select(Some(column_items_len - 1));
                        }
                        Some(0) => {
                            column_list_state.select(Some(column_items_len - 1));
                        }
                        Some(i) => {
                            column_list_state.select(Some(i - 1));
                        }
                    },
                },
                Key::Down => match configuring_columns {
                    false => (),
                    true => match column_list_state.selected() {
                        None => {
                            column_list_state.select(Some(0));
                        }
                        Some(i) => {
                            if i >= column_items_len - 1 {
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
                            columns[i].enabled = !columns[i].enabled;
                        }
                    }
                },
                _ => {}
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
                let new_fetch = block_fetcher.fetch(block.number.unwrap().low_u32());
                blocks.push_front(new_fetch);
            }
        }
    }

    Ok(())
}

struct BlockFetcher {
    _bg_thread: thread::JoinHandle<()>,
    network_tx: tokio_mpsc::UnboundedSender<ArcFetch>, // tell network what to fetch
                                                       // network_rx: mpsc::Receiver<ArcFetch>,
}

impl BlockFetcher {
    fn start(
        provider: Provider<Ws>, // Ws is required because we watch for new blocks
        highest_block: Arc<Mutex<Option<u32>>>,
        tx: mpsc::Sender<Result<UIMessage, io::Error>>,
    ) -> BlockFetcher {
        let (network_tx, mut network_rx) = tokio_mpsc::unbounded_channel();

        let handle = thread::spawn(move || {
            run_networking(provider, highest_block, tx, &mut network_rx);
        });

        BlockFetcher {
            _bg_thread: handle,
            network_tx: network_tx,
            // network_rx: network_rx,
        }
    }

    // TODO: return error
    fn fetch(&self, block_number: u32) -> ArcFetch {
        let new_fetch = Arc::new(Mutex::new(BlockFetch::Waiting(block_number)));

        let sent_fetch = new_fetch.clone();

        if let Err(_) = self.network_tx.send(sent_fetch) {
            // TODO(2021-09-09): fetch() should return a Result
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
    network_rx: &mut tokio_mpsc::UnboundedReceiver<ArcFetch>,
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
    network_rx: &mut tokio_mpsc::UnboundedReceiver<ArcFetch>,
) {
    loop {
        let arc_fetch = network_rx.recv().await.unwrap(); // blocks until we have more input

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
