use std::cell::Cell;
use std::rc::Rc;

use egui::accesskit::Role;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use wordcraft_doc::para::OBJ;
use wordcraft_doc::{Document, Pos, StoryRef};
use wordcraft_engine::{Selection, Session};

use crate::{Services, WordApp};

type Harness = egui_kittest::Harness<'static, WordApp>;

const TYPED: &str = "Diagram of the Q3 architecture";

/// A file of this test's own in the temp folder.
fn temp_docx(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("wordcraft-r3-alt-{tag}-{}.docx", std::process::id()))
}

/// A saved document with an ellipse selected and the Alt Text pane open.
fn shape_app(path: &std::path::Path) -> WordApp {
    let mut a = WordApp::new(Session::new(Document::new()), Services::default());
    a.run("text.insert", json!({"text": "Some text"})).unwrap();
    a.run("insert.shape", json!({"kind": "ellipse"})).unwrap();
    a.run("view.altTextPane", json!({"value": true})).unwrap();
    a.run("file.save", json!({"path": path.to_string_lossy()})).unwrap();
    assert!(!a.session.dirty, "saved");
    a
}

/// A saved document with a table, the caret in its first cell, and the Alt Text pane open.
fn table_app(path: &std::path::Path) -> WordApp {
    table_app_after("", path)
}

/// The same, with a paragraph of `intro` above the table (none when it is empty).
fn table_app_after(intro: &str, path: &std::path::Path) -> WordApp {
    let mut a = WordApp::new(Session::new(Document::new()), Services::default());
    if !intro.is_empty() {
        a.run("text.insert", json!({"text": intro})).unwrap();
    }
    a.run("insert.table", json!({"rows": 2, "cols": 2})).unwrap();
    let cell = a.session.doc.para_paths(StoryRef::Body).into_iter().find(|p| p.cell().is_some()).expect("the table has a cell");
    a.session.sel = Selection::caret(Pos { story: StoryRef::Body, path: cell, off: 0 });
    a.run("view.altTextPane", json!({"value": true})).unwrap();
    a.run("file.save", json!({"path": path.to_string_lossy()})).unwrap();
    assert!(!a.session.dirty, "saved");
    a
}

fn harness(a: WordApp) -> Harness {
    harness_sized(a, egui::vec2(1440.0, 900.0))
}

/// Wide enough for the ribbon's Size group to keep its Alt box beside the pane.
fn wide_harness(a: WordApp) -> Harness {
    harness_sized(a, egui::vec2(1800.0, 900.0))
}

fn harness_sized(a: WordApp, size: egui::Vec2) -> Harness {
    // The pane's fields sit under a label of their own and have no accessible name, so the
    // harness's check for unnamed widgets is off.
    let mut h = egui_kittest::Harness::builder().with_size(size).with_accessibility_check(false).build_ui_state(
        |ui, app: &mut WordApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        a,
    );
    h.run_steps(6);
    h
}

/// The pane's `nth` text field of `role` (a description is multi-line, a table's title is not).
/// The ribbon has text fields too, to the left of the pane.
fn field(h: &Harness, role: Role, nth: usize) -> egui_kittest::Node<'_> {
    h.get_all_by_role(role).filter(|n| n.rect().min.x > 1000.0).nth(nth).expect("the pane has the field")
}

fn shown(h: &Harness, role: Role, nth: usize) -> String {
    field(h, role, nth).value().unwrap_or_default()
}

/// Click into the field and type `text`, leaving the keyboard focus in it.
fn type_in(h: &mut Harness, role: Role, nth: usize, text: &str) {
    field(h, role, nth).focus();
    h.run_steps(2);
    field(h, role, nth).type_text(text);
    h.run_steps(3);
    assert_eq!(shown(h, role, nth), text, "the field shows what was typed");
    assert!(field(h, role, nth).is_focused(), "and keeps the keyboard");
}

