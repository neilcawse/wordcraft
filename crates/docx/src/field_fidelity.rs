//! Range-field `w:ffData` (`form`) and result `w:rPrChange` preservation around `craft-docx`.

use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use wordcraft_doc::para::InlineObject;
use wordcraft_doc::{Block, Blocks, Document};

const RANGE_MARKER: &str = "ADDIN __WC_RANGE__ ";

fn attr_str(e: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    for a in e.attributes().flatten() {
        if a.key.as_ref() == key || a.key.local_name().as_ref() == key {
            return String::from_utf8(a.value.into_owned()).ok();
        }
    }
    None
}

/// Extract `<w:ffData>...</w:ffData>` strings from each `<w:fldChar w:fldCharType="begin">` in
/// document order, and mark complex fields whose result runs carry `<w:rPrChange` so `craft-docx`
/// keeps them as range fields (`FieldStart`..`FieldEnd`).
fn inspect_and_mark_fields_xml(xml: &str) -> (Vec<String>, Option<String>) {
    if !xml.contains("fldChar") {
        return (Vec::new(), None);
    }
    // First pass: for each top-level complex field (`begin`..`end`), record:
    // 1) any `<w:ffData>...</w:ffData>` inside its `begin`
    // 2) whether its result (between `separate` and `end`) contains `<w:rPrChange`
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut forms = Vec::new();
    let mut needs_range_marker = Vec::new();

    #[derive(Default)]
    struct FieldScan {
        in_begin: bool,
        in_ffdata: usize,
        ffdata_buf: Vec<u8>,
        in_result: bool,
        has_rpr_change: bool,
    }
    let mut stack: Vec<FieldScan> = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                if let Some(top) = stack.last_mut()
                    && top.in_begin
                    && (e.name().as_ref() == b"w:ffData" || top.in_ffdata > 0)
                {
                    top.in_ffdata += 1;
                    let mut w = quick_xml::Writer::new(&mut top.ffdata_buf);
                    let _ = w.write_event(Event::Start(e.clone()));
                    buf.clear();
                    continue;
                }
                match e.name().as_ref() {
                    b"w:fldChar" => match attr_str(e, b"w:fldCharType").as_deref() {
                        Some("begin") => {
                            stack.push(FieldScan { in_begin: true, ..Default::default() });
                        }
                        Some("separate") => {
                            if let Some(top) = stack.last_mut() {
                                top.in_result = true;
                            }
                        }
                        Some("end") => {
                            if let Some(done) = stack.pop() {
                                forms.push(String::from_utf8(done.ffdata_buf).unwrap_or_default());
                                needs_range_marker.push(done.has_rpr_change);
                            }
                        }
                        _ => {}
                    },
                    b"w:rPrChange" => {
                        if let Some(top) = stack.last_mut()
                            && top.in_result
                        {
                            top.has_rpr_change = true;
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(ref e)) => {
                if let Some(top) = stack.last_mut()
                    && top.in_begin
                    && (e.name().as_ref() == b"w:ffData" || top.in_ffdata > 0)
                {
                    let mut w = quick_xml::Writer::new(&mut top.ffdata_buf);
                    let _ = w.write_event(Event::Empty(e.clone()));
                    buf.clear();
                    continue;
                }
                match e.name().as_ref() {
                    b"w:fldChar" => match attr_str(e, b"w:fldCharType").as_deref() {
                        Some("begin") => {
                            stack.push(FieldScan::default());
                        }
                        Some("separate") => {
                            if let Some(top) = stack.last_mut() {
                                top.in_result = true;
                            }
                        }
                        Some("end") => {
                            if let Some(done) = stack.pop() {
                                forms.push(String::from_utf8(done.ffdata_buf).unwrap_or_default());
                                needs_range_marker.push(done.has_rpr_change);
                            }
                        }
                        _ => {}
                    },
                    b"w:rPrChange" => {
                        if let Some(top) = stack.last_mut()
                            && top.in_result
                        {
                            top.has_rpr_change = true;
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                if let Some(top) = stack.last_mut()
                    && top.in_begin
                    && top.in_ffdata > 0
                {
                    let mut w = quick_xml::Writer::new(&mut top.ffdata_buf);
                    let _ = w.write_event(Event::End(e.clone()));
                    top.in_ffdata -= 1;
                    buf.clear();
                    continue;
                }
                if e.name().as_ref() == b"w:fldChar"
                    && let Some(top) = stack.last_mut()
                {
                    top.in_begin = false;
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            Ok(ev) => {
                if let Some(top) = stack.last_mut()
                    && top.in_begin
                    && top.in_ffdata > 0
                {
                    let mut w = quick_xml::Writer::new(&mut top.ffdata_buf);
                    let _ = w.write_event(ev);
                }
            }
        }
        buf.clear();
    }

    if !needs_range_marker.iter().any(|&b| b) {
        return (forms, None);
    }

    // Second pass: prefix the first `<w:instrText>` of each marked complex field with `RANGE_MARKER`.
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut writer = quick_xml::Writer::new(Vec::with_capacity(xml.len() + 64));
    let mut field_idx = 0usize;
    let mut active_marked = false;
    let mut in_instr_text = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                match e.name().as_ref() {
                    b"w:fldChar" if attr_str(&e, b"w:fldCharType").as_deref() == Some("begin") => {
                        active_marked = needs_range_marker.get(field_idx).copied().unwrap_or(false);
                        field_idx += 1;
                    }
                    b"w:instrText" if active_marked => {
                        in_instr_text = true;
                    }
                    _ => {}
                }
                if writer.write_event(Event::Start(e)).is_err() {
                    return (forms, None);
                }
            }
            Ok(Event::Empty(e)) => {
                if e.name().as_ref() == b"w:fldChar" && attr_str(&e, b"w:fldCharType").as_deref() == Some("begin") {
                    active_marked = needs_range_marker.get(field_idx).copied().unwrap_or(false);
                    field_idx += 1;
                }
                if writer.write_event(Event::Empty(e)).is_err() {
                    return (forms, None);
                }
            }
            Ok(Event::Text(e)) if in_instr_text && active_marked => {
                let raw = String::from_utf8_lossy(e.as_ref());
                let new_txt = format!("{RANGE_MARKER}{raw}");
                let _ = writer.write_event(Event::Text(BytesText::from_escaped(&new_txt)));
                active_marked = false;
                in_instr_text = false;
            }
            Ok(Event::End(e)) => {
                if e.name().as_ref() == b"w:instrText" {
                    in_instr_text = false;
                }
                if writer.write_event(Event::End(e)).is_err() {
                    return (forms, None);
                }
            }
            Ok(Event::Eof) => break,
            Ok(ev) => {
                if writer.write_event(ev).is_err() {
                    return (forms, None);
                }
            }
            Err(_) => return (forms, None),
        }
        buf.clear();
    }
    (forms, String::from_utf8(writer.into_inner()).ok())
}

pub(crate) fn preprocess_read(bytes: &[u8]) -> (Vec<String>, Option<Vec<u8>>) {
    let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(bytes)) else {
        return (Vec::new(), None);
    };
    let mut entries = Vec::with_capacity(archive.len());
    let mut forms = Vec::new();
    let mut changed = false;
    for idx in 0..archive.len() {
        let Ok(mut file) = archive.by_index(idx) else {
            return (Vec::new(), None);
        };
        let name = file.name().to_string();
        let mut data = Vec::new();
        if std::io::Read::read_to_end(&mut file, &mut data).is_err() {
            return (Vec::new(), None);
        }
        if name == "word/document.xml"
            && let Ok(xml) = std::str::from_utf8(&data)
        {
            let (f, rewritten) = inspect_and_mark_fields_xml(xml);
            forms = f;
            if let Some(r) = rewritten {
                data = r.into_bytes();
                changed = true;
            }
        }
        entries.push((name, data));
    }
    if !changed {
        return (forms, None);
    }
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::with_capacity(bytes.len() + 64)));
    for (name, data) in entries {
        if out.start_file(name, zip::write::SimpleFileOptions::default()).is_err() {
            return (forms, None);
        }
        if std::io::Write::write_all(&mut out, &data).is_err() {
            return (forms, None);
        }
    }
    (forms, out.finish().ok().map(|c| c.into_inner()))
}

