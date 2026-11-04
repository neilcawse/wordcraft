//! The `postMessage` bridge to a same-origin parent page (`?host=parent`), for pages that embed
//! WordCraft in an `<iframe>` and keep its files (a Nextcloud app, a personal file server, the
//! Craft Office extension). The same messages as GridCraft's and PdfCraft's bridges; each is a
//! plain object with a `type`:
//!
//! - `wordcraft:ready` (to the parent): `{ version }`, sent once the app is running and
//!   listening; send a file now.
//! - `wordcraft:open` (from the parent): `{ name, bytes }`, bytes an `ArrayBuffer` or
//!   `Uint8Array`. It opens like a document picked with File › Open or dropped on the window.
//! - `wordcraft:opened` (to the parent): `{ name, docId }` once a sent file has been opened, in
//!   the order they were sent: `docId` is the document it became, as the open reported it. It
//!   is `null` when the parent must never write that document over the file it sent: the file
//!   didn't open, it waits behind a password prompt (after which it counts as a document the
//!   user opened), or it is an import-only Word 97–2003 file (Save writes a new .docx).
//! - `wordcraft:save` (to the parent): `{ name, saveAs, bytes, docId, saveId }` (`bytes` a
//!   transferred `ArrayBuffer`) for every file WordCraft would otherwise download. The parent
//!   stores it and reports failures itself. `docId` is the document the bytes were made from,
//!   taken when they were made (`null`: no document's own file), the same number
//!   `wordcraft:opened` gave for a sent file, so the parent writes it back only to that file.
//!   `saveId` numbers the saves. `saveAs` (always there) says whether the file becomes the
//!   document's file under a name the parent must ask for:
//!   - Save: `false`. It writes the opened file under its own name and format, as the desktop
//!     app does. A name the parent never sent (an untitled document, an imported .doc saved as
//!     .docx) is a new file: `true`, as for Save As.
//!   - Save As, and File › Export to another document format (which runs Save As): `true` for a
//!     format that keeps everything (.docx and the other Word packages, .odt, .rtf). The document
//!     is now that file; the next Save sends its name with `false`.
//!   - Save As or Export to a format that doesn't (PDF, Markdown, plain text), Export as PDF and
//!     Export Page as PNG: `false`. They are copies; the document's file stays the same, and a
//!     parent that takes `saveAs` as "this is the document's new file" must not rebind it.
//! - `wordcraft:saved` (from the parent, with `?ack=1`): `{ saveId, name }`, the save is stored
//!   as `name`. Only then does a Save or Save As that keeps everything mark the document saved,
//!   as it was when it was saved (later edits stay unsaved), and make `name` its file. Without
//!   `?ack=1` the document counts as saved once it is handed over. Meanwhile a Save Changes
//!   prompt's Save doesn't close or replace the document: the status line says it is waiting
//!   for the save, and the user repeats the command once it is stored.
//! - `wordcraft:discard` (from the parent, with `?ack=1`): `{ saveId }`, the parent will never
//!   store that save (the picker was cancelled, the write failed, a newer save replaced it). The
//!   document stays unsaved. At most [`crate::unacked::MAX_WAITING`] saves wait for an answer.
//! - `wordcraft:dirty` (to the parent): `{ dirty }` whenever closing the page starts or stops
//!   losing work: unsaved changes in any open document, comment text not yet written to one, or
//!   text an input method is composing.
//! - `wordcraft:request-save` (from the parent): save the document as File › Save would, so a
//!   parent's own Save button or shortcut gets a `wordcraft:save` back.
//! - `wordcraft:failed` (to the parent): `{ error }` when the app couldn't start.
//!
//! Only the parent window on the page's own origin is heard or answered. In host mode the parent
//! guards leaving the page, and `?author=` names the user for comments and tracked changes.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use wasm_bindgen::JsCast as _;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wordcraft_ui_egui::{HostSave, Services, WordApp};

use crate::unacked::Unacked;

/// The longest user name taken from `?author=`.
const MAX_AUTHOR_CHARS: usize = 200;

/// A file's name and bytes.
type File = (String, Vec<u8>);

#[derive(Clone)]
pub struct Bridge {
    window: web_sys::Window,
    parent: web_sys::Window,
    origin: String,
    /// `?ack=1`: a document counts as saved when the parent says it stored it (`wordcraft:saved`),
    /// not when it is handed over.
    ack: bool,
    /// Files the parent sent that the app hasn't opened yet.
    sent: Rc<RefCell<VecDeque<File>>>,
    /// Saves the parent stored since the last frame: `(saveId, name)`.
    stored: Rc<RefCell<Vec<(u64, String)>>>,
    unacked: Rc<RefCell<Unacked>>,
    next_save_id: Rc<Cell<u64>>,
    /// The parent asked for a save (`wordcraft:request-save`) since the last frame.
    save_requested: Rc<Cell<bool>>,
}