/// The alt text of each picture and shape in the body, in order.
fn alt_texts(doc: &Document) -> Vec<String> {
    let mut out = Vec::new();
    for path in doc.para_paths(StoryRef::Body) {
        let Some(p) = doc.para(StoryRef::Body, &path) else { continue };
        for off in p.object_offsets() {
            if let Some(o) = p.object_at(off).filter(|o| o.is_drawing()) {
                out.push(o.alt_text().to_string());
            }
        }
    }
    out
}

/// The title and description of the first table in the body.
fn table_texts(doc: &Document) -> (Option<String>, Option<String>) {
    let (tp, _, _) = doc.para_paths(StoryRef::Body).into_iter().find_map(|p| p.cell()).expect("a table");
    let t = doc.table(StoryRef::Body, &tp).expect("the table");
    (t.props.caption.clone(), t.props.description.clone())
}

fn saved(path: &std::path::Path) -> Document {
    let doc = wordcraft_engine::io::open_path(path);
    let _ = std::fs::remove_file(path);
    doc.expect("the saved file opens")
}

fn drawing_positions(doc: &Document) -> Vec<Pos> {
    let mut out = Vec::new();
    for path in doc.para_paths(StoryRef::Body) {
        let Some(p) = doc.para(StoryRef::Body, &path) else { continue };
        for off in p.object_offsets() {
            if p.object_at(off).is_some_and(|o| o.is_drawing()) {
                out.push(Pos { story: StoryRef::Body, path: path.clone(), off });
            }
        }
    }
    out
}

fn select_object(a: &mut WordApp, pos: &Pos) {
    a.session.sel = Selection { anchor: pos.clone(), focus: Pos { off: pos.off + OBJ.len_utf8(), ..pos.clone() } };
}

const MULTI: Role = Role::MultilineTextInput;
const SINGLE: Role = Role::TextInput;

/// Alt Text typed into the pane is work the page would lose: the host is told at once, so the
/// tab asks before it closes, and quitting asks about the document.
#[test]
fn typed_alt_text_counts_as_unsaved_work() {
    let path = temp_docx("dirty");
    let reported = Rc::new(Cell::new(None));
    let mut a = shape_app(&path);
    let seen = reported.clone();
    a.services.on_dirty = Some(Box::new(move |d| seen.set(Some(d))));
    let mut h = harness(a);
    assert_eq!(reported.get(), Some(false), "nothing to lose yet");
    type_in(&mut h, MULTI, 0, TYPED);
    assert!(h.state().has_unsaved_documents(), "the typed description is unsaved work");
    assert_eq!(reported.get(), Some(true), "the host is told, so closing the tab asks first");
    let _ = std::fs::remove_file(&path);
}

/// Save (Cmd+S, File › Save, the host page's request) while the keyboard is still in the
/// description writes what was typed: the field hasn't lost focus to commit it.
#[test]
fn saving_while_the_alt_text_field_has_focus_writes_what_was_typed() {
    let path = temp_docx("save");
    let mut h = harness(shape_app(&path));
    type_in(&mut h, MULTI, 0, TYPED);
    h.state_mut().run("file.save", json!({})).unwrap();
    assert!(!h.state().session.dirty, "saved");
    assert!(!h.state().has_unsaved_documents(), "and nothing is left to lose");
    assert_eq!(alt_texts(&saved(&path)), [TYPED], "the file has the description");
    h.run_steps(2);
    assert_eq!(shown(&h, MULTI, 0), TYPED, "the field still shows it");
}

/// The Save shortcut once the pane is gone (a field that has the keyboard keeps Mod+S, as the
/// Search field keeps Mod+H): the field never saw itself lose focus, and the description is
/// still written.
#[test]
fn the_save_shortcut_after_hiding_the_pane_writes_what_was_typed() {
    let path = temp_docx("shortcut");
    let mut h = harness(shape_app(&path));
    type_in(&mut h, MULTI, 0, TYPED);
    h.state_mut().run("view.altTextPane", json!({"value": false})).unwrap();
    h.run_steps(3);
    assert!(!h.ctx.text_edit_focused(), "the field is gone, and the keyboard with it");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::S);
    h.run_steps(3);
    assert!(!h.state().has_unsaved_documents(), "the shortcut saved");
    assert_eq!(alt_texts(&saved(&path)), [TYPED]);
}

