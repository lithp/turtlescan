use crate::util;

use std::io;
use std::sync::{Arc, Mutex};
use termion::raw::IntoRawMode;
use termion::screen::AlternateScreen;
use tokio::time::sleep as async_sleep;
use tui::backend::TermionBackend;
use tui::symbols::DOT;
use tui::text::{Span, Spans};
use tui::Terminal;

use tui::layout::{Constraint, Direction, Layout};
use tui::style::{Color, Modifier, Style};
use tui::widgets::{Block, Borders, List, ListItem, ListState, Tabs};

use std::thread;
use std::time;

use ethers_providers::{Middleware, Provider, JsonRpcClient};
use log::debug;
use std::error::Error;
use termion::event::Key;
use termion::input::TermRead;

use std::collections::VecDeque;

use ethers_core::types::TxHash;
use ethers_core::types::Block as EthBlock;

use std::sync::mpsc;


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



// TODO(2021-08-27) why does the following line not work?
// fn run_tui() -> Result<(), Box<io::Error>> {
pub fn run_tui<T: JsonRpcClient + 'static>(provider: Provider<T>) -> Result<(), Box<dyn Error>> {
    let stdout = io::stdout().into_raw_mode()?;
    let stdout = AlternateScreen::from(stdout);
    let backend = TermionBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (tx, rx) = mpsc::channel();

    let keys_tx = tx.clone();
    thread::spawn(move || {
        let stdin = io::stdin().keys();

        for key in stdin {
            let mapped = key.map(|k| UIMessage::Key(k));
            keys_tx.send(mapped).unwrap();
        }
    });

    // if we need it: prevents us from blocking when we check for more input
    // let mut stdin = termion::async_stdin().keys();

    let mut current_tab: usize = 0;

    let tab_count = 3;

    let blocks: VecDeque<EthBlock<TxHash>> = VecDeque::new();

    let highest_block: Arc<Mutex<Option<u32>>> = Arc::new(Mutex::new(None));

    let mut block_list_state = ListState::default();

    // let's do some networking in the background
    // no real need to hold onto this handle, the thread will be killed when this main
    // thread exits.
    let highest_block_clone = highest_block.clone();
    let _block_fetcher = BlockFetcher::start(provider, highest_block_clone, tx);

    loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([Constraint::Min(0), Constraint::Length(2)].as_ref())
                .split(f.size());

            let block_lines: Vec<ListItem> = blocks
                .iter()
                .map(|block| {
                    let formatted = util::format_block(block);
                    ListItem::new(Span::raw(formatted))
                })
                .collect();

            let block_list = List::new(block_lines)
                .block(Block::default().borders(Borders::ALL).title("Blocks"))
                .highlight_style(Style::default().bg(Color::LightGreen));
            f.render_stateful_widget(block_list, chunks[0], &mut block_list_state);

            let titles = {
                let block_number_opt = highest_block.lock().unwrap();

                match *block_number_opt {
                    None => [
                        String::from("Blocks"),
                        String::from("Tab2"),
                        String::from("Tab3"),
                    ],
                    Some(number) => [
                        String::from("Blocks"),
                        number.to_string(),
                        String::from("Tab3"),
                    ],
                }
            };

            let titles = titles.iter().cloned().map(Spans::from).collect();
            let tabs = Tabs::new(titles)
                .block(Block::default().title("Tabs")) // .borders(Borders::ALL))
                .select(current_tab)
                .highlight_style(Style::default().add_modifier(Modifier::BOLD))
                .divider(DOT);
            f.render_widget(tabs, chunks[1]);
        })?;

        // let input = stdin.next();  // blocks until we have more input
        let input = rx.recv().unwrap()?;  // blocks until we have more input

        match input {
            UIMessage::Key(key) => {
                match key {
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
                }
            },
            UIMessage::Refresh() => {}
        }
    }

    Ok(())
}


struct BlockFetcher {
    _bg_thread: thread::JoinHandle<()>
}

impl BlockFetcher {

    fn start<T: JsonRpcClient + 'static>(provider: Provider<T>, highest_block: Arc<Mutex<Option<u32>>>, tx: mpsc::Sender<Result<UIMessage, io::Error>>) -> BlockFetcher {
        let handle = thread::spawn(move || {
            run_networking(provider, highest_block, tx);
        });

        BlockFetcher {
            _bg_thread: handle
        }
    }

    fn _fetch(&self, _block_number: u32) {

    }
}


#[tokio::main(worker_threads = 1)]
async fn run_networking<T: JsonRpcClient>(provider: Provider<T>, highest_block: Arc<Mutex<Option<u32>>>, tx: mpsc::Sender<Result<UIMessage, io::Error>>) {
    debug!("started networking thread");
    async_sleep(time::Duration::from_millis(1000)).await;
    {
        let mut block_number = highest_block.lock().unwrap();
        *block_number = Some(10);
    }
    tx.send(Ok(UIMessage::Refresh())).unwrap();
    debug!("updated block number");

    let block_number_opt = provider.get_block_number().await;

    match block_number_opt {
        Err(error) => debug!("{:}", error),
        Ok(number) => {
            let mut block_number = highest_block.lock().unwrap();
            *block_number = Some(number.low_u32());
        }
    }
    tx.send(Ok(UIMessage::Refresh())).unwrap();
}
