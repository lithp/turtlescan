use turtlescan::data;
use turtlescan::tui;

use ::tui::backend::TestBackend;
use ::tui::buffer::Buffer;
use ::tui::Terminal;
use ethers_core::types::{Block as EthBlock, Transaction, TransactionReceipt, TxHash};

use std::fmt::Write;
use unicode_width::UnicodeWidthStr;

struct FakeDB {}
impl data::Data for FakeDB {
    fn apply_progress(&mut self, _progress: data::Response) {}
    fn bump_highest_block(&self, _blocknum: u64) {}
    fn get_highest_block(&self) -> Option<u64> {
        Some(3)
    }
    fn get_block(&mut self, blocknum: u64) -> data::RequestStatus<EthBlock<TxHash>> {
        if blocknum == 3 {
            // our test relies on the fact that this fake block has no transactions!
            let block_repr = r#"{"number":"0x3","hash":"0xda53da08ef6a3cbde84c33e51c04f68c3853b6a3731f10baa2324968eee63972","parentHash":"0x689c70c080ca22bc0e681694fa803c1aba16a69c8b6368fed5311d279eb9de90","mixHash":"0x0000000000000000000000000000000000000000000000000000000000000000","nonce":"0x0000000000000000","sha3Uncles":"0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347","logsBloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","transactionsRoot":"0x7270c1c4440180f2bd5215809ee3d545df042b67329499e1ab97eb759d31610d","stateRoot":"0x29f32984517a7d25607da485b23cefabfd443751422ca7e603395e1de9bc8a4b","receiptsRoot":"0x056b23fbba480696b65fe5a59b8f2148a1299103c4f57df839233af2cf4ca2d2","miner":"0x0000000000000000000000000000000000000000","difficulty":"0x0","totalDifficulty":"0x0","extraData":"0x","size":"0x3e8","gasLimit":"0x6691b7","gasUsed":"0x5208","timestamp":"0x5ecedbb9","transactions":[],"uncles":[]}"#;
            let block: EthBlock<TxHash> = serde_json::from_str(block_repr).unwrap();
            return data::RequestStatus::Completed(block);
        }

        data::RequestStatus::Waiting()
    }

    fn get_block_with_transactions(
        &mut self,
        blocknum: u64,
    ) -> data::RequestStatus<EthBlock<Transaction>> {
        if blocknum == 3 {
            // our test relies on the fact that this fake block has no transactions!
            let block_repr = r#"{"number":"0x3","hash":"0xda53da08ef6a3cbde84c33e51c04f68c3853b6a3731f10baa2324968eee63972","parentHash":"0x689c70c080ca22bc0e681694fa803c1aba16a69c8b6368fed5311d279eb9de90","mixHash":"0x0000000000000000000000000000000000000000000000000000000000000000","nonce":"0x0000000000000000","sha3Uncles":"0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347","logsBloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","transactionsRoot":"0x7270c1c4440180f2bd5215809ee3d545df042b67329499e1ab97eb759d31610d","stateRoot":"0x29f32984517a7d25607da485b23cefabfd443751422ca7e603395e1de9bc8a4b","receiptsRoot":"0x056b23fbba480696b65fe5a59b8f2148a1299103c4f57df839233af2cf4ca2d2","miner":"0x0000000000000000000000000000000000000000","difficulty":"0x0","totalDifficulty":"0x0","extraData":"0x","size":"0x3e8","gasLimit":"0x6691b7","gasUsed":"0x5208","timestamp":"0x5ecedbb9","transactions":[],"uncles":[]}"#;
            let block: EthBlock<Transaction> = serde_json::from_str(block_repr).unwrap();
            return data::RequestStatus::Completed(block);
        }

        data::RequestStatus::Waiting()
    }

    fn get_block_receipts(
        &mut self,
        _blocknum: u64,
    ) -> data::RequestStatus<Vec<TransactionReceipt>> {
        data::RequestStatus::Waiting()
    }
    fn invalidate_block(&mut self, _blocknum: u64) {}
}

// copied from tui-rs:src/backend/test.rs
/// Returns a string representation of the given buffer for debugging purpose.
fn _buffer_view(buffer: &Buffer) -> String {
    let mut view = String::with_capacity(buffer.content.len() + buffer.area.height as usize * 3);
    for cells in buffer.content.chunks(buffer.area.width as usize) {
        let mut overwritten = vec![];
        let mut skip: usize = 0;
        view.push('"');
        for (x, c) in cells.iter().enumerate() {
            if skip == 0 {
                view.push_str(&c.symbol);
            } else {
                overwritten.push((x, &c.symbol))
            }
            skip = std::cmp::max(skip, c.symbol.width()).saturating_sub(1);
        }
        view.push('"');
        if !overwritten.is_empty() {
            write!(
                &mut view,
                " Hidden by multi-width symbols: {:?}",
                overwritten
            )
            .unwrap();
        }
        view.push('\n');
    }
    view
}

fn draw(ui: &mut tui::TUI<FakeDB>, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui.draw(f)).unwrap();
    terminal.backend().buffer().clone()
}

/// after this function has been called you can 'cat {dest}' and your terminal should
/// interpret the result and show you what would have been shown.
///
/// I'm pretty sure the Github UI will not correctly print this so a next step might be to
/// save the result as an SVG [1]. Though, an SVG which we can parse to get back the
/// expected output and run tests against it.
///
/// [1] https://github.com/markrages/ansi_svg/blob/master/ansi_svg.py
fn _save_buffer(ui: &mut tui::TUI<FakeDB>, dest: &str, width: u16, height: u16) {
    use ::tui::backend::TermionBackend;
    use ::tui::layout::Rect;
    use ::tui::terminal::TerminalOptions;
    use ::tui::terminal::Viewport;
    use std::fs::File;

    let file = File::create(dest).unwrap();
    let backend = TermionBackend::new(file);
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::fixed(Rect::new(0, 0, width, height)),
        },
    )
    .unwrap();
    terminal.clear().unwrap();
    terminal.draw(|f| ui.draw(f)).unwrap();
}

#[test]
fn no_crash_when_block_has_no_txns() {
    let mut database = FakeDB {}; // TODO: to make this test more robust explicitly start
                                  //       at block 3
    let mut ui = tui::TUI::new(&mut database);

    /*
     * TODO(2021-09-26): This test is not very robust at all. It performs a sequence of
     *                   actions and asserts that they don't lead us into a crash (and
     *                   emulates what a fuzz tester would have produced) but if the app
     *                   changes then this sequence of actions will not necessarily get
     *                   us back into the desired dangerous state. It is better to
     *                   explicitly start a TUI into the BlockTransactions(1) state and
     *                   from there try to handle_key_right() and draw().
     *
     *                   This might even deserve to be factored into a few unit tests.
     *                   The transaction details pane should not crash when the desired
     *                   txn offset does not exist, and it should emit a log message.
     */

    // if we do not do a draw then block_list_top_block will never be set
    draw(&mut ui, 90, 10);

    ui.handle_key_down(); // select the first block
    ui.handle_key_right(); // open the transaction list
    draw(&mut ui, 90, 10); // selects the first transaction
    ui.handle_key_right(); // open transaction details

    // this used to crash the TUI, so the fact that this call does not panic means that
    // the bug remains fixed
    draw(&mut ui, 90, 10);
    // _save_buffer(&mut ui, "startup.80.10", 80, 10);
}