fn apply_forms_to_blocks(blocks: &mut Blocks, forms: &mut std::slice::Iter<'_, String>) {
    for block in blocks.iter_mut() {
        match std::sync::Arc::make_mut(block) {
            Block::Para(p) => {
                for obj in &mut p.objects {
                    match obj {
                        InlineObject::FieldStart { instr, form, .. } => {
                            if let Some(rest) = instr.strip_prefix(RANGE_MARKER) {
                                *instr = rest.trim().to_string();
                            }
                            if let Some(f) = forms.next()
                                && form.is_empty()
                                && !f.is_empty()
                            {
                                *form = f.clone();
                            }
                        }
                        InlineObject::Field { form, .. } => {
                            if let Some(f) = forms.next()
                                && form.is_empty()
                                && !f.is_empty()
                            {
                                *form = f.clone();
                            }
                        }
                        _ => {}
                    }
                }
            }
            Block::Table(t) => {
                for r in &mut t.rows {
                    for c in &mut r.cells {
                        apply_forms_to_blocks(&mut c.blocks, forms);
                    }
                }
            }
        }
    }
}

pub(crate) fn postprocess_read(doc: &mut Document, forms: &[String]) {
    let mut iter = forms.iter();
    apply_forms_to_blocks(&mut doc.body, &mut iter);
}