/// Hiding the pane while the keyboard is in the description doesn't lose it: it is still
/// there when the pane comes back, still unsaved work, and a save has it.
#[test]
fn hiding_the_pane_keeps_the_typed_alt_text() {
    let path = temp_docx("hide");
    let mut h = harness(shape_app(&path));
    type_in(&mut h, MULTI, 0, TYPED);
    h.state_mut().run("view.altTextPane", json!({"value": false})).unwrap();
    h.run_steps(3);
    assert!(h.state().has_unsaved_documents(), "hidden, it is still unsaved work");
    h.state_mut().run("view.altTextPane", json!({"value": true})).unwrap();
    h.run_steps(3);
    assert_eq!(shown(&h, MULTI, 0), TYPED, "the pane shows it again");
    h.state_mut().run("file.save", json!({})).unwrap();
    assert_eq!(alt_texts(&saved(&path)), [TYPED]);
}

/// File (the Backstage) replaces the whole window while the keyboard is in the description:
/// what was typed is there when File closes, and Save from the Backstage has it.
#[test]
fn opening_file_keeps_the_typed_alt_text() {
    let path = temp_docx("backstage");
    let mut h = harness(shape_app(&path));
    type_in(&mut h, MULTI, 0, TYPED);
    h.state_mut().run("ui.backstage", json!({"value": true, "page": "info"})).unwrap();
    h.run_steps(3);
    assert!(h.state().has_unsaved_documents(), "with File open it is still unsaved work");
    h.state_mut().run("ui.backstage", json!({"value": false})).unwrap();
    h.run_steps(3);
    assert_eq!(shown(&h, MULTI, 0), TYPED, "the pane shows it again");
    h.state_mut().run("ui.backstage", json!({"value": true, "page": "info"})).unwrap();
    h.run_steps(3);
    h.get_by_label("Save").click();
    h.run_steps(4);
    assert!(!h.state().has_unsaved_documents());
    assert_eq!(alt_texts(&saved(&path)), [TYPED]);
}

/// Leaving the field the usual way, by clicking on the page, still writes the description to
/// its object as one change.
#[test]
fn leaving_the_alt_text_field_writes_it_to_the_document() {
    let path = temp_docx("leave");
    let mut h = harness(shape_app(&path));
    type_in(&mut h, MULTI, 0, TYPED);
    let at = h.state().canvas.page_rects[0].center();
    h.event(egui::Event::PointerMoved(at));
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
        h.step();
    }
    h.run_steps(3);
    assert_eq!(alt_texts(&h.state().session.doc), [TYPED], "the shape has the description");
    assert!(h.state().session.dirty, "as a change to the document");
    h.state_mut().run("file.save", json!({})).unwrap();
    assert_eq!(alt_texts(&saved(&path)), [TYPED]);
}

/// Close (the window's button, File › Close, Quit) asks Save / Don't Save / Cancel about a
/// description that was typed and not yet written. Cancel keeps it, Don't Save drops it, Save
/// stores it.
#[test]
fn closing_with_typed_alt_text_asks_to_save() {
    let path = temp_docx("close");
    let mut h = harness(shape_app(&path));
    type_in(&mut h, MULTI, 0, TYPED);
    assert!(!h.state_mut().close_requested(), "the typed description is an unsaved change");
    assert!(matches!(h.state().dialog, Some(crate::dialogs::Dialog::SaveChanges { .. })), "so the close asks");

    h.state_mut().run("ui.saveChanges", json!({"answer": "cancel"})).unwrap();
    h.run_steps(3);
    assert!(!h.state().quit_requested && h.state().has_document(), "Cancel keeps the document");
    assert!(h.state().has_unsaved_documents(), "and what was typed");
    assert_eq!(shown(&h, MULTI, 0), TYPED);

    assert!(!h.state_mut().close_requested(), "asked again");
    h.state_mut().run("ui.saveChanges", json!({"answer": "save"})).unwrap();
    assert!(h.state().quit_requested, "Save goes on closing");
    assert_eq!(alt_texts(&saved(&path)), [TYPED], "with the description in the file");
}