impl Bridge {
    /// `None` when the page isn't framed (there is no parent to talk to), or when the parent is
    /// on another origin: messages only go to this origin, so the browser would drop every save
    /// without an error while the document showed as saved. `ack`: `?ack=1`.
    pub fn new(ack: bool) -> Option<Self> {
        let window = web_sys::window()?;
        let parent = window.parent().ok().flatten()?;
        if js_sys::Object::is(&parent, &window) {
            return None;
        }
        let origin = window.location().origin().ok()?;
        // An opaque origin (a `file://` page in some browsers) can't be a `postMessage` target.
        if origin == "null" {
            return None;
        }
        // Reading another origin's location throws, which is `None` too.
        if parent.location().origin().ok()? != origin {
            return None;
        }
        Some(Self {
            window,
            parent,
            origin,
            ack,
            sent: Rc::default(),
            stored: Rc::default(),
            unacked: Rc::default(),
            next_save_id: Rc::new(Cell::new(1)),
            save_requested: Rc::default(),
        })
    }

    /// Hand saved and exported files to the parent instead of downloading them, report unsaved
    /// changes to it, and hear the files it sends.
    pub fn connect(&self, services: &mut Services, ctx: egui::Context) {
        self.listen(ctx);
        // Save, Save As and Export go to the parent at once, with the document they came from,
        // so a failed hand-off fails the save.
        let bridge = self.clone();
        services.host_save = Some(Box::new(move |save: &HostSave<'_>| bridge.hand_over(save)));
        services.host_confirms_saves = self.ack;
        // Anything else that would download is a new file of no document's: the parent asks
        // where it goes and never writes it over a document's file.
        let bridge = self.clone();
        services.download = Some(Box::new(move |name: &str, bytes: &[u8]| {
            let save_id = bridge.take_save_id();
            bridge.post_save(name, bytes, true, None, save_id)
        }));
        // The browser's save picker is never used, so no save finishes later.
        services.async_saves = None;
        let bridge = self.clone();
        let reported: Rc<Cell<Option<bool>>> = Rc::default();
        services.on_dirty = Some(Box::new(move |dirty: bool| {
            if reported.get() != Some(dirty) && bridge.send(&[("type", "wordcraft:dirty".into()), ("dirty", dirty.into())]) {
                reported.set(Some(dirty));
            }
        }));
    }

    /// `?author=`: the host's name for the user signs comments and tracked changes.
    pub fn set_author(app: &mut WordApp, author: &str) {
        let author: String = author.trim().chars().take(MAX_AUTHOR_CHARS).collect();
        if !author.is_empty() {
            app.session.author = author;
        }
    }

    pub fn post_ready(&self) {
        self.send(&[("type", "wordcraft:ready".into()), ("version", env!("CARGO_PKG_VERSION").into())]);
    }

    /// The app couldn't start (no WebGPU or WebGL2, for example): let the parent say so.
    pub fn post_failed(&self, error: &str) {
        self.send(&[("type", "wordcraft:failed".into()), ("error", error.into())]);
    }

    /// After each frame's logic: a save the parent asked for runs, stored saves mark their
    /// document saved, and the files the parent sent open, each answered with the document it
    /// became. Anything that changed unsaved work is reported on the next frame, asked for here.
    pub fn sync(&self, app: &mut WordApp, ctx: &egui::Context) {
        let mut changed = false;
        if self.save_requested.take() {
            changed = true;
            if let Err(e) = app.run("file.save", serde_json::json!({})) {
                app.status(e);
            }
        }
        changed |= self.apply_stored(app);
        loop {
            // Not borrowed while the file opens.
            let next = self.sent.borrow_mut().pop_front();
            let Some((name, bytes)) = next else { break };
            changed = true;
            let doc = app.open_sent_file(&name, &bytes);
            self.send(&[("type", "wordcraft:opened".into()), ("name", name.as_str().into()), ("docId", id_value(doc))]);
        }
        if changed {
            ctx.request_repaint();
        }
    }

    /// Post a Save, Save As or Export. With `?ack=1` a save that marks the document saved waits
    /// for the parent to store it.
    fn hand_over(&self, save: &HostSave<'_>) -> Result<(), String> {
        let save_id = self.take_save_id();
        self.post_save(save.name, save.bytes, save.save_as, Some(save.document), save_id)?;
        if self.ack && save.marks_saved {
            self.unacked.borrow_mut().push(save_id, save.document, save.rev);
        }
        Ok(())
    }

    /// A stored save makes `name` its document's file and marks the document saved if it hasn't
    /// changed since (later edits stay unsaved). Older saves of that document won't be stored
    /// after a newer one, and saves of closed documents are forgotten. Whether any was stored.
    fn apply_stored(&self, app: &mut WordApp) -> bool {
        let stored = std::mem::take(&mut *self.stored.borrow_mut());
        let any = !stored.is_empty();
        let mut unacked = self.unacked.borrow_mut();
        for (save_id, name) in stored {
            let Some((doc, rev)) = unacked.stored(save_id) else { continue };
            let marked = app.session.with_document(doc, |s| {
                s.path = Some(std::path::PathBuf::from(&name));
                if s.rev() == rev {
                    s.dirty = false;
                }
            });
            if marked.is_err() {
                log::info!("{name} is stored, but its document was closed");
            }
        }
        unacked.retain_open(|doc| app.session.is_open(doc));
        any
    }

    fn take_save_id(&self) -> u64 {
        let id = self.next_save_id.get();
        self.next_save_id.set(id.saturating_add(1));
        id
    }

    fn post_save(&self, path: &str, bytes: &[u8], save_as: bool, doc: Option<u64>, save_id: u64) -> Result<(), String> {
        let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
        let buffer = js_sys::Uint8Array::from(bytes).buffer();
        let message = message(&[
            ("type", "wordcraft:save".into()),
            ("name", name.as_str().into()),
            ("saveAs", save_as.into()),
            ("bytes", buffer.clone().into()),
            ("docId", id_value(doc)),
            ("saveId", id_value(Some(save_id))),
        ])
        .ok_or_else(|| "couldn't build the message".to_string())?;
        self.parent
            .post_message_with_transfer(&message, &self.origin, &js_sys::Array::of1(&buffer))
            .map_err(|e| e.as_string().unwrap_or_else(|| format!("{e:?}")))
    }

    /// Hear `wordcraft:open` (opened after the frame by [`Self::sync`]), `wordcraft:saved`,
    /// `wordcraft:discard` and `wordcraft:request-save` from the parent.
    fn listen(&self, ctx: egui::Context) {
        let parent = self.parent.clone();
        let origin = self.origin.clone();
        let save_requested = Rc::clone(&self.save_requested);
        let sent = Rc::clone(&self.sent);
        let stored = Rc::clone(&self.stored);
        let unacked = Rc::clone(&self.unacked);
        let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
            if event.origin() != origin || !event.source().is_some_and(|s| js_sys::Object::is(&s, &parent)) {
                return;
            }
            let data = event.data();
            match string(&data, "type").as_deref() {
                Some("wordcraft:open") => {
                    let name = string(&data, "name").filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Document.docx".to_string());
                    let Some(bytes) = bytes(&data) else { return };
                    sent.borrow_mut().push_back((name, bytes));
                }
                Some("wordcraft:saved") => {
                    let Some(save_id) = number(&data, "saveId") else { return };
                    let Some(name) = string(&data, "name").filter(|n| !n.trim().is_empty()) else { return };
                    stored.borrow_mut().push((save_id, name));
                }
                // Nothing to show: the document was unsaved and stays so.
                Some("wordcraft:discard") => {
                    if let Some(save_id) = number(&data, "saveId") {
                        unacked.borrow_mut().discard(save_id);
                    }
                    return;
                }
                Some("wordcraft:request-save") => save_requested.set(true),
                _ => return,
            }
            ctx.request_repaint();
        });
        if self.window.add_event_listener_with_callback("message", on_message.as_ref().unchecked_ref()).is_ok() {
            // The listener lives as long as the page.
            on_message.forget();
        }
    }

