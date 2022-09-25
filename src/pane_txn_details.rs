
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
        
        let tree_item = self.items();
        
        let widget = Tree::new(tree_item)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(style::border_color(self.is_focused)))
                    .title(title))
            .focused(self.is_focused);
        
        frame.render_stateful_widget(widget, self.area, state);
    }
    
    pub fn items<'a>(&self) -> Vec<TreeItem<'a>> {
        match &self.txn {
            None => vec![],
            Some(txn) => {
                vec![
                    self.txn_tree_item(txn),
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
    fn txn_tree_item<'a>(&self, txn: &Transaction) -> TreeItem<'a> {
        // TODO: how do we represent `txn.input`?
        //       we should at very least print and try to parse the 4bytes, that's
        //       just about the most important field and it's currently completely
        //       hidden!
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
        
        // TODO(2022-09-24): use the passed width, rather than hard-coding it ourselves
        let children: Vec<TreeItem> = kvs.into_iter().map(|(key, value)| {
            let render = move |_width: u16, focused: bool, selected: bool| -> Spans {
                let key_span = Span::from(format!("{}: ", key));
                let value_span = style::selected_span(value.clone(), selected, focused);
                Spans::from(vec![key_span, value_span])
            };
            
            TreeItem::new(render)
        }).collect();
        
        let mut result = TreeItem::from("txn:");
        result.children = children;
        result
    }
    
    fn receipt_tree_item<'a>(receipt: &Option<TransactionReceipt>) -> TreeItem<'a> {
        let mut result = TreeItem::from(
            match receipt {
                None => "receipt: (fetching)",
                Some(_) => "receipt:",
            }
        );
        
        result.children = match receipt {
            None => vec![],
            Some(receipt) => {
                let columns = column::default_receipt_columns();
                
                columns.into_iter()
                    .map(|col| {
                        // TODO(2022-09-24) is there a way to save ourselves from the clone?
                        //                   there might be for now, but there probably isn't
                        //                   once col.render accepts a width argument
                        let cloned = receipt.clone();
                        TreeItem::new(move |_width: u16, focused: bool, selected: bool| {
                            let key_span = Span::from(format!("{}: ", col.name));
                            let rendered = (col.render)(&cloned);
                            let val_span = style::selected_span(rendered, selected, focused);
                            Spans::from(vec![key_span, val_span])
                        })
                    })
                    .collect()
            }
        };

        result
    }
}