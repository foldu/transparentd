use i3ipc::reply::Node;

pub const PROBABLE_AMOUNT_OF_WINDOWS: usize = 16;

pub struct AllWindows {
    stack: Vec<Node>,
}

pub trait I3Ext {
    fn iter_windows(&mut self) -> Result<AllWindows, i3ipc::MessageError>;
    fn get_focused_window(&mut self) -> Result<Option<i64>, i3ipc::MessageError>;
}

impl I3Ext for i3ipc::I3Connection {
    fn iter_windows(&mut self) -> Result<AllWindows, i3ipc::MessageError> {
        let mut stack = Vec::with_capacity(PROBABLE_AMOUNT_OF_WINDOWS);
        stack.push(self.get_tree()?);
        Ok(AllWindows { stack })
    }

    fn get_focused_window(&mut self) -> Result<Option<i64>, i3ipc::MessageError> {
        Ok(self
            .iter_windows()?
            .find(|node| node.focused)
            .map(|node| node.id))
    }
}

impl Iterator for AllWindows {
    type Item = Node;
    fn next(&mut self) -> Option<Self::Item> {
        self.stack.pop().map(|node| {
            self.stack.extend(node.nodes.clone());
            node
        })
    }
}
