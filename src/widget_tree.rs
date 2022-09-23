use std::collections::HashSet;

use tui::{widgets::{StatefulWidget, Block, Widget, Paragraph}, text::{Spans, Span}, layout::Rect, buffer::Buffer};

pub type TreeLoc = Vec<usize>;

pub const INDENT_WIDTH: usize = 2;

// renders a tree. note that our caller is responsible for handling highlights
#[derive(Debug, PartialEq, Eq)]
pub struct TreeItem<T> {
    pub content: T,
    pub children: Vec<TreeItem<T>>,
}

impl<T> TreeItem<T> {
    pub fn new(content: T) -> TreeItem<T> {
        TreeItem {
            content,
            children: vec![]
        }
    }
    
    pub fn traverse(&self, loc: TreeLoc) -> Option<&T> {
        let mut result = self;
        
        for idx in loc.into_iter() {
            match result.children.get(idx) {
                None => return None,
                Some(child) => {
                    result = child;
                }
            }
        }
        
        return Some(&result.content)
    }
}

impl<'a> From<String> for TreeItem<Spans<'a>> {
    fn from(s: String) -> TreeItem<Spans<'a>> {
        TreeItem::new(Spans::from(s))
    }
}

impl<'a> From<&'a str> for TreeItem<Spans<'a>> {
    fn from(s: &'a str) -> TreeItem<Spans<'a>> {
        TreeItem::new(Spans::from(s))
    }
}

impl<'a> From<Spans<'a>> for TreeItem<Spans<'a>> {
    fn from(s: Spans<'a>) -> TreeItem<Spans<'a>> {
        TreeItem::new(s)
    }
}

// TODO(2022-09-22) a TreeItems(Vec<TreeItem>) struct seems to make sense? That's also where traverse() belongs
fn flatten_items<'a>(items: Vec<TreeItem<Spans<'a>>>) -> Vec<(TreeLoc, Spans<'a>)> {
    let mut result = vec![];
    
    for (idx, item) in items.into_iter().enumerate() {
        let flattened = flatten_item(item);
        for (loc, s) in flattened.into_iter() {
            let mut full_idx = vec![idx];
            full_idx.extend(loc);
            
            result.push((full_idx, s));
        }
    }

    return result;
}

fn flatten_item<'a>(item: TreeItem<Spans<'a>>) -> Vec<(TreeLoc, Spans<'a>)> {
    let mut result = vec![];
    
    result.push((vec![], item.content));

    for (idx, item) in item.children.into_iter().enumerate() {
        let flattened = flatten_item(item);
        for (loc, s) in flattened.into_iter() {
            let mut full_idx = vec![idx];
            full_idx.extend(loc);
            
            result.push((full_idx, s));
        }
    }

    return result;
}

pub struct Tree<'a, 'b> {
    pub block: Option<Block<'a>>,
    
    // TODO: should this, instead, be a root TreeItem?

    // 2022-09-22 decent chance I regret choosing Spans over Text. Spans represents a
    //            single line of content where Text represents multiple lines. I'm making
    //            life simpler for now but might need to refactor this later
    pub items: Vec<TreeItem<Spans<'b>>>,
}

pub struct TreeState {
    pub top: TreeLoc,
    pub selection: Option<TreeLoc>,
    pub collapsed: HashSet<TreeLoc>,
}

impl Default for TreeState {
    fn default() -> Self {
        TreeState { top: vec![], selection: None, collapsed: HashSet::new() }
    }
}

impl TreeState {
    pub fn select(&mut self, treeloc: TreeLoc) {
        self.selection = Some(treeloc);
    }
}

impl<'a, 'b> Tree<'a, 'b> {
    pub fn new(items: Vec<TreeItem<Spans<'b>>>) -> Tree<'a, 'b> {
        Tree {
            block: None,
            items: items,
        }
    }
    
    pub fn block(mut self, block: Block<'a>) -> Tree<'a, 'b> {
        self.block = Some(block);
        self
    }
    
    // in tui-rs this logic happens during the call to `render` (see widgets/list.rs):
    // - when widgets are drawn they scroll themselves in order to keep the selected item
    //   in the viewport.
    // 
    // this method allows the caller to "pre-scroll" for a given viewport so calls to
    // root.traverse(state.selection) will return the correct item, allowing the caller
    // to appropriately highlight the selected item
    pub fn scroll(&self, _area: Rect, _state: &mut TreeState) {
        // there are a couple cases to handle
        // 
        // - top cannot be greater than the number of items we have
        // - top <= selection (selection before viewport -> scroll up)
        // - selection afer viewport -> scroll down
        // 
        // - you cannot select a collapsed item
        // 
        // TODO: update state.top
    }
    
    fn inner_area(&self, area: Rect) -> Rect {
        match self.block {
            None => area,
            Some(ref block) => {
                block.inner(area)
            }
        }
    }
    
    fn render_block(&mut self, area: Rect, buf: &mut Buffer) {
        if let Some(block) = self.block.take() {
            block.render(area, buf);
        }
    }
}

impl<'a> StatefulWidget for Tree<'a, 'a> {
    type State = TreeState;
    
    fn render(mut self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let inner_area = self.inner_area(area);
        self.render_block(area, buf);
        let area = inner_area;
        
        if area.width < 1 || area.height < 1 {
            // not enough room to render anything!
            return;
        }
        
        if self.items.is_empty() {
            // nothing to render!
            return;
        }
        
        // TODO: actually implement scrolling
        self.scroll(area, state);
        
        let mut spans: Vec<Spans> = vec![];
        let items = flatten_items(self.items);
        for (treeloc, item) in items.into_iter() {
            if treeloc < state.top {
                continue;
            }
            
            let indent = treeloc.len().saturating_sub(1);
            let indent = std::iter::repeat(" ").take(indent * INDENT_WIDTH).collect::<String>();
            
            let mut with_indent = vec![Span::from(indent)];
            with_indent.extend(item.0);
            
            // TODO: if collapsed include a grey (collapsed) indication
            
            let with_indent = Spans::from(with_indent);
            spans.push(with_indent);
        }
        
        let widget = Paragraph::new(spans);
        widget.render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::TreeItem;
    
    #[test]
    fn traverse_no_children() {
        let root: TreeItem<usize> = TreeItem {
            content: 0,
            children: vec![],
        };
        
        assert_eq!(root.traverse(vec![]), Some(&0));
        assert_eq!(root.traverse(vec![0]), None);
    }
    
    #[test]
    fn traverse_children() {
        let root: TreeItem<usize> = TreeItem {
            content: 0,
            children: vec![TreeItem {
                content: 1,
                children: vec![],
            }],
        };
        
        assert_eq!(root.traverse(vec![]), Some(&0));
        assert_eq!(root.traverse(vec![0]), Some(&1));
        assert_eq!(root.traverse(vec![0, 0]), None);
        assert_eq!(root.traverse(vec![1]), None);
    }
}