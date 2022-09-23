
use crate::util;
use crate::style;
use crate::column;
use crate::widget_tree::Tree;
use crate::widget_tree::TreeItem;
use crate::widget_tree::TreeState;

use ethers_core::types::Transaction;
use ethers_core::types::TransactionReceipt;
use tui::style::Style;
use tui::text::Span;
use tui::text::Spans;
use tui::widgets::{Block, Borders};
use tui::{backend::Backend, Frame};
use tui::layout::Rect;

pub struct PaneTransactionDetails {
    // careful: this object is thrown away at the end of each frame, don't keep
    //          any important data here
    pub txn: Option<Transaction>,
    pub receipt: Option<TransactionReceipt>,
    pub is_focused: bool,
    pub area: Rect,
}

impl PaneTransactionDetails {
    pub fn draw<B: Backend>(&self, frame: &mut Frame<B>, state: &mut TreeState) {
        let txn = &self.txn;

        // TODO(2022-09-23) take up as much width as we can
        let title = txn
            .as_ref()
            .map(|txn| util::format_block_hash(txn.hash.as_bytes()))
            .unwrap_or("Transaction".to_string());
        
        let tree_item = self.items(state);
        
        let widget = Tree::new(tree_item).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(style::border_color(self.is_focused)))
                .title(title),
        );
        
        frame.render_stateful_widget(widget, self.area, state);
    }
    
    pub fn items(&self, state: &TreeState) -> Vec<TreeItem<Spans>> {
        let mut txn_selection = None;
        match state.selection {
            None => (),
            Some(ref selection) => {
                match selection.get(0) {
                    None => (),
                    Some(idx) => {
                        if *idx == 0 {
                            txn_selection = selection.get(1).cloned();
                        }
                    }
                }
            }
        }
        
        match &self.txn {
            None => vec![],
            Some(txn) => {
                vec![
                    self.txn_tree_item(txn, txn_selection),
                    Self::receipt_tree_item(&self.receipt),
                ]
            }
        }
    }
    
    // this breaks encapsulation: it hard-codes an idea of how Tree handles indentation
    // TODO(2-2209-23): this should also be called to render the "deployed to" column in our receipt items
    fn make_kv_from_hash<'a>(width: u16, key: &'a str, hash: &[u8]) -> (&'a str, String) {
        let avail_width = (width - 2 - 2 - 4) as usize - key.len();
        
        (key, util::format_bytes_into_width(hash, avail_width))
    }
    
    // TODO(2022-09-23) this should probably be unified with our txn columns
    fn txn_tree_item(&self, txn: &Transaction, selection_offset: Option<usize>) -> TreeItem<Spans> {
        // TODO: how do we represent `txn.inpur`?
        //       we should at very least print and try to parse the 4bytes!
        let kvs = vec![
            Self::make_kv_from_hash(self.area.width, "hash", txn.hash.as_bytes()),
            Self::make_kv_from_hash(self.area.width, "from", txn.from.as_bytes()),
            ("nonce", txn.nonce.to_string()),
            match txn.to {
                None => ("to", "".to_owned()),
                Some(hash) => Self::make_kv_from_hash(self.area.width, "to", hash.as_bytes()),
            },
            ("value", txn.value.to_string()),
            ("gas price", txn.gas_price.map(|price| price.to_string()).unwrap_or("None".to_string())),
            ("max priority fee", txn.max_priority_fee_per_gas.map(|price| price.to_string()).unwrap_or("None".to_string())),
            ("max gas fee", txn.max_fee_per_gas.map(|price| price.to_string()).unwrap_or("None".to_string())),
        ];
        
        let children: Vec<Spans> = kvs.iter().enumerate().map(|(idx, (key, value))| {
            let selected = match selection_offset {
                None => false,
                Some(selection) => idx == selection,
            };
            
            if selected {
                Spans::from(vec![
                    Span::from(format!("{}: ", key)),
                    Span::styled(format!("{}", value), Style::default().bg(style::selection_color(self.is_focused))),
                ])
            } else {
                Spans::from(format!("{}: {}", key, value))
            }
        }).collect();
        let children: Vec<TreeItem<Spans>> = children.into_iter().map(|s| TreeItem::from(s)).collect();
        
        let content = Spans::from("txn:");
        
        TreeItem { content, children }
    }
    
    fn receipt_tree_item(receipt: &Option<TransactionReceipt>) -> TreeItem<Spans> {
        let content = match receipt {
            None => Spans::from("receipt: (fetching)"),
            Some(_) => Spans::from("receipt:"),
        };
        
        let children = match receipt {
            None => vec![],
            Some(receipt) => {
                let columns = column::default_receipt_columns();
                
                columns.iter()
                    .map(|col| {
                        TreeItem::new(Spans::from(format!("  {}: {}", col.name, (col.render)(&receipt))))
                    })
                    .collect()
            }
        };
        
        TreeItem {
            content: content,
            children: children,
        }
    }
}