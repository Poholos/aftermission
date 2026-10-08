//! Files the browser hands the page: picked through a file input or
//! dropped on the window. The browser reads a file asynchronously, so a
//! pick is not a job; it is a numbered exchange of messages over one
//! channel the app owns, and the app opens what the newest choice yields.
//!
//! Each pick or drop takes the next number and sends [`Picked::Chosen`]
//! the moment its file is known, then one outcome. The rule for picks
//! that overlap, which native never sees since its dialog is modal: the
//! newest pick that has chosen a file wins, and an older read that ends
//! later is dropped. Opening or canceling a chooser changes nothing, so a
//! read still running when a second chooser is canceled opens as it
//! would have.
//!
//! A pick carries the slot its log opens into, A or B, to its outcome.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use crate::worker::Slot;

/// A pick or a drop: its number, and the slot its log opens into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pick {
    pub number: u64,
    pub slot: Slot,
}

/// A message from a pick or a drop, tagged with it.
pub enum Picked {
    /// The file is known; its bytes follow.
    Chosen { pick: Pick, name: String },
    /// The file, read.
    File {
        pick: Pick,
        name: String,
        bytes: Vec<u8>,
    },
    /// The browser could not read the file. For a drop, a folder is the
    /// usual case, an entry with no file behind it; a chosen file fails
    /// when it has gone before the browser read it.
    Unreadable {
        pick: Pick,
        name: String,
        dropped: bool,
    },
    /// The chooser closed without a choice.
    Canceled { pick: Pick },
    /// The browser would not open the chooser.
    Refused { pick: Pick },
}

/// The bytes are left out: a log runs to megabytes.
impl std::fmt::Debug for Picked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Picked::Chosen { pick, name } => write!(f, "Chosen({pick:?}, {name:?})"),
            Picked::File { pick, name, bytes } => {
                write!(f, "File({pick:?}, {name:?}, {} bytes)", bytes.len())
            }
            Picked::Unreadable {
                pick,
                name,
                dropped,
            } => write!(f, "Unreadable({pick:?}, {name:?}, dropped: {dropped})"),
            Picked::Canceled { pick } => write!(f, "Canceled({pick:?})"),
            Picked::Refused { pick } => write!(f, "Refused({pick:?})"),
        }
    }
}

/// What the app acts on, once the rule above has been applied, with the
/// slot the pick was for.
#[derive(Debug)]
pub enum Outcome {
    /// The newest choice, read: open it.
    File {
        name: String,
        bytes: Vec<u8>,
        slot: Slot,
    },
    /// The newest choice could not be read; `dropped` when it came by a
    /// drop rather than the chooser.
    Unreadable {
        name: String,
        dropped: bool,
        slot: Slot,
    },
    /// The browser would not open the chooser.
    Refused { slot: Slot },
}

/// The app's end of the channel, the pick counter, and the choice it
/// waits on.
pub struct Picks {
    tx: Sender<Picked>,
    rx: Receiver<Picked>,
    /// The number the next pick takes.
    next: u64,
    /// The newest pick that has chosen a file, until its outcome lands.
    chosen: Option<(Pick, String)>,
}

impl std::fmt::Debug for Picks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Picks")
            .field("next", &self.next)
            .field("chosen", &self.chosen)
            .finish_non_exhaustive()
    }
}

impl Default for Picks {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            next: 1,
            chosen: None,
        }
    }
}

impl Picks {
    /// Number a new pick or drop for `slot` and give it the sender its
    /// messages go through.
    pub fn start(&mut self, slot: Slot) -> (Pick, Sender<Picked>) {
        let pick = Pick {
            number: self.next,
            slot,
        };
        self.next += 1;
        (pick, self.tx.clone())
    }

    /// The name of the choice being read and its slot, for the status
    /// line.
    pub fn reading(&self) -> Option<(&str, Slot)> {
        self.chosen
            .as_ref()
            .map(|(pick, name)| (name.as_str(), pick.slot))
    }

