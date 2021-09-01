use crate::util;

use std::io;
use std::sync::{Arc, Mutex};
use termion::raw::IntoRawMode;
use termion::screen::AlternateScreen;
// use tokio::time::sleep as async_sleep;
use tui::backend::TermionBackend;
use tui::text::{Span, Spans};
use tui::Terminal;

use tui::layout::{Constraint, Direction, Layout};
use tui::style::{Color, Modifier, Style};
use tui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use std::thread;
// use std::time;

use ethers_providers::{JsonRpcClient, Middleware, Provider};
use log::debug;
use std::error::Error;
use termion::event::Key;
use termion::input::TermRead;

use std::collections::VecDeque;

use ethers_core::types::Block as EthBlock;
use ethers_core::types::TxHash;

use std::sync::mpsc;

use signal_hook::consts::signal::*;
use signal_hook::iterator::Signals;


/*
 * This has the promise to be a pretty cool TUI but at the moment it's just a demo.
 *
 * Currently it is a very complicated and unintuitive way to fetch the current block
 * number from infura.
 */

enum UIMessage {
    // a message sent to the UI
    Key(termion::event::Key),
    Refresh(),
}

enum BlockFetch {
    Waiting(u32),
    Started(u32),
    Completed(EthBlock<TxHash>),
    // Failed(io::Error),
}

type ArcFetch = Arc<Mutex<BlockFetch>>;

// TODO(2021-08-27) why does the following line not work?
// fn run_tui() -> Result<(), Box<io::Error>> {
pub fn run_tui<T: JsonRpcClient + 'static>(provider: Provider<T>) -> Result<(), Box<dyn Error>> {
    let stdout = io::stdout().into_raw_mode()?;
    let stdout = AlternateScreen::from(stdout);
    let backend = TermionBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (tx, rx) = mpsc::channel(); // tell the UI thread (this one) what to do

    let keys_tx = tx.clone();
    thread::spawn(move || {
        let stdin = io::stdin().keys();

        for key in stdin {
            let mapped = key.map(|k| UIMessage::Key(k));
            keys_tx.send(mapped).unwrap();
        }
    });

    let winch_tx = tx.clone();
    thread::spawn(move || {
        let mut signals = Signals::new(&[
            SIGWINCH,
        ]).unwrap();

        for _signal in signals.forever() {
            winch_tx.send(Ok(UIMessage::Refresh())).unwrap();
        }
    });

    // if we need it: prevents us from blocking when we check for more input
    // let mut stdin = termion::async_stdin().keys();

    let mut current_tab: usize = 0;

    let tab_count = 3;

    let mut blocks: VecDeque<ArcFetch> = VecDeque::new();

    let highest_block: Arc<Mutex<Option<u32>>> = Arc::new(Mutex::new(None));

    let mut block_list_state = ListState::default();

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

                let header = ListItem::new(Spans::from(vec![
                    Span::styled(" blk num", underline_style),
                    Span::raw(" "),
                    Span::styled("  blk hash  ", underline_style),
                    Span::raw(" "),
                    Span::styled(" parent hash", underline_style),
                    Span::raw(" "),
                    Span::styled("  coinbase  ", underline_style),
                    Span::raw(" "),
                    Span::styled(" gas used", underline_style),
                    Span::raw(" "),
                    Span::styled("gas limit", underline_style),
                    Span::raw(" "),
                    Span::styled("base fee", underline_style),
                    Span::raw(" "),
                    Span::styled("txns", underline_style),
                    Span::raw(" "),
                    Span::styled("size", underline_style),
                ]));

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
                                Completed(block) => util::format_block(block),
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

                let bold_title = Span::styled(
                    "turtlescan",
                    Style::default().add_modifier(Modifier::BOLD),
                );

                let status_line = Paragraph::new("  (q) quit - (c) configure columns")
                    .block(Block::default().title(bold_title));
                f.render_widget(status_line, chunks[1]);
            })?;
        }

        // let input = stdin.next();  // blocks until we have more input
        let input = rx.recv().unwrap()?; // blocks until we have more input

        match input {
            UIMessage::Key(key) => match key {
                Key::Char('q') => break,
                Key::Right => {
                    current_tab = (current_tab + 1) % tab_count;
                }
                Key::Left => {
                    current_tab = match current_tab {
                        0 => tab_count - 1,
                        x => (x - 1) % tab_count,
                    }
                }
                _ => (),
            },
            UIMessage::Refresh() => {}
        }
    }

    Ok(())
}

struct BlockFetcher {
    _bg_thread: thread::JoinHandle<()>,
    network_tx: mpsc::Sender<ArcFetch>, // tell network what to fetch
                                        // network_rx: mpsc::Receiver<ArcFetch>,
}

impl BlockFetcher {
    fn start<T: JsonRpcClient + 'static>(
        provider: Provider<T>,
        highest_block: Arc<Mutex<Option<u32>>>,
        tx: mpsc::Sender<Result<UIMessage, io::Error>>,
    ) -> BlockFetcher {
        let (network_tx, network_rx) = mpsc::channel();

        let handle = thread::spawn(move || {
            run_networking(provider, highest_block, tx, network_rx);
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
        self.network_tx.send(sent_fetch).unwrap();

        new_fetch
    }
}

#[tokio::main(worker_threads = 1)]
async fn run_networking<T: JsonRpcClient>(
    provider: Provider<T>,
    highest_block: Arc<Mutex<Option<u32>>>,
    tx: mpsc::Sender<Result<UIMessage, io::Error>>,
    network_rx: mpsc::Receiver<ArcFetch>,
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

    loop {
        // we're blocking from inside a coro but atm we're the only coro in this reactor
        let arc_fetch = network_rx.recv().unwrap(); // blocks until we have more input

        let block_number = {
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
