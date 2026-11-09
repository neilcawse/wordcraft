//! The Accessibility pane (the checker's findings, grouped, each a jump and maybe a fix) and the
//! Alt Text pane (the selected object's or table's alternative text).

use std::collections::BTreeMap;

use egui::Ui;
use serde_json::{Value, json};
use wordcraft_doc::para::OBJ;
use wordcraft_doc::{Path, Pos, StoryRef};
use wordcraft_engine::Selection;

use crate::WordApp;
use crate::theme::{regular, semibold};

/// Review › Check Accessibility: the findings for the document as it is now.
pub fn accessibility(app: &mut WordApp, ui: &mut Ui) {
    let issues = issues(app, ui);
    if issues.is_empty() {
        ui.label(egui::RichText::new(tl!("No accessibility issues found.")).weak());
        return;
    }
    let mut picked: Option<(Value, Option<Value>)> = None;
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (kind, title) in [("error", "Errors ({n})"), ("warning", "Warnings ({n})"), ("tip", "Tips ({n})")] {
            let group: Vec<&Value> = issues.iter().filter(|i| i["kind"] == kind).collect();
            if group.is_empty() {
                continue;
            }
            let heading = crate::i18n::fmt(tl!(title), &[("n", &group.len().to_string())]);
            egui::CollapsingHeader::new(egui::RichText::new(heading).font(semibold(13.0))).id_salt(("a11y", kind)).default_open(true).show(
                ui,
                |ui| {
                    for (k, i) in group.into_iter().enumerate() {
                        ui.push_id(k, |ui| {
                            let text = i["issue"].as_str().unwrap_or("");
                            let r = ui.add(egui::Button::new(egui::RichText::new(text).font(regular(12.5))).frame(false).wrap());
                            if let Some(fix) = i["fix"].as_str() {
                                ui.label(egui::RichText::new(fix).small().weak());
                            }
                            if r.clicked() {
                                picked = Some((i.clone(), None));
                            }
                            if let Some(a) = i.get("action").filter(|a| a.is_object())
                                && let Some(label) = a["label"].as_str()
                                && ui.small_button(tl!(label)).clicked()
                            {
                                picked = Some((i.clone(), Some(a.clone())));
                            }
                            ui.add_space(4.0);
                        });
                    }
                },
            );
        }
    });
    if let Some((issue, action)) = picked {
        go_to(app, &issue);
        if let Some(a) = action
            && let Some(cmd) = a["command"].as_str()
        {
            let _ = app.run(cmd, a.get("params").cloned().unwrap_or_else(|| json!({})));
        }
    }
}

/// The checker's findings, kept until the document changes.
fn issues(app: &mut WordApp, ui: &Ui) -> Vec<Value> {
    let key = egui::Id::unique("a11y_issues");
    let stamp = (app.session.document_id(), app.session.rev());
    if let Some((s, v)) = ui.data(|d| d.get_temp::<((u64, u64), Vec<Value>)>(key))
        && s == stamp
    {
        return v;
    }
    let v = wordcraft_engine::cmd::accessibility::check(&app.session.doc);
    ui.data_mut(|d| d.insert_temp(key, (stamp, v.clone())));
    v
}

/// Select the object an issue is about, or put the caret where it is.
fn go_to(app: &mut WordApp, issue: &Value) {
    if let Some(sel) = issue.get("select").filter(|s| s.is_object()) {
        let _ = app.run("select.range", sel.clone());
    } else if let Some(pos) = issue.get("pos").filter(|p| p.is_object()) {
        let _ = app.run("caret.set", json!({"pos": pos}));
    }
    app.canvas.want_focus = true;
}

