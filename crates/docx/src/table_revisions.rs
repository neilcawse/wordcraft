//! Table row/cell tracked insertion, deletion, and `w:tblPrExChange` preservation around `craft-docx`.

use quick_xml::events::{BytesStart, Event};
use wordcraft_doc::props::{PropChange, TableProps};
use wordcraft_doc::{Block, Document, Revision, RevisionKind};

#[derive(Clone, Debug, Default)]
struct RevAttr {
    author: String,
    date: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct CellRev {
    ins: Option<RevAttr>,
    del: Option<RevAttr>,
}

#[derive(Clone, Debug, Default)]
struct RowRev {
    ex_change: Option<(RevAttr, TableProps)>,
    ins: Option<RevAttr>,
    del: Option<RevAttr>,
    cells: Vec<CellRev>,
}

fn attr_str(e: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    for a in e.attributes().flatten() {
        if a.key.as_ref() == key || a.key.local_name().as_ref() == key {
            return String::from_utf8(a.value.into_owned()).ok();
        }
    }
    None
}

fn rev_attr(e: &BytesStart<'_>) -> RevAttr {
    RevAttr {
        author: attr_str(e, b"w:author").or_else(|| attr_str(e, b"author")).unwrap_or_default(),
        date: attr_str(e, b"w:date").or_else(|| attr_str(e, b"date")),
    }
}

fn parse_table_revs(xml: &str) -> Vec<Vec<RowRev>> {
    if !xml.contains("w:tbl") {
        return Vec::new();
    }
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut tables: Vec<Vec<RowRev>> = Vec::new();
    let mut cur_table: Option<Vec<RowRev>> = None;
    let mut cur_row: Option<RowRev> = None;
    let mut cur_cell: Option<CellRev> = None;
    let mut in_trpr = false;
    let mut in_tcpr = false;
    let mut in_tblprex_change: Option<(RevAttr, TableProps)> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                b"w:tbl" => cur_table = Some(Vec::new()),
                b"w:tr" => cur_row = Some(RowRev::default()),
                b"w:tc" => cur_cell = Some(CellRev::default()),
                b"w:trPr" => in_trpr = true,
                b"w:tcPr" => in_tcpr = true,
                b"w:tblPrExChange" if cur_row.is_some() => {
                    in_tblprex_change = Some((rev_attr(e), TableProps::default()));
                }
                b"w:tblW" if in_tblprex_change.is_some() => {
                    if let Some((_, tp)) = &mut in_tblprex_change {
                        let w_type = attr_str(e, b"w:type").or_else(|| attr_str(e, b"type")).unwrap_or_default();
                        if let Some(w) = attr_str(e, b"w:w").or_else(|| attr_str(e, b"w")).and_then(|s| s.parse::<f32>().ok()) {
                            if w_type == "pct" {
                                tp.width_pct = Some(w / 50.0);
                            } else {
                                tp.width = Some(w / 20.0);
                            }
                        }
                    }
                }
                b"w:ins" if in_trpr => {
                    if let Some(r) = &mut cur_row {
                        r.ins = Some(rev_attr(e));
                    }
                }
                b"w:del" if in_trpr => {
                    if let Some(r) = &mut cur_row {
                        r.del = Some(rev_attr(e));
                    }
                }
                b"w:cellIns" if in_tcpr => {
                    if let Some(c) = &mut cur_cell {
                        c.ins = Some(rev_attr(e));
                    }
                }
                b"w:cellDel" if in_tcpr => {
                    if let Some(c) = &mut cur_cell {
                        c.del = Some(rev_attr(e));
                    }
                }
                _ => {}
            },
            Ok(Event::End(ref e)) => match e.name().as_ref() {
                b"w:trPr" => in_trpr = false,
                b"w:tcPr" => in_tcpr = false,
                b"w:tblPrExChange" => {
                    if let (Some(r), Some(ch)) = (&mut cur_row, in_tblprex_change.take()) {
                        r.ex_change = Some(ch);
                    }
                }
                b"w:tc" => {
                    if let (Some(r), Some(c)) = (&mut cur_row, cur_cell.take()) {
                        r.cells.push(c);
                    }
                }
                b"w:tr" => {
                    if let (Some(t), Some(r)) = (&mut cur_table, cur_row.take()) {
                        t.push(r);
                    }
                }
                b"w:tbl" => {
                    if let Some(t) = cur_table.take() {
                        tables.push(t);
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    tables
}

fn push_rev(doc: &mut Document, kind: RevisionKind, attr: &RevAttr) -> u32 {
    let id = doc.revisions.len() as u32;
    doc.revisions.push(Revision { kind, author: attr.author.clone(), date: attr.date.clone().unwrap_or_default(), ..Default::default() });
    id
}

pub(crate) fn apply_read_table_revs(bytes: &[u8], doc: &mut Document) {
    let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(bytes)) else {
        return;
    };
    let Ok(mut file) = archive.by_name("word/document.xml") else {
        return;
    };
    let mut xml = String::new();
    if std::io::Read::read_to_string(&mut file, &mut xml).is_err() {
        return;
    }
    let parsed = parse_table_revs(&xml);
    if parsed.is_empty() {
        return;
    }
    let mut t_idx = 0usize;
    let body = std::mem::take(&mut doc.body);
    let mut new_body = Vec::with_capacity(body.len());
    for mut block in body {
        if let Block::Table(t) = std::sync::Arc::make_mut(&mut block) {
            if let Some(t_revs) = parsed.get(t_idx) {
                for (row, r_rev) in t.rows.iter_mut().zip(t_revs.iter()) {
                    if let Some((attr, old_tp)) = &r_rev.ex_change {
                        let rev_id = push_rev(doc, RevisionKind::Format, attr);
                        let ex = row.props.exceptions.get_or_insert_with(|| Box::new(TableProps::default()));
                        ex.fmt_change = PropChange::boxed(rev_id, old_tp.clone());
                    }
                    if let Some(attr) = &r_rev.ins {
                        row.props.ins = Some(push_rev(doc, RevisionKind::Insert, attr));
                    }
                    if let Some(attr) = &r_rev.del {
                        row.props.del = Some(push_rev(doc, RevisionKind::Delete, attr));
                    }
                    for (cell, c_rev) in row.cells.iter_mut().zip(r_rev.cells.iter()) {
                        if let Some(attr) = &c_rev.ins {
                            cell.props.ins = Some(push_rev(doc, RevisionKind::Insert, attr));
                        }
                        if let Some(attr) = &c_rev.del {
                            cell.props.del = Some(push_rev(doc, RevisionKind::Delete, attr));
                        }
                    }
                }
            }
            t_idx += 1;
        }
        new_body.push(block);
    }
    doc.body = new_body;
}

fn fmt_change_attrs(doc: &Document, rev: u32, next_id: &mut u32) -> String {
    let id = *next_id;
    *next_id += 1;
    let r = doc.revisions.get(rev as usize);
    let author = r.map(|x| x.author.as_str()).unwrap_or("Unknown");
    let esc_author = quick_xml::escape::escape(author);
    match r.map(|x| x.date.as_str()).filter(|d| !d.is_empty()) {
        Some(d) => format!(r#"w:id="{id}" w:author="{esc_author}" w:date="{}""#, quick_xml::escape::escape(d)),
        None => format!(r#"w:id="{id}" w:author="{esc_author}""#),
    }
}

fn fmt_tblprex_inner(t: &TableProps) -> String {
    let mut s = String::new();
    if let Some(p) = t.width_pct {
        let w = (p.clamp(0.0, 1000.0) * 50.0).round() as i64;
        s.push_str(&format!(r#"<w:tblW w:w="{w}" w:type="pct"/>"#));
    } else if let Some(v) = t.width {
        let w = wordcraft_geom::to_twips(v);
        s.push_str(&format!(r#"<w:tblW w:w="{w}" w:type="dxa"/>"#));
    }
    s
}

fn rewrite_table_xml(xml: &str, doc: &Document) -> Option<String> {
    let tables: Vec<&wordcraft_doc::table::Table> = doc.body.iter().filter_map(|b| b.as_table()).collect();
    let has_any = tables.iter().any(|t| {
        t.rows.iter().any(|r| {
            r.props.ins.is_some()
                || r.props.del.is_some()
                || r.props.exceptions.as_ref().is_some_and(|x| x.fmt_change.is_some())
                || r.cells.iter().any(|c| c.props.ins.is_some() || c.props.del.is_some())
        })
    });
    if !has_any {
        return None;
    }

    let mut reader = quick_xml::Reader::from_str(xml);
    let mut writer = quick_xml::Writer::new(Vec::with_capacity(xml.len() + 512));
    let mut buf = Vec::new();
    let mut t_idx = 0usize;
    let mut r_idx = 0usize;
    let mut c_idx = 0usize;
    let mut in_tbl = false;
    let mut in_tr = false;
    let mut tr_had_trpr = false;
    let mut next_id = 100u32;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = e.name();
                match name.as_ref() {
                    b"w:tbl" => {
                        in_tbl = true;
                        r_idx = 0;
                    }
                    b"w:tr" if in_tbl => {
                        in_tr = true;
                        tr_had_trpr = false;
                        c_idx = 0;
                    }
                    b"w:trPr" if in_tr => {
                        tr_had_trpr = true;
                    }
                    b"w:tc" if in_tr => {
                        if !tr_had_trpr
                            && let Some(row) = tables.get(t_idx).and_then(|t| t.rows.get(r_idx))
                            && (row.props.ins.is_some() || row.props.del.is_some())
                        {
                            let mut trpr = String::from("<w:trPr>");
                            if let Some(ins) = row.props.ins {
                                trpr.push_str(&format!("<w:ins {}/>", fmt_change_attrs(doc, ins, &mut next_id)));
                            }
                            if let Some(del) = row.props.del {
                                trpr.push_str(&format!("<w:del {}/>", fmt_change_attrs(doc, del, &mut next_id)));
                            }
                            trpr.push_str("</w:trPr>");
                            writer.get_mut().extend_from_slice(trpr.as_bytes());
                            tr_had_trpr = true;
                        }
                    }
                    _ => {}
                }
                writer.write_event(Event::Start(e)).ok()?;
            }
            Ok(Event::End(e)) => {
                let name = e.name();
                match name.as_ref() {
                    b"w:tblPrEx" if in_tr => {
                        if let Some(ch) = tables
                            .get(t_idx)
                            .and_then(|t| t.rows.get(r_idx))
                            .and_then(|r| r.props.exceptions.as_ref())
                            .and_then(|x| x.fmt_change.as_ref())
                        {
                            let attrs = fmt_change_attrs(doc, ch.rev, &mut next_id);
                            let inner = fmt_tblprex_inner(&ch.old);
                            let snippet = format!("<w:tblPrExChange {attrs}><w:tblPrEx>{inner}</w:tblPrEx></w:tblPrExChange>");
                            writer.get_mut().extend_from_slice(snippet.as_bytes());
                        }
                    }
                    b"w:trPr" if in_tr => {
                        if let Some(row) = tables.get(t_idx).and_then(|t| t.rows.get(r_idx)) {
                            if let Some(ins) = row.props.ins {
                                let snippet = format!("<w:ins {}/>", fmt_change_attrs(doc, ins, &mut next_id));
                                writer.get_mut().extend_from_slice(snippet.as_bytes());
                            }
                            if let Some(del) = row.props.del {
                                let snippet = format!("<w:del {}/>", fmt_change_attrs(doc, del, &mut next_id));
                                writer.get_mut().extend_from_slice(snippet.as_bytes());
                            }
                        }
                    }
                    b"w:tcPr" if in_tr => {
                        if let Some(cell) = tables.get(t_idx).and_then(|t| t.rows.get(r_idx)).and_then(|r| r.cells.get(c_idx)) {
                            if let Some(ins) = cell.props.ins {
                                let snippet = format!("<w:cellIns {}/>", fmt_change_attrs(doc, ins, &mut next_id));
                                writer.get_mut().extend_from_slice(snippet.as_bytes());
                            } else if let Some(del) = cell.props.del {
                                let snippet = format!("<w:cellDel {}/>", fmt_change_attrs(doc, del, &mut next_id));
                                writer.get_mut().extend_from_slice(snippet.as_bytes());
                            }
                        }
                    }
                    b"w:tc" if in_tr => {
                        c_idx += 1;
                    }
                    b"w:tr" if in_tr => {
                        in_tr = false;
                        r_idx += 1;
                    }
                    b"w:tbl" if in_tbl => {
                        in_tbl = false;
                        t_idx += 1;
                    }
                    _ => {}
                }
                writer.write_event(Event::End(e)).ok()?;
            }
            Ok(Event::Eof) => break,
            Ok(ev) => writer.write_event(ev).ok()?,
            Err(_) => return None,
        }
        buf.clear();
    }
    String::from_utf8(writer.into_inner()).ok()
}

pub(crate) fn apply_write_table_revs(doc: &Document, bytes: Vec<u8>) -> Vec<u8> {
    let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(bytes.as_slice())) else {
        return bytes;
    };
    let mut entries = Vec::with_capacity(archive.len());
    let mut changed = false;
    for idx in 0..archive.len() {
        let (name, mut data) = {
            let Ok(mut file) = archive.by_index(idx) else {
                drop(archive);
                return bytes;
            };
            let name = file.name().to_string();
            let mut data = Vec::new();
            if std::io::Read::read_to_end(&mut file, &mut data).is_err() {
                drop(file);
                drop(archive);
                return bytes;
            }
            (name, data)
        };
        if name == "word/document.xml"
            && let Ok(xml) = std::str::from_utf8(&data)
            && let Some(rewritten) = rewrite_table_xml(xml, doc)
        {
            data = rewritten.into_bytes();
            changed = true;
        }
        entries.push((name, data));
    }
    drop(archive);
    if !changed {
        return bytes;
    }
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::with_capacity(bytes.len() + 512)));
    for (name, data) in entries {
        if out.start_file(name, zip::write::SimpleFileOptions::default()).is_err() {
            return bytes;
        }
        if std::io::Write::write_all(&mut out, &data).is_err() {
            return bytes;
        }
    }
    out.finish().map(|c| c.into_inner()).unwrap_or(bytes)
}