    /// Drop the read in progress: its outcome, when it lands, counts for
    /// nothing. A log that opens as A without a pick, the demo or the
    /// pair, is a newer choice than any file still being read, and a B
    /// being read would land beside it, which it does not belong with.
    pub fn drop_reading(&mut self) {
        self.chosen = None;
    }

    /// Apply the messages that have arrived and return the first outcome
    /// to act on, if any. A choice replaces the one before it; a read's
    /// result counts only for the current choice; a cancel changes
    /// nothing; a refusal is reported whatever the numbers, since it
    /// names no file and touches no read.
    pub fn poll(&mut self) -> Option<Outcome> {
        loop {
            let message = match self.rx.try_recv() {
                Ok(message) => message,
                // the sender is ours, so the channel is never closed
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return None,
            };
            match message {
                Picked::Chosen { pick, name } => self.chosen = Some((pick, name)),
                Picked::File { pick, name, bytes } if self.is_current(pick) => {
                    self.chosen = None;
                    return Some(Outcome::File {
                        name,
                        bytes,
                        slot: pick.slot,
                    });
                }
                Picked::Unreadable {
                    pick,
                    name,
                    dropped,
                } if self.is_current(pick) => {
                    self.chosen = None;
                    return Some(Outcome::Unreadable {
                        name,
                        dropped,
                        slot: pick.slot,
                    });
                }
                Picked::File { pick, .. } | Picked::Unreadable { pick, .. } => {
                    tracing::debug!(
                        pick = pick.number,
                        "a read finished after a newer choice or a new A; dropped"
                    );
                }
                Picked::Canceled { .. } => {}
                Picked::Refused { pick } => return Some(Outcome::Refused { slot: pick.slot }),
            }
        }
    }

    fn is_current(&self, pick: Pick) -> bool {
        self.chosen
            .as_ref()
            .is_some_and(|(current, _)| *current == pick)
    }
}

/// The browser-bound half: the file input and the dropped file's reader.
#[cfg(target_arch = "wasm32")]
pub use web::{pick_file, read_dropped};

#[cfg(target_arch = "wasm32")]
mod web {
    use std::sync::mpsc::Sender;

    use eframe::wasm_bindgen::JsCast as _;
    use eframe::wasm_bindgen::closure::Closure;

    use super::{Pick, Picked};

    /// Show the browser's file chooser for one `.bin` and read the file
    /// chosen. The input element is made for this pick alone and never
    /// attached to the page; its handlers hold it until the browser is
    /// done with it. A click the browser would refuse is followed by
    /// nothing, so the activation is checked first and the pick refused
    /// rather than left waiting.
    pub fn pick_file(pick: Pick, tx: &Sender<Picked>, ctx: &egui::Context) {
        let Some(input) = file_input() else {
            tracing::warn!("no document to pick a file with");
            let _ = tx.send(Picked::Refused { pick });
            return;
        };
        let change = {
            let input = input.clone();
            let tx = tx.clone();
            let ctx = ctx.clone();
            Closure::<dyn FnMut()>::new(move || {
                let Some(file) = input.files().and_then(|files| files.get(0)) else {
                    let _ = tx.send(Picked::Canceled { pick });
                    ctx.request_repaint();
                    return;
                };
                read_file(pick, file, tx.clone(), ctx.clone());
            })
        };
        input.set_onchange(Some(change.as_ref().unchecked_ref()));
        change.forget();
        // Every current browser fires `cancel` on a chooser closed without
        // a choice; one that does not leaves nothing waiting, since the
        // app keeps no state for a pick that has not chosen.
        let cancel = {
            let tx = tx.clone();
            let ctx = ctx.clone();
            Closure::<dyn FnMut()>::new(move || {
                let _ = tx.send(Picked::Canceled { pick });
                ctx.request_repaint();
            })
        };
        if input
            .add_event_listener_with_callback("cancel", cancel.as_ref().unchecked_ref())
            .is_ok()
        {
            cancel.forget();
        }
        if may_open_dialog() {
            input.click();
        } else {
            tracing::warn!("the browser would not open the file chooser");
            let _ = tx.send(Picked::Refused { pick });
            ctx.request_repaint();
        }
    }