/// Alt Text: the selected picture's, shape's, chart's or group's description and decorative mark,
/// or the title and description of the table the caret is in.
pub fn alt_text(app: &mut WordApp, ui: &mut Ui) {
    if let Some((pos, obj)) = wordcraft_engine::cmd::objects::selected(&app.session) {
        let decorative = obj.is_decorative();
        ui.label(egui::RichText::new(tl!("Describe what this object shows, for people who can't see it.")).weak());
        ui.add_space(4.0);
        // Read-only where Alt Text can't change (a protected document), so nothing is typed in vain.
        let editable = !decorative && crate::widgets::enabled(app, "object.altText");
        draft_box(app, ui, AltField::Object(pos), obj.alt_text(), editable, true);
        ui.add_space(6.0);
        let mut d = decorative;
        if ui.checkbox(&mut d, tl!("Mark as decorative")).changed() {
            let _ = app.run("object.altText", json!({"decorative": d}));
        }
        ui.label(egui::RichText::new(tl!("Decorative objects only add visual interest: screen readers skip them.")).small().weak());
        return;
    }
    let focus = app.session.sel.focus.clone();
    let table = focus
        .path
        .cell()
        .and_then(|(tp, _, _)| app.session.doc.table(focus.story, &tp).map(|t| (tp, t.props.caption.clone(), t.props.description.clone())));
    if let Some((tp, caption, description)) = table {
        let editable = crate::widgets::enabled(app, "table.altText");
        ui.label(egui::RichText::new(tl!("Describe what this table shows, for people who can't see it.")).weak());
        ui.add_space(4.0);
        ui.label(tl!("Title:"));
        draft_box(app, ui, AltField::TableTitle(focus.story, tp.clone()), caption.as_deref().unwrap_or(""), editable, false);
        ui.add_space(4.0);
        ui.label(tl!("Description:"));
        draft_box(app, ui, AltField::TableText(focus.story, tp), description.as_deref().unwrap_or(""), editable, true);
        return;
    }
    ui.label(egui::RichText::new(tl!("Select a picture, shape, chart or table to describe it.")).weak());
}

/// What an Alt Text field edits, and so what its draft is kept under: an object is named by its
/// position, a table by its story and its path.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum AltField {
    /// The description of the picture, shape, text box, chart or group at a position.
    Object(Pos),
    /// A table's title.
    TableTitle(StoryRef, Path),
    /// A table's description.
    TableText(StoryRef, Path),
}

/// What has been typed in the Alt Text pane's fields but not yet written to the document, as
/// [`crate::panes::CommentDrafts`] does for comments. A field writes its text back when it loses
/// focus, but the keyboard can still be in it when the page closes, when a Save is made, when
/// the pane is hidden or File opens, and a field that is not drawn notices nothing. So the draft
/// is the app's: it counts as unsaved work, outlives the pane, and every command that saves or
/// replaces the document writes it first ([`WordApp::flush_drafts`]). Drafts belong to one
/// document ([`Session::document_id`](wordcraft_engine::Session::document_id)): when another
/// document replaces it they are dropped, so an object there at the same position never shows
/// them. A draft is removed only once its command succeeds; a refused edit keeps it.
#[derive(Default)]
pub(crate) struct AltTextDrafts {
    document: u64,
    fields: BTreeMap<AltField, String>,
}

impl AltTextDrafts {
    /// The drafts for `document`, dropping any left from another one.
    fn of(&mut self, document: u64) -> &mut Self {
        if self.document != document {
            self.fields.clear();
            self.document = document;
        }
        self
    }