#[test]
fn dont_save_drops_typed_alt_text_on_close() {
    let path = temp_docx("dontsave");
    let mut h = harness(shape_app(&path));
    type_in(&mut h, MULTI, 0, TYPED);
    assert!(!h.state_mut().close_requested());
    h.state_mut().run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap();
    assert!(h.state_mut().close_requested(), "Don't Save closes");
    assert_eq!(alt_texts(&saved(&path)), [""], "and the file keeps its old description");
}

/// File › Close (an agent's `file.close` too) asks about it the same way.
#[test]
fn file_close_with_typed_alt_text_asks_to_save() {
    let path = temp_docx("fileclose");
    let mut h = harness(shape_app(&path));
    type_in(&mut h, MULTI, 0, TYPED);
    h.state_mut().run("file.close", json!({})).unwrap();
    assert!(matches!(h.state().dialog, Some(crate::dialogs::Dialog::SaveChanges { .. })), "the close asks");
    assert!(crate::control::close_refused(h.state(), "file.close", &json!({})).is_some(), "an agent has to say force");
    h.state_mut().run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap();
    assert!(!h.state().has_document(), "Don't Save closes the document");
    let _ = std::fs::remove_file(&path);
}

/// A table's title and description are two fields of one table: typed, they are unsaved work,
/// and a save made with the keyboard in the second has both.
#[test]
fn typed_table_alt_text_is_saved_and_counted() {
    let path = temp_docx("table");
    let reported = Rc::new(Cell::new(None));
    let mut a = table_app(&path);
    let seen = reported.clone();
    a.services.on_dirty = Some(Box::new(move |d| seen.set(Some(d))));
    let mut h = harness(a);
    type_in(&mut h, SINGLE, 0, "Q3 figures");
    assert_eq!(reported.get(), Some(true), "the title is unsaved work");
    type_in(&mut h, MULTI, 0, TYPED);
    h.state_mut().run("file.save", json!({})).unwrap();
    assert!(!h.state().has_unsaved_documents());
    assert_eq!(table_texts(&saved(&path)), (Some("Q3 figures".to_string()), Some(TYPED.to_string())));
}

/// The title alone, with the keyboard still in it, and hiding the pane on the way.
#[test]
fn a_table_title_survives_hiding_the_pane() {
    let path = temp_docx("tabletitle");
    let mut h = harness(table_app(&path));
    type_in(&mut h, SINGLE, 0, "Q3 figures");
    h.state_mut().run("view.altTextPane", json!({"value": false})).unwrap();
    h.run_steps(3);
    h.state_mut().run("view.altTextPane", json!({"value": true})).unwrap();
    h.run_steps(3);
    assert_eq!(shown(&h, SINGLE, 0), "Q3 figures");
    h.state_mut().run("file.save", json!({})).unwrap();
    assert_eq!(table_texts(&saved(&path)).0.as_deref(), Some("Q3 figures"));
}

/// A description typed for one picture is written to that picture when another is selected
/// before the save, not dropped and not given to the other one.
#[test]
fn a_draft_is_written_to_its_own_object_when_another_is_selected() {
    let path = temp_docx("two");
    let mut a = shape_app(&path);
    a.run("insert.shape", json!({"kind": "rectangle"})).unwrap();
    a.run("file.save", json!({})).unwrap();
    let objects = drawing_positions(&a.session.doc);
    assert_eq!(objects.len(), 2, "two shapes");
    select_object(&mut a, &objects[0]);
    let mut h = harness(a);
    type_in(&mut h, MULTI, 0, TYPED);
    // The selection moves to the other shape without the field noticing it lost focus.
    select_object(h.state_mut(), &objects[1]);
    h.run_steps(3);
    assert_eq!(shown(&h, MULTI, 0), "", "the pane now describes the other shape");
    assert!(h.state().has_unsaved_documents(), "the first one's description is still unsaved work");
    let selection = h.state().session.sel.clone();
    h.state_mut().run("file.save", json!({})).unwrap();
    assert_eq!(h.state().session.sel, selection, "the selection stays where it was");
    assert_eq!(alt_texts(&saved(&path)), [TYPED, ""], "the first shape has it, the second doesn't");
}