    /// Read a file dropped on the page: its name at once, its bytes when
    /// the browser has them.
    pub fn read_dropped(
        pick: Pick,
        handle: egui::DroppedFileHandle,
        tx: Sender<Picked>,
        ctx: egui::Context,
    ) {
        let name = handle.path().to_string_lossy().into_owned();
        let _ = tx.send(Picked::Chosen {
            pick,
            name: name.clone(),
        });
        // the drop's own frame polled the picks before the drop was
        // handled, so the name needs a frame of its own
        ctx.request_repaint();
        wasm_bindgen_futures::spawn_local(async move {
            let message = match handle.bytes_async().await {
                Ok(bytes) => Picked::File { pick, name, bytes },
                Err(err) => {
                    tracing::warn!(error = %err, name, "cannot read the dropped file");
                    Picked::Unreadable {
                        pick,
                        name,
                        dropped: true,
                    }
                }
            };
            let _ = tx.send(message);
            ctx.request_repaint();
        });
    }

    /// Read a chosen file: its name at once, its bytes when the browser
    /// has them.
    fn read_file(pick: Pick, file: web_sys::File, tx: Sender<Picked>, ctx: egui::Context) {
        let name = file.name();
        let _ = tx.send(Picked::Chosen {
            pick,
            name: name.clone(),
        });
        ctx.request_repaint();
        wasm_bindgen_futures::spawn_local(async move {
            let read = wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await;
            let message = if let Ok(buffer) = read {
                Picked::File {
                    pick,
                    name,
                    bytes: js_sys::Uint8Array::new(&buffer).to_vec(),
                }
            } else {
                tracing::warn!(name, "cannot read the chosen file");
                Picked::Unreadable {
                    pick,
                    name,
                    dropped: false,
                }
            };
            let _ = tx.send(message);
            ctx.request_repaint();
        });
    }

    /// A detached file input that takes one `.bin`.
    fn file_input() -> Option<web_sys::HtmlInputElement> {
        let input: web_sys::HtmlInputElement = web_sys::window()?
            .document()?
            .create_element("input")
            .ok()?
            .dyn_into()
            .ok()?;
        input.set_type("file");
        input.set_accept(".bin,.BIN");
        Some(input)
    }