    /// Nothing typed and not yet written.
    pub(crate) fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

/// Whether the open document has Alt Text typed and not yet written to it (or kept after the
/// document refused it): closing the page would lose it.
pub(crate) fn has_alt_text_drafts(app: &WordApp) -> bool {
    let d = &app.alt_text_drafts;
    d.document == app.session.document_id() && !d.is_empty()
}

fn drafts(app: &mut WordApp) -> &mut AltTextDrafts {
    let document = app.session.document_id();
    app.alt_text_drafts.of(document)
}

/// A text box editing `field`, whose text in the document is `current`. Typing stays in a draft
/// until the box loses focus, or a command that saves or replaces the document writes it
/// ([`flush_alt_text_drafts`]); frame-local buffers lose keystrokes. A refusal is on the status
/// line, and the draft stays in the box.
fn draft_box(app: &mut WordApp, ui: &mut Ui, field: AltField, current: &str, enabled: bool, multiline: bool) {
    let mut text = drafts(app).fields.get(&field).map_or_else(|| current.to_string(), Clone::clone);
    // Per document: egui's own state for the box (its cursor and undo history) never carries over
    // to another document's object at the same position.
    let id = ui.scope_id().with((app.session.document_id(), &field));
    let edit = if multiline { egui::TextEdit::multiline(&mut text).desired_rows(5) } else { egui::TextEdit::singleline(&mut text) };
    let r = ui.add_enabled(enabled, edit.id(id).desired_width(f32::INFINITY));
    if r.changed() {
        // Typing back what the document has leaves nothing to write.
        if text == current {
            drafts(app).fields.remove(&field);
        } else {
            drafts(app).fields.insert(field.clone(), text);
        }
    }
    if r.lost_focus() {
        let _ = commit(app, &field);
    }
}

/// Write `field`'s draft to the document, unless it says what the document already says or what
/// it describes is gone. The draft is dropped once written; a refused edit keeps it.
fn commit(app: &mut WordApp, field: &AltField) -> Result<(), String> {
    let Some(text) = drafts(app).fields.get(field).cloned() else { return Ok(()) };
    if current_text(app, field).is_some_and(|current| current != text) {
        write(app, field, &text)?;
    }
    drafts(app).fields.remove(field);
    Ok(())
}

/// What the document says for `field`; `None` once its object or table is gone.
fn current_text(app: &WordApp, field: &AltField) -> Option<String> {
    let doc = &app.session.doc;
    match field {
        AltField::Object(pos) => doc.para_at(pos)?.object_at(pos.off).filter(|o| o.is_drawing()).map(|o| o.alt_text().to_string()),
        AltField::TableTitle(story, table) => doc.table(*story, table).map(|t| t.props.caption.clone().unwrap_or_default()),
        AltField::TableText(story, table) => doc.table(*story, table).map(|t| t.props.description.clone().unwrap_or_default()),
    }
}

/// Write `text` as `field`'s with its Alt Text command, one undo step. The commands act on the
/// selection: a field whose object or table isn't selected (a draft that outlived the selection)
/// gets it for the command, and the selection comes back.
fn write(app: &mut WordApp, field: &AltField, text: &str) -> Result<(), String> {
    let (command, params) = match field {
        AltField::Object(_) => ("object.altText", json!({"text": text})),
        AltField::TableTitle(..) => ("table.altText", json!({"title": text})),
        AltField::TableText(..) => ("table.altText", json!({"text": text})),
    };
    let Some(at) = selection_for(app, field)? else { return app.run(command, params).map(|_| ()) };
    let before = (app.session.sel.clone(), std::mem::take(&mut app.session.also_selected));
    app.session.sel = at;
    let r = app.run(command, params);
    (app.session.sel, app.session.also_selected) = before;
    r.map(|_| ())
}

/// The selection that puts `field`'s object or table within reach of its command, or `None` when
/// the selection is there already.
fn selection_for(app: &WordApp, field: &AltField) -> Result<Option<Selection>, String> {
    match field {
        AltField::Object(pos) => {
            if wordcraft_engine::cmd::objects::selected(&app.session).is_some_and(|(p, _)| p == *pos) {
                return Ok(None);
            }
            Ok(Some(Selection { anchor: pos.clone(), focus: Pos { off: pos.off + OBJ.len_utf8(), ..pos.clone() } }))
        }
        AltField::TableTitle(story, table) | AltField::TableText(story, table) => {
            let focus = &app.session.sel.focus;
            if focus.story == *story && focus.path.cell().is_some_and(|(t, _, _)| t == *table) {
                return Ok(None);
            }
            let cell = app.session.doc.para_paths(*story).into_iter().find(|p| p.cell().is_some_and(|(t, _, _)| t == *table));
            let cell = cell.ok_or_else(|| "the table has no cell to put the caret in".to_string())?;
            Ok(Some(Selection::caret(Pos { story: *story, path: cell, off: 0 })))
        }
    }
}

/// Write every pending Alt Text draft to the document, so a save has them and a close, New or
/// Open sees them as unsaved changes. Every draft is tried; the first refusal is returned (and is
/// on the status line), and refused drafts stay.
pub(crate) fn flush_alt_text_drafts(app: &mut WordApp) -> Result<(), String> {
    let fields: Vec<AltField> = drafts(app).fields.keys().cloned().collect();
    let mut first = Ok(());
    for field in fields {
        let r = commit(app, &field);
        if first.is_ok() {
            first = r;
        }
    }
    first
}

#[cfg(test)]
mod tests;