/// A protected (read-only) document can't take alt text, so its fields can't be typed in.
#[test]
fn alt_text_fields_are_read_only_where_alt_text_cant_change() {
    let path = temp_docx("readonly");
    let mut a = shape_app(&path);
    a.session.doc.settings.protection = Some("readOnly".into());
    let mut h = harness(a);
    field(&h, MULTI, 0).focus();
    h.run_steps(2);
    field(&h, MULTI, 0).type_text("Typed in vain");
    h.run_steps(3);
    assert_eq!(shown(&h, MULTI, 0), "");
    assert!(!h.state().has_unsaved_documents());
    let _ = std::fs::remove_file(&path);
}

/// A description the document refuses (it became read-only after it was typed) is kept and
/// counts as unsaved work; saving the rest of the document goes on and says why.
#[test]
fn a_refused_alt_text_draft_is_kept_and_a_save_goes_on() {
    let path = temp_docx("refused");
    let mut h = harness(shape_app(&path));
    type_in(&mut h, MULTI, 0, TYPED);
    h.state_mut().session.doc.settings.protection = Some("readOnly".into());
    h.state_mut().run("file.save", json!({})).unwrap();
    assert!(h.state().has_unsaved_documents(), "the refused description is kept");
    let status = h.state().status_msg.as_ref().map(|(s, _)| s.clone()).unwrap_or_default();
    assert!(status.contains("protected"), "the refusal is shown: {status:?}");
    assert!(!h.state_mut().close_requested(), "closing asks about it");
    h.state_mut().run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap();
    assert!(h.state_mut().close_requested(), "Don't Save drops it");
    let _ = std::fs::remove_file(&path);
}

/// What is typed belongs to its document: another document never shows it or counts it as its
/// own, and it is back with its document when that is shown again.
#[test]
fn typed_alt_text_stays_with_its_document() {
    let path = temp_docx("document");
    let mut a = shape_app(&path);
    // The web app's one window shows each document in turn.
    a.windows.single();
    let first = a.session.document_id();
    a.run("insert.shape", json!({"kind": "rectangle"})).unwrap();
    a.run("file.save", json!({})).unwrap();
    let objects = drawing_positions(&a.session.doc);
    select_object(&mut a, &objects[0]);
    let mut h = harness(a);
    type_in(&mut h, MULTI, 0, TYPED);
    // A second document with a shape at the same position.
    h.state_mut().run("file.new", json!({})).unwrap();
    h.state_mut().run("text.insert", json!({"text": "Some text"})).unwrap();
    h.state_mut().run("insert.shape", json!({"kind": "ellipse"})).unwrap();
    h.state_mut().run("view.altTextPane", json!({"value": true})).unwrap();
    h.state_mut().session.dirty = false;
    h.run_steps(4);
    assert_ne!(h.state().session.document_id(), first);
    assert_eq!(shown(&h, MULTI, 0), "", "the other document's shape doesn't show the description");
    assert!(h.state().has_unsaved_documents(), "the first document's description still counts");
    assert!(h.state().session.documents().iter().all(|d| !d.dirty), "though neither document has unsaved changes of its own");
    h.state_mut().run("view.switchWindows", json!({"id": first})).unwrap();
    h.run_steps(4);
    assert_eq!(shown(&h, MULTI, 0), TYPED, "it is back with its document");
    let _ = std::fs::remove_file(&path);
}

