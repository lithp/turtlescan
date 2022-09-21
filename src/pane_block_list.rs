use ethers_core::types::{Block as EthBlock, TxHash};
use tui::{backend::Backend, Frame, layout::Rect, widgets::{ListState, ListItem, Block, Borders}, text::Span, style::Style};

use crate::{data, column::{self, Column}, header_list::HeaderList, style};


pub struct PaneBlockList<'a> {
    pub highest_block: u64,
    pub selected_block: Option<u64>,
    pub top_block: u64,
    pub bottom_block: u64,
    pub columns: &'a Vec<Column<EthBlock<TxHash>>>,
    pub is_focused: bool,
}

impl<'a> PaneBlockList<'a> {
    pub fn draw<B: Backend, T: data::Data>(&self, frame: &mut Frame<B>, area: Rect, database: &mut T) {
        let mut block_list_state = ListState::default();
        if let Some(selection) = self.selected_block {
            //TODO: error handling?
            let offset = self.top_block - selection;
            block_list_state.select(Some(offset as usize));
        }

        let header = column::columns_to_header(&self.columns);

        let block_range = (self.bottom_block)..(self.top_block + 1);
        let block_lines = {
            let block_lines: Vec<ListItem> = block_range
                .rev()
                .map(|blocknum| (blocknum, database.get_block(blocknum)))
                // if we do not do this rust complains there are multiple active closures
                // which reference self which... might be a legitimate complaint?
                // TODO(2021-09-16) any better ways to fix this problem?
                .collect::<Vec<(u64, data::RequestStatus<EthBlock<TxHash>>)>>()
                .iter()
                .map(|(height, fetch)| {
                    use data::RequestStatus::*;
                    let formatted = match fetch {
                        Waiting() => format!("{} waiting", height),
                        Started() => format!("{} fetching", height),
                        Completed(block) => column::render_item_with_cols(&self.columns, &block),
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
                    .border_style(Style::default().fg(style::border_color(self.is_focused)))
                    .title(self.title()),
            )
            .highlight_style(Style::default().bg(style::selection_color(self.is_focused)))
            .header(header);
        frame.render_stateful_widget(block_list, area, &mut block_list_state);
    }
    
    fn title(&self) -> String {
        let is_behind = self.top_block != self.highest_block;
        let has_selection = self.selected_block.is_some();
        
        if is_behind {
            format!("Blocks ({} behind)", self.highest_block - self.top_block)
        } else if !has_selection {
            "Blocks (following tip)".to_string()
        } else {
            "Blocks".to_string()
        }
    }
}