use crate::util;
use crate::style;
use crate::column;

use ethers_core::types::Transaction;
use ethers_core::types::TransactionReceipt;
use tui::style::Style;
use tui::text::Spans;
use tui::text::Text;
use tui::widgets::{Paragraph, Block, Borders};
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
    pub fn draw<B: Backend>(&self, frame: &mut Frame<B>) {
        let txn = &self.txn;

        let title = txn
            .as_ref()
            .map(|txn| util::format_block_hash(txn.hash.as_bytes()))
            .unwrap_or("Transaction".to_string());

        let text = match self.txn {
            None => Text::raw(""),
            Some(ref txn) => self.text(txn),
        };

        let widget = Paragraph::new(text).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(style::border_color(self.is_focused)))
                .title(title),
        );
        frame.render_widget(widget, self.area);
    }

    // 2022-09-12 this lifetime hint is technically too broad, the Text holds a lifetime to to all
    // the str's we threw into it, but all strs are either 'static or belong to newly created
    // String's. Some day it could be a nice exercise to untangle this.
    fn text<'a>(&'a self, txn: &'a Transaction) -> Text {
        let mut txn_spans = Self::txn_spans(txn, self.area);
        let receipt_spans = Self::receipt_spans(&self.receipt);
        txn_spans.extend(receipt_spans);

        Text::from(txn_spans)
    }

    fn receipt_spans(receipt: &Option<TransactionReceipt>) -> Vec<Spans> {
        match receipt {
            None => vec![Spans::from("receipt: (fetching)")],
            Some(receipt) => {
                //TODO(2021-09-15) no reason to build a new one every time
                //                 how do rust globals work? thread locals?
                let columns = column::default_receipt_columns();

                let mut result = vec![Spans::from("receipt:")];

                result.extend::<Vec<Spans>>(
                    columns
                        .iter()
                        .map(|col| {
                            Spans::from(format!("  {}: {}", col.name, (col.render)(&receipt)))
                        })
                        .collect(),
                );

                result
            }
        }
    }
    
    fn txn_spans(txn: &Transaction, area: Rect) -> Vec<Spans> {
        // TODO: this logic should also apply to "deployed to" from our receipt_spans,
        //       this means either inlining the important columns here
        //       or passing the available width in to the column render function
        let hash_to_span = |hash: &[u8], title: &str| -> Spans {
            let avail_width = (area.width - 2 - 4) as usize - title.len();
            Spans::from(format!(
                "  {}: {}",
                title,
                util::format_bytes_into_width(hash, avail_width)
            ))
        };

        vec![
            Spans::from("txn:"),
            //TODO: should probably unify these with the txn columns
            hash_to_span(txn.hash.as_bytes(), "hash"),
            hash_to_span(txn.from.as_bytes(), "from"),
            Spans::from(format!("    nonce: {}", txn.nonce.to_string())),
            match txn.to {
                None => Spans::from("  to: "),
                Some(hash) => hash_to_span(hash.as_bytes(), "to")
            },
            Spans::from(format!("  value: {}", txn.value.to_string())),
            // TODO(2021-09-15) implement this
            // Spans::from(format!("  data: {}", txn.input.to_string())),
            Spans::from(format!(
                "  gas price: {}",
                txn.gas_price
                    .map(|price| price.to_string())
                    .unwrap_or("None".to_string())
            )),
            Spans::from(format!(
                "  max priority fee: {}",
                txn.max_priority_fee_per_gas
                    .map(|fee| fee.to_string())
                    .unwrap_or("None".to_string())
            )),
            Spans::from(format!(
                "  max gas fee: {}",
                txn.max_fee_per_gas
                    .map(|fee| fee.to_string())
                    .unwrap_or("None".to_string())
            )),
        ]
    }
}