    /// Post a message to the parent; false if it couldn't be sent.
    fn send(&self, fields: &[(&str, JsValue)]) -> bool {
        message(fields).is_some_and(|m| self.parent.post_message(&m, &self.origin).is_ok())
    }
}

/// A document id or save number as a JS number (ids stay far below 2^53), `null` for none.
fn id_value(id: Option<u64>) -> JsValue {
    id.map_or(JsValue::NULL, |id| JsValue::from_f64(id as f64))
}

fn message(fields: &[(&str, JsValue)]) -> Option<js_sys::Object> {
    let m = js_sys::Object::new();
    for (key, value) in fields {
        js_sys::Reflect::set(&m, &JsValue::from_str(key), value).ok()?;
    }
    Some(m)
}

fn string(data: &JsValue, key: &str) -> Option<String> {
    js_sys::Reflect::get(data, &JsValue::from_str(key)).ok()?.as_string()
}

/// A whole, non-negative number field.
fn number(data: &JsValue, key: &str) -> Option<u64> {
    let n = js_sys::Reflect::get(data, &JsValue::from_str(key)).ok()?.as_f64()?;
    (n >= 0.0 && n.fract() == 0.0 && n < 9_007_199_254_740_992.0).then_some(n as u64)
}

/// `bytes` as an `ArrayBuffer` or `Uint8Array`.
fn bytes(data: &JsValue) -> Option<Vec<u8>> {
    let bytes = js_sys::Reflect::get(data, &JsValue::from_str("bytes")).ok()?;
    if let Some(array) = bytes.dyn_ref::<js_sys::Uint8Array>() {
        Some(array.to_vec())
    } else if bytes.is_instance_of::<js_sys::ArrayBuffer>() {
        Some(js_sys::Uint8Array::new(&bytes).to_vec())
    } else {
        None
    }
}