fn collect_forms_from_blocks(blocks: &Blocks, out: &mut Vec<String>) {
    for block in blocks.iter() {
        match block.as_ref() {
            Block::Para(p) => {
                for obj in &p.objects {
                    match obj {
                        InlineObject::FieldStart { form, .. } | InlineObject::Field { form, .. } => {
                            out.push(form.clone());
                        }
                        _ => {}
                    }
                }
            }
            Block::Table(t) => {
                for r in &t.rows {
                    for c in &r.cells {
                        collect_forms_from_blocks(&c.blocks, out);
                    }
                }
            }
        }
    }
}

fn inject_forms_into_xml(xml: &str, forms: &[String]) -> Option<String> {
    if !forms.iter().any(|f| !f.is_empty()) {
        return None;
    }
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut writer = quick_xml::Writer::new(Vec::with_capacity(xml.len() + 256));
    let mut buf = Vec::new();
    let mut idx = 0usize;
    let mut changed = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if e.name().as_ref() == b"w:fldChar" && attr_str(&e, b"w:fldCharType").as_deref() == Some("begin") {
                    idx += 1;
                }
                writer.write_event(Event::Start(e)).ok()?;
            }
            Ok(Event::Empty(e)) => {
                if e.name().as_ref() == b"w:fldChar" && attr_str(&e, b"w:fldCharType").as_deref() == Some("begin") {
                    let cur_form = forms.get(idx).map(String::as_str).unwrap_or("");
                    idx += 1;
                    if !cur_form.is_empty() {
                        writer.write_event(Event::Start(e)).ok()?;
                        writer.get_mut().extend_from_slice(cur_form.as_bytes());
                        writer.write_event(Event::End(BytesEnd::new("w:fldChar"))).ok()?;
                        changed = true;
                        buf.clear();
                        continue;
                    }
                }
                writer.write_event(Event::Empty(e)).ok()?;
            }
            Ok(Event::Eof) => break,
            Ok(ev) => writer.write_event(ev).ok()?,
            Err(_) => return None,
        }
        buf.clear();
    }
    if changed { String::from_utf8(writer.into_inner()).ok() } else { None }
}

pub(crate) fn postprocess_write(doc: &Document, bytes: Vec<u8>) -> Vec<u8> {
    let mut forms = Vec::new();
    collect_forms_from_blocks(&doc.body, &mut forms);
    if !forms.iter().any(|f| !f.is_empty()) {
        return bytes;
    }
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
            && let Some(rewritten) = inject_forms_into_xml(xml, &forms)
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
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::with_capacity(bytes.len() + 256)));
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
