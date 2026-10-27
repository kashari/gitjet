//! Messages from popups and palette entries back to the log view.
//!
//! Popup callbacks and palette entries only receive `&Ctx`, so they post a [`Msg`] here and the
//! [`LogView`](super::LogView) applies it on its next draw or key press. The UI runs on one thread, so
//! a thread-local queue is all that is needed.

use std::cell::RefCell;

use super::filters::Filters;

/// A request for the log view.
pub enum Msg {
    /// Change filters (the log reloads afterwards).
    Edit(Box<dyn FnOnce(&mut Filters)>),
    /// Remove every filter.
    ClearFilters,
    /// Select this commit (full hash), reloading anchored at it when it is not loaded.
    GoTo(String),
    /// Mark or unmark a commit (full hash).
    ToggleMark(String),
}

thread_local! {
    static MAILBOX: RefCell<Vec<Msg>> = const { RefCell::new(Vec::new()) };
}

/// Queue a message for the log view.
pub fn post(msg: Msg) {
    MAILBOX.with(|m| m.borrow_mut().push(msg));
}

/// Take all queued messages, oldest first.
pub fn take() -> Vec<Msg> {
    MAILBOX.with(|m| std::mem::take(&mut *m.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_arrive_in_order() {
        take();
        post(Msg::GoTo("a".into()));
        post(Msg::ClearFilters);
        let got = take();
        assert!(matches!(got.as_slice(), [Msg::GoTo(h), Msg::ClearFilters] if h == "a"));
        assert!(take().is_empty());
    }
}
