use tui::{widgets::{Block, ListItem, StatefulWidget, ListState, Paragraph, List, Widget}, style::Style, text::Spans, layout::Rect, buffer::Buffer};

pub struct HeaderList<'a> {
    /*
     * a List where the first row is a header and does not participate in
     * scrolling or selection
     */
    // we need a lifetime because the Title uses &str to hold text
    pub block: Option<Block<'a>>,
    pub highlight_style: Style,

    pub header: Option<Spans<'a>>,
    pub items: Vec<ListItem<'a>>,
}

impl<'a> HeaderList<'a> {
    pub fn new(items: Vec<ListItem<'a>>) -> HeaderList<'a> {
        HeaderList {
            block: None,
            highlight_style: Style::default(),
            items: items,
            header: None,
        }
    }

    pub fn block(mut self, block: Block<'a>) -> HeaderList<'a> {
        self.block = Some(block);
        self
    }

    pub fn highlight_style(mut self, style: Style) -> HeaderList<'a> {
        self.highlight_style = style;
        self
    }

    pub fn header(mut self, header: Spans<'a>) -> HeaderList<'a> {
        self.header = Some(header);
        self
    }
}

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