/// A saved document with an ellipse and a rectangle after its text, the ellipse selected and the
/// Alt Text pane open; the drawings' positions.
fn two_shapes_app(path: &std::path::Path) -> (WordApp, Vec<Pos>) {
    let mut a = shape_app(path);
    a.run("insert.shape", json!({"kind": "rectangle"})).unwrap();
    a.run("file.save", json!({})).unwrap();
    let objects = drawing_positions(&a.session.doc);
    assert_eq!(objects.len(), 2, "two shapes");
    select_object(&mut a, &objects[0]);
    (a, objects)
}

/// A saved document holding a picture, selected, with the Picture Format tab shown.
fn picture_app(path: &std::path::Path) -> WordApp {
    let mut a = WordApp::new(Session::new(Document::new()), Services::default());
    let pixels = image::RgbaImage::from_pixel(20, 10, image::Rgba([200, 30, 30, 255]));
    let mut png = Vec::new();
    pixels.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    a.run("insert.picture", json!({"data": wordcraft_engine::cmd::insert::base64_encode(&png)})).unwrap();
    a.run("ui.tab", json!({"tab": "Picture Format"})).unwrap();
    a.run("file.save", json!({"path": path.to_string_lossy()})).unwrap();
    assert!(!a.session.dirty, "saved");
    a
}

/// The Alt box in Picture Format's Size group: a one-line field with the hint "Alt text".
fn ribbon_alt(h: &Harness) -> egui_kittest::Node<'_> {
    h.get_all_by_role(SINGLE).find(|n| n.accesskit_node().data().placeholder() == Some("Alt text")).expect("the Size group has the Alt box")
}

/// Click into the ribbon's Alt box and type `text`, leaving the keyboard focus in it.
fn type_in_ribbon(h: &mut Harness, text: &str) {
    ribbon_alt(h).focus();
    h.run_steps(2);
    ribbon_alt(h).type_text(text);
    h.run_steps(3);
    assert_eq!(ribbon_alt(h).value().unwrap_or_default(), text, "the box shows what was typed");
    assert!(ribbon_alt(h).is_focused(), "and keeps the keyboard");
}

/// A description typed for a drawing names its position until it is written, and typing before
/// the drawing moves it: the description goes with its drawing, not lost with the position.
#[test]
fn alt_text_typed_for_a_drawing_goes_with_it_when_text_is_inserted_before_it() {
    let path = temp_docx("insert-before");
    let (a, objects) = two_shapes_app(&path);
    let mut h = harness(a);
    type_in(&mut h, MULTI, 0, TYPED);
    h.state_mut().run("view.altTextPane", json!({"value": false})).unwrap();
    h.run_steps(3);
    h.state_mut().session.sel = Selection::caret(Pos { off: 0, ..objects[0].clone() });
    h.state_mut().run("text.insert", json!({"text": "prefix "})).unwrap();
    let moved = drawing_positions(&h.state().session.doc);
    assert_eq!(moved[0].off, objects[0].off + "prefix ".len(), "both drawings moved right");
    assert!(!super::has_alt_text_drafts(h.state()), "the description was entered before the text, with its drawing");
    h.state_mut().run("file.save", json!({})).unwrap();
    assert!(!h.state().has_unsaved_documents());
    assert_eq!(alt_texts(&saved(&path)), [TYPED, ""], "the ellipse has it, the rectangle doesn't");
}

/// Deleting the drawing a description was typed for moves the next drawing into its position:
/// the description is not given to it.
#[test]
fn alt_text_typed_for_a_deleted_drawing_is_not_given_to_the_one_that_takes_its_place() {
    let path = temp_docx("delete-owner");
    let (a, _) = two_shapes_app(&path);
    let mut h = harness(a);
    type_in(&mut h, MULTI, 0, TYPED);
    h.state_mut().run("view.altTextPane", json!({"value": false})).unwrap();
    h.run_steps(3);
    h.state_mut().run("text.delete", json!({})).unwrap();
    assert_eq!(drawing_positions(&h.state().session.doc).len(), 1, "the ellipse is gone");
    assert!(!super::has_alt_text_drafts(h.state()), "the description went with it");
    h.state_mut().run("file.save", json!({})).unwrap();
    assert_eq!(alt_texts(&saved(&path)), [""], "the rectangle that took its position has no description");
}

