use std::{collections::HashSet, fmt::Debug};

use tui::{widgets::{StatefulWidget, Block, Widget, Paragraph}, text::{Spans, Span}, layout::Rect, buffer::Buffer, style::Style};

use crate::style;

pub type TreeLoc = Vec<usize>;

pub const INDENT_WIDTH: usize = 2;

pub trait RenderFn<'a>: Fn(u16, bool, bool) -> Spans<'a> {
    // width, is_selected, is_focused
}
 
impl<'a, F> RenderFn<'a> for F where F: Fn(u16, bool, bool) -> Spans<'a> { }

impl<'a> std::fmt::Debug for dyn RenderFn<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RenderFunction")
    }
}

// renders a tree. note that our caller is responsible for handling highlights
pub struct TreeItem<'a> {
    pub render: Box<dyn RenderFn<'a> + 'a>,
    pub children: Vec<TreeItem<'a>>,
    // TODO(2022-09-23) what does it even mean for us to accept the lifetime of the
    //                  element the passed-in closure returns?
}

// derive(Debug) causes issues with lifetimes so lets implement it manually
impl<'a> Debug for TreeItem<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TreeItem(len={})", self.children.len())
    }
}

impl<'a> TreeItem<'a> {
    pub fn new(render: impl RenderFn<'a> + 'a) -> TreeItem<'a> {
        TreeItem {
            render: Box::new(render),
            children: vec![]
        }
    }
}

impl<'a> From<String> for TreeItem<'a> {
    fn from(s: String) -> TreeItem<'a> {
        let render = move |_width: u16, focused: bool, selected: bool| {
            style::selected_span(s.clone(), selected, focused);
            if selected {
                Spans::from(
                    Span::styled(
                        s.clone(),
                        Style::default().bg(style::selection_color(focused))
                    )
                )
            } else {
                Spans::from(s.clone())
            }
        };
        
        TreeItem {
            render: Box::new(render),
            children: vec![],
        }
    }
}

impl<'a> From<&'a str> for TreeItem<'a> {
    fn from(s: &'a str) -> TreeItem<'a> {
        let render = move |_width: u16, focused: bool, selected: bool| {
            if selected {
                Spans::from(
                    Span::styled(
                        s,
                        Style::default().bg(style::selection_color(focused))
                    )
                )
            } else {
                Spans::from(s)
            }
        };
        
        TreeItem::new(Box::new(render))
    }
}

impl<'a> From<Spans<'a>> for TreeItem<'a> {
    fn from(s: Spans<'a>) -> TreeItem<'a> {
        let render = move |_width: u16, _focused: bool, _selected: bool| {
            let cloned = s.clone();
            cloned
        };
        
        TreeItem::new(Box::new(render))
    }
}