    /// Whether the browser would open a chooser now: it takes a click
    /// from code only while the user's own click is recent.
    fn may_open_dialog() -> bool {
        let Some(window) = web_sys::window() else {
            return false;
        };
        let activation = window.navigator().user_activation();
        if activation.is_undefined() {
            return true;
        }
        activation.is_active()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(pick: Pick, name: &str) -> Picked {
        Picked::File {
            pick,
            name: name.to_string(),
            bytes: vec![1, 2, 3],
        }
    }

    fn chosen(pick: Pick, name: &str) -> Picked {
        Picked::Chosen {
            pick,
            name: name.to_string(),
        }
    }

    #[test]
    fn the_newest_choice_wins_and_a_cancel_changes_nothing() {
        let mut picks = Picks::default();
        let (first, tx) = picks.start(Slot::A);
        let (second, _) = picks.start(Slot::A);
        let (third, _) = picks.start(Slot::A);
        assert_eq!((first.number, second.number, third.number), (1, 2, 3));

        tx.send(chosen(first, "a.bin")).unwrap();
        tx.send(chosen(second, "b.bin")).unwrap();
        // a third chooser opened and closed without a choice
        tx.send(Picked::Canceled { pick: third }).unwrap();
        assert!(picks.poll().is_none());
        assert_eq!(
            picks.reading(),
            Some(("b.bin", Slot::A)),
            "the second read is waited on"
        );

        // the older read lands first and is dropped
        tx.send(file(first, "a.bin")).unwrap();
        assert!(picks.poll().is_none());
        assert_eq!(picks.reading(), Some(("b.bin", Slot::A)));

        tx.send(file(second, "b.bin")).unwrap();
        match picks.poll() {
            Some(Outcome::File { name, bytes, slot }) => {
                assert_eq!(name, "b.bin");
                assert_eq!(bytes, [1, 2, 3]);
                assert_eq!(slot, Slot::A);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(picks.reading(), None);
    }

    #[test]
    fn the_slot_reaches_the_outcome() {
        let mut picks = Picks::default();
        let (pick, tx) = picks.start(Slot::B);
        tx.send(chosen(pick, "baseline.bin")).unwrap();
        assert!(picks.poll().is_none());
        assert_eq!(picks.reading(), Some(("baseline.bin", Slot::B)));
        tx.send(file(pick, "baseline.bin")).unwrap();
        assert!(matches!(
            picks.poll(),
            Some(Outcome::File { slot: Slot::B, .. })
        ));

        let (pick, tx) = picks.start(Slot::B);
        tx.send(chosen(pick, "photos")).unwrap();
        tx.send(Picked::Unreadable {
            pick,
            name: "photos".into(),
            dropped: false,
        })
        .unwrap();
        assert!(matches!(
            picks.poll(),
            Some(Outcome::Unreadable { slot: Slot::B, .. })
        ));
        let (pick, tx) = picks.start(Slot::B);
        tx.send(Picked::Refused { pick }).unwrap();
        assert!(matches!(
            picks.poll(),
            Some(Outcome::Refused { slot: Slot::B })
        ));
    }

    #[test]
    fn a_new_a_drops_a_b_read_in_progress() {
        // a newer pick for A wins over B's read, as any newer choice does
        let mut picks = Picks::default();
        let (b, tx) = picks.start(Slot::B);
        let (a, _) = picks.start(Slot::A);
        tx.send(chosen(b, "baseline.bin")).unwrap();
        tx.send(chosen(a, "flight.bin")).unwrap();
        tx.send(file(b, "baseline.bin")).unwrap();
        assert!(picks.poll().is_none(), "B's read is dropped");
        assert_eq!(picks.reading(), Some(("flight.bin", Slot::A)));

        // an A that opens without a pick, as the demo does, drops it too
        let mut picks = Picks::default();
        let (b, tx) = picks.start(Slot::B);
        tx.send(chosen(b, "baseline.bin")).unwrap();
        assert!(picks.poll().is_none());
        picks.drop_reading();
        assert_eq!(picks.reading(), None);
        tx.send(file(b, "baseline.bin")).unwrap();
        assert!(picks.poll().is_none(), "its bytes count for nothing");
    }

    #[test]
    fn a_failed_read_is_reported_for_the_current_choice_only() {
        let mut picks = Picks::default();
        let (first, tx) = picks.start(Slot::A);
        let (second, _) = picks.start(Slot::A);
        tx.send(chosen(first, "a.bin")).unwrap();
        tx.send(chosen(second, "b.bin")).unwrap();
        tx.send(Picked::Unreadable {
            pick: first,
            name: "a.bin".into(),
            dropped: false,
        })
        .unwrap();
        assert!(picks.poll().is_none());
        tx.send(Picked::Unreadable {
            pick: second,
            name: "b.bin".into(),
            dropped: true,
        })
        .unwrap();
        assert!(matches!(
            picks.poll(),
            Some(Outcome::Unreadable { name, dropped: true, .. }) if name == "b.bin"
        ));
        assert_eq!(picks.reading(), None);
    }

    #[test]
    fn a_refusal_is_reported_and_leaves_a_read_alone() {
        let mut picks = Picks::default();
        let (first, tx) = picks.start(Slot::A);
        let (second, _) = picks.start(Slot::A);
        tx.send(chosen(first, "a.bin")).unwrap();
        tx.send(Picked::Refused { pick: second }).unwrap();
        assert!(matches!(picks.poll(), Some(Outcome::Refused { .. })));
        assert_eq!(picks.reading(), Some(("a.bin", Slot::A)));
        tx.send(file(first, "a.bin")).unwrap();
        assert!(matches!(picks.poll(), Some(Outcome::File { .. })));
    }
}