/// A table is named by its path, which a paragraph inserted above it changes.
#[test]
fn a_table_title_typed_for_a_table_goes_with_it_when_a_paragraph_is_inserted_above() {
    let path = temp_docx("table-shift");
    let mut h = harness(table_app_after("Intro", &path));
    type_in(&mut h, SINGLE, 0, "Q3 figures");
    h.state_mut().run("view.altTextPane", json!({"value": false})).unwrap();
    h.run_steps(3);
    let first = h.state().session.doc.para_paths(StoryRef::Body).into_iter().next().expect("a paragraph");
    assert!(first.cell().is_none(), "the table has a paragraph above it");
    h.state_mut().session.sel = Selection::caret(Pos { story: StoryRef::Body, path: first, off: 0 });
    h.state_mut().run("text.newParagraph", json!({})).unwrap();
    assert!(!super::has_alt_text_drafts(h.state()), "the title was entered before the paragraph, with its table");
    h.state_mut().run("file.save", json!({})).unwrap();
    assert_eq!(table_texts(&saved(&path)).0.as_deref(), Some("Q3 figures"));
}

/// The Alt box in Picture Format's Size group is a second way to type a picture's description,
/// and typed there it is unsaved work the page would lose.
#[test]
fn typed_ribbon_alt_text_counts_as_unsaved_work() {
    let path = temp_docx("ribbon-dirty");
    let reported = Rc::new(Cell::new(None));
    let mut a = picture_app(&path);
    let seen = reported.clone();
    a.services.on_dirty = Some(Box::new(move |d| seen.set(Some(d))));
    let mut h = wide_harness(a);
    assert_eq!(reported.get(), Some(false), "nothing to lose yet");
    type_in_ribbon(&mut h, TYPED);
    assert!(h.state().has_unsaved_documents(), "the typed description is unsaved work");
    assert_eq!(reported.get(), Some(true), "the host is told, so closing the tab asks first");
    let _ = std::fs::remove_file(&path);
}

/// Save with the keyboard still in the ribbon's box writes what was typed.
#[test]
fn saving_while_the_ribbon_alt_box_has_focus_writes_what_was_typed() {
    let path = temp_docx("ribbon-save");
    let mut h = wide_harness(picture_app(&path));
    type_in_ribbon(&mut h, TYPED);
    h.state_mut().run("file.save", json!({})).unwrap();
    assert!(!h.state().session.dirty, "saved");
    assert!(!h.state().has_unsaved_documents(), "and nothing is left to lose");
    assert_eq!(alt_texts(&saved(&path)), [TYPED], "the file has the description");
}

/// Leaving the tab hides the box without it noticing it lost the keyboard; what was typed stays.
#[test]
fn the_ribbon_alt_text_survives_leaving_the_tab() {
    let path = temp_docx("ribbon-tab");
    let mut h = wide_harness(picture_app(&path));
    type_in_ribbon(&mut h, TYPED);
    h.state_mut().run("ui.tab", json!({"tab": "Home"})).unwrap();
    h.run_steps(3);
    assert!(h.state().has_unsaved_documents(), "it is still unsaved work");
    h.state_mut().run("file.save", json!({})).unwrap();
    assert_eq!(alt_texts(&saved(&path)), [TYPED]);
}