// TODO(2022-09-22) a TreeItems(Vec<TreeItem>) struct seems to make sense? That's also where traverse() belongs
fn flatten_items<'a>(items: Vec<TreeItem<'a>>) -> Vec<(TreeLoc, impl RenderFn<'a>)> {
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

fn flatten_item<'a>(item: TreeItem<'a>) -> Vec<(TreeLoc, impl RenderFn<'a>)> {
    let mut result = vec![];
    
    result.push((vec![], item.render));

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
    
    pub focused: bool,
    
    // TODO: should this, instead, be a root TreeItem?

    // 2022-09-22 decent chance I regret choosing Spans over Text. Spans represents a
    //            single line of content where Text represents multiple lines. I'm making
    //            life simpler for now but might need to refactor this later
    pub items: Vec<TreeItem<'b>>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
enum TreeSelection {
    None,
    Just(TreeLoc),
    Next(TreeLoc),
    Prev(TreeLoc),
}

impl TreeSelection {
    fn next(self) -> Self {
        use TreeSelection::*;
        match self {
            None => None,
            Prev(treeloc) => Just(treeloc),
            Just(treeloc) => Next(treeloc),
            Next(treeloc) => Next(treeloc),
        }
    
    }
    
    fn prev(self) -> Self {
        use TreeSelection::*;
        match self {
            None => None,
            Prev(treeloc) => Prev(treeloc),
            Just(treeloc) => Prev(treeloc),
            Next(treeloc) => Just(treeloc),
        }
    }
}

impl Default for TreeSelection {
    fn default() -> Self {
        Self::None
    }
}

pub struct TreeState {
    pub top: TreeLoc,
    selection: TreeSelection,
    pub collapsed: HashSet<TreeLoc>,
}

impl Default for TreeState {
    fn default() -> Self {
        TreeState {
            top: vec![],
            selection: TreeSelection::default(),
            collapsed: HashSet::new()
        }
    }
}

impl TreeState {
    pub fn select(&mut self, treeloc: TreeLoc) {
        self.selection = TreeSelection::Just(treeloc);
    }
    

    /// selects either the previous child or our parent
    pub fn select_prev(&mut self) {
        self.selection = self.selection.clone().prev();
    }
    
    /// selects the next child
    pub fn select_next(&mut self) {
        // TOOD(2022-09-24) if I was better at rust I would be able to avoid this clone
        self.selection = self.selection.clone().next();
    }

}

impl<'a, 'b> Tree<'a, 'b> {
    pub fn new(items: Vec<TreeItem<'b>>) -> Tree<'a, 'b> {
        Tree {
            block: None,
            items: items,
            focused: false,
        }
    }
    
    pub fn block(mut self, block: Block<'a>) -> Tree<'a, 'b> {
        self.block = Some(block);
        self
    }
    
    pub fn focused(mut self, focused: bool) -> Tree<'a, 'b>{
        self.focused = focused;
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
    
    // when select_next()/select_prev() are called they do not know the shape of the data so they
    // cannot navigate to the next/prev element. Now that we have the data in front of us we honor
    // the request
    fn fix_selection(treelocs: Vec<&TreeLoc>, state: &mut TreeState) {
        // TODO(2022-09-24) something needs to be changed here once support for collapsing items is
        //                  added as written these allow you to select merged items
        use TreeSelection::*;
        match state.selection {
            None => return,
            Just(_) => return,

            Prev(ref treeloc) => {
                let partition = treelocs.binary_search(&&treeloc);
                let pos = match partition { Ok(pos) => pos, Err(pos) => pos };
                
                let pos = pos.saturating_sub(1);
                state.selection = Just(treelocs[pos].clone());
            },
            Next(ref treeloc) => {
                let partition = treelocs.binary_search(&&treeloc);
                let pos = match partition { Ok(pos) => pos, Err(pos) => pos };

                let pos = std::cmp::min(pos + 1, treelocs.len() - 1);
                state.selection = Just(treelocs[pos].clone());
            }
        }
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

        let items = flatten_items(self.items);
        
        {
            let treelocs: Vec<&TreeLoc> = items.iter().map(|(loc, _fn)| {loc}).collect();
            Self::fix_selection(treelocs, state);
        }
        
        let mut spans: Vec<Spans> = vec![];
        for (treeloc, item) in items.into_iter() {
            if treeloc < state.top {
                continue;
            }
            
            let indent = treeloc.len().saturating_sub(1);
            let indent = std::iter::repeat(" ").take(indent * INDENT_WIDTH).collect::<String>();
            
            let mut with_indent = vec![Span::from(indent)];
            
            use TreeSelection::*;
            let selected = match state.selection {
                None => false,
                Just(ref selection) => *selection == treeloc,
                
                // TODO(2022-09-24) it's possible to make this error unrepresentable, we should
                //                  not be operating directly over self.selection...
                Prev(_) | Next(_) => panic!("should have called fix_selection()"),
            };
            
            let rendered: Spans = (item)(area.width, self.focused, selected);
            with_indent.extend(rendered.0);
            
            // TODO: if collapsed include a grey (collapsed) indication
            
            let with_indent = Spans::from(with_indent);
            spans.push(with_indent);
        }
        
        let widget = Paragraph::new(spans);
        widget.render(area, buf);
    }
}