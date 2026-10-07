//! The queue between native callbacks and the program (ADR 0074 §4).
//!
//! Toolkit callbacks never call JavaScript. They append here, and the program drains the queue
//! between its own turns. That is what keeps run-to-completion intact: a callback that AppKit
//! fires synchronously inside a property write (a window resized by `setContentSize:`) becomes
//! an event the program sees after the write returns, not a nested call.

use std::cell::RefCell;
use std::collections::VecDeque;

/// Event kinds, numbered as the TypeScript side reads them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum EventKind {
    Press = 1,
    Input = 2,
    Submit = 3,
    Change = 4,
    Close = 5,
    Resize = 6,
    Menu = 7,
    Hotkey = 8,
    Tray = 9,
    Quit = 10,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// The widget the event belongs to; 0 for application-wide events (menus, hotkeys, quit).
    pub handle: u32,
    pub kind: EventKind,
    pub a: f64,
    pub b: f64,
    pub text: String,
}

/// The fixed-size part of an event as the C ABI hands it out; the text follows through
/// `kiln_gui_event_text`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct EventHeader {
    pub handle: u32,
    pub kind: u32,
    pub a: f64,
    pub b: f64,
    pub text_len: u64,
}

thread_local! {
    static QUEUE: RefCell<VecDeque<Event>> = const { RefCell::new(VecDeque::new()) };
    static CURRENT: RefCell<Option<Event>> = const { RefCell::new(None) };
}

pub fn push(event: Event) {
    QUEUE.with(|q| q.borrow_mut().push_back(event));
}

pub fn is_empty() -> bool {
    QUEUE.with(|q| q.borrow().is_empty())
}

/// Moves the oldest event into the current slot and returns its header.
pub fn next() -> Option<EventHeader> {
    let event = QUEUE.with(|q| q.borrow_mut().pop_front())?;
    let header = EventHeader {
        handle: event.handle,
        kind: event.kind as u32,
        a: event.a,
        b: event.b,
        text_len: event.text.len() as u64,
    };
    CURRENT.with(|c| *c.borrow_mut() = Some(event));
    Some(header)
}

/// The text of the event [`next`] returned last.
pub fn current_text() -> String {
    CURRENT.with(|c| {
        c.borrow()
            .as_ref()
            .map(|e| e.text.clone())
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_come_out_in_order_with_their_text() {
        push(Event {
            handle: 7,
            kind: EventKind::Input,
            a: 0.0,
            b: 0.0,
            text: "hé".into(),
        });
        push(Event {
            handle: 8,
            kind: EventKind::Change,
            a: 2.0,
            b: 0.0,
            text: String::new(),
        });
        let first = next().unwrap();
        assert_eq!((first.handle, first.kind, first.text_len), (7, 2, 3));
        assert_eq!(current_text(), "hé");
        let second = next().unwrap();
        assert_eq!((second.handle, second.a), (8, 2.0));
        assert!(next().is_none());
        assert!(is_empty());
    }
}