/// Close asks Save / Don't Save / Cancel about it: Cancel keeps it, Don't Save drops it.
#[test]
fn closing_with_ribbon_alt_text_asks_to_save() {
    let path = temp_docx("ribbon-close");
    let mut h = wide_harness(picture_app(&path));
    type_in_ribbon(&mut h, TYPED);
    assert!(!h.state_mut().close_requested(), "the typed description is an unsaved change");
    assert!(matches!(h.state().dialog, Some(crate::dialogs::Dialog::SaveChanges { .. })), "so the close asks");
    h.state_mut().run("ui.saveChanges", json!({"answer": "cancel"})).unwrap();
    h.run_steps(3);
    assert!(h.state().has_document() && h.state().has_unsaved_documents(), "Cancel keeps the document and what was typed");
    assert_eq!(ribbon_alt(&h).value().unwrap_or_default(), TYPED);
    assert!(!h.state_mut().close_requested(), "asked again");
    h.state_mut().run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap();
    assert!(h.state_mut().close_requested(), "Don't Save closes");
    assert_eq!(alt_texts(&saved(&path)), [""], "and the file keeps its old description");
}

/// One description has one draft, whichever box it is typed in: the ribbon's and the pane's show
/// each other's text.
#[test]
fn the_ribbon_and_the_pane_edit_one_alt_text_draft() {
    let path = temp_docx("ribbon-pane");
    let mut a = picture_app(&path);
    a.run("view.altTextPane", json!({"value": true})).unwrap();
    let mut h = wide_harness(a);
    type_in(&mut h, MULTI, 0, "From the pane");
    assert_eq!(ribbon_alt(&h).value().unwrap_or_default(), "From the pane", "the ribbon's box shows what the pane has");
    assert_eq!(h.state().alt_text_drafts.fields.len(), 1, "one draft");
    let _ = std::fs::remove_file(&path);

    let path = temp_docx("ribbon-pane2");
    let mut a = picture_app(&path);
    a.run("view.altTextPane", json!({"value": true})).unwrap();
    let mut h = wide_harness(a);
    type_in_ribbon(&mut h, "From the ribbon");
    assert_eq!(shown(&h, MULTI, 0), "From the ribbon", "the pane shows what the ribbon's box has");
    assert_eq!(h.state().alt_text_drafts.fields.len(), 1, "one draft");
    h.state_mut().run("file.save", json!({})).unwrap();
    assert_eq!(alt_texts(&saved(&path)), ["From the ribbon"]);
}

/// A script's edit addressed to a document that is not shown would move the drawings its typed
/// Alt Text names behind its back: it waits until the document is shown, where the edit writes the
/// description first. Queries go on.
#[test]
fn an_edit_addressed_to_a_background_document_with_typed_alt_text_waits_for_its_window() {
    use crate::control::{ControlRequest, Outcome, handle};
    let path = temp_docx("background");
    let mut a = shape_app(&path);
    // The web app's one window shows each document in turn.
    a.windows.single();
    let first = a.session.document_id();
    let mut h = harness(a);
    type_in(&mut h, MULTI, 0, TYPED);
    h.state_mut().run("file.new", json!({})).unwrap();
    h.run_steps(4);
    assert_ne!(h.state().session.document_id(), first, "another document is shown");
    let ctx = h.ctx.clone();
    let send = |h: &mut Harness, id: &str| {
        let (req, _reply) = ControlRequest::new("engine.execute", json!({"command": id, "params": {"text": "x"}, "doc": first}));
        match handle(h.state_mut(), &ctx, &req) {
            Outcome::Done(v) => v,
            _ => panic!("a reply"),
        }
    };
    let refused = send(&mut h, "text.insert");
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(refused["error"].as_str().is_some_and(|e| e.contains("Alt Text")), "{refused}");
    assert!(h.state().has_unsaved_documents(), "the description is still there");
    assert_eq!(send(&mut h, "document.inspect")["ok"], true, "a query moves nothing");

    h.state_mut().run("view.switchWindows", json!({"id": first})).unwrap();
    h.run_steps(4);
    let at = drawing_positions(&h.state().session.doc)[0].clone();
    h.state_mut().session.sel = Selection::caret(Pos { off: 0, ..at });
    h.state_mut().run("text.insert", json!({"text": "x"})).unwrap();
    h.state_mut().run("file.save", json!({})).unwrap();
    assert_eq!(alt_texts(&saved(&path)), [TYPED], "shown, the edit wrote the description first");
}
