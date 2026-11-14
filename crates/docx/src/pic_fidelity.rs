//! Picture crop, stroke, and DrawingML effect preservation around `craft-docx`.

use quick_xml::events::{BytesStart, Event};
use wordcraft_doc::effects::{Glow, Shadow, ShapeEffects};
use wordcraft_doc::para::InlineObject;
use wordcraft_doc::props::Rgb;
use wordcraft_doc::{Block, Blocks, Document};

#[derive(Clone, Debug, Default)]
struct PicExtra {
    crop: Option<[f32; 4]>,
    stroke: Option<Rgb>,
    stroke_width: f32,
    effects: ShapeEffects,
}

fn attr_str(e: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    for a in e.attributes().flatten() {
        if a.key.as_ref() == key || a.key.local_name().as_ref() == key {
            return String::from_utf8(a.value.into_owned()).ok();
        }
    }
    None
}

fn attr_i64(e: &BytesStart<'_>, key: &[u8]) -> Option<i64> {
    attr_str(e, key)?.trim().parse::<f64>().ok().filter(|v| v.is_finite()).map(|v| v.round() as i64)
}

fn parse_pic_extras(xml: &str) -> Vec<PicExtra> {
    if !xml.contains("pic:pic") {
        return Vec::new();
    }
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut out = Vec::new();
    let mut in_pic = false;
    let mut cur = PicExtra::default();
    let mut in_sppr = false;
    let mut in_ln = false;
    let mut in_outer_shdw = false;
    let mut in_glow = false;
    let mut shdw_blur = 0.0f32;
    let mut shdw_dist = 0.0f32;
    let mut shdw_angle = 0.0f32;
    let mut shdw_rot = true;
    let mut glow_rad = 0.0f32;
    let mut cur_rgb: Option<Rgb> = None;
    let mut cur_alpha = 100.0f32;
    let mut cur_alpha_mod: Option<i64> = None;
    let mut cur_alpha_off: Option<i64> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let name = e.name();
                match name.as_ref() {
                    b"pic:pic" => {
                        in_pic = true;
                        cur = PicExtra::default();
                    }
                    b"a:srcRect" if in_pic => {
                        let mut crop = [0.0f32; 4];
                        for (i, k) in [b"l".as_slice(), b"t", b"r", b"b"].into_iter().enumerate() {
                            if let Some(v) = attr_i64(e, k) {
                                crop[i] = (v.clamp(-100_000_000, 100_000_000) as f32 / 100_000.0).clamp(-1000.0, 1000.0);
                            }
                        }
                        cur.crop = Some(crop);
                    }
                    b"pic:spPr" if in_pic => in_sppr = true,
                    b"a:ln" if in_sppr => {
                        in_ln = true;
                        if let Some(w) = attr_i64(e, b"w") {
                            cur.stroke_width = (w.max(0) as f32 / 12_700.0).clamp(0.0, 14_400.0);
                        }
                    }
                    b"a:outerShdw" if in_sppr => {
                        in_outer_shdw = true;
                        shdw_blur = attr_i64(e, b"blurRad").map(|v| v.max(0) as f32 / 12_700.0).unwrap_or(0.0);
                        shdw_dist = attr_i64(e, b"dist").map(|v| v.max(0) as f32 / 12_700.0).unwrap_or(0.0);
                        shdw_angle = attr_i64(e, b"dir").map(|v| (v.rem_euclid(21_600_000) as f32) / 60_000.0).unwrap_or(0.0);
                        shdw_rot = !matches!(attr_str(e, b"rotWithShape").as_deref(), Some("0" | "false"));
                        cur_rgb = None;
                        cur_alpha = 100.0;
                        cur_alpha_mod = None;
                        cur_alpha_off = None;
                    }
                    b"a:glow" if in_sppr => {
                        in_glow = true;
                        glow_rad = attr_i64(e, b"rad").map(|v| v.max(0) as f32 / 12_700.0).unwrap_or(0.0);
                        cur_rgb = None;
                        cur_alpha = 100.0;
                        cur_alpha_mod = None;
                        cur_alpha_off = None;
                    }
                    b"a:softEdge" if in_sppr => {
                        if let Some(r) = attr_i64(e, b"rad") {
                            cur.effects.soft_edge = Some((r.max(0) as f32 / 12_700.0).clamp(0.0, 14_400.0));
                        }
                    }
                    b"a:srgbClr" if in_ln || in_outer_shdw || in_glow => {
                        let rgb = attr_str(e, b"val").and_then(|s| Rgb::parse(&s));
                        if in_ln {
                            cur.stroke = rgb;
                        } else {
                            cur_rgb = rgb;
                        }
                    }
                    b"a:alpha" if in_outer_shdw || in_glow => {
                        if let Some(v) = attr_i64(e, b"val") {
                            cur_alpha = v.clamp(0, 100_000) as f32 / 1000.0;
                        }
                    }
                    b"a:alphaMod" if in_outer_shdw || in_glow => {
                        cur_alpha_mod = attr_i64(e, b"val");
                    }
                    b"a:alphaOff" if in_outer_shdw || in_glow => {
                        cur_alpha_off = attr_i64(e, b"val");
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => match e.name().as_ref() {
                b"pic:pic" if in_pic => {
                    in_pic = false;
                    cur.effects = cur.effects.sanitized();
                    out.push(std::mem::take(&mut cur));
                }
                b"pic:spPr" => in_sppr = false,
                b"a:ln" => in_ln = false,
                b"a:outerShdw" if in_outer_shdw => {
                    in_outer_shdw = false;
                    let mut opacity = cur_alpha;
                    if let Some(m) = cur_alpha_mod {
                        opacity *= m.max(0) as f32 / 100_000.0;
                    }
                    if let Some(o) = cur_alpha_off {
                        opacity += o as f32 / 1000.0;
                    }
                    let transparency = 100.0 - opacity.clamp(0.0, 100.0);
                    cur.effects.shadow = Some(Shadow {
                        color: cur_rgb.unwrap_or(Rgb::BLACK),
                        transparency,
                        blur: shdw_blur,
                        distance: shdw_dist,
                        angle: shdw_angle,
                        rot_with_shape: shdw_rot,
                    });
                }
                b"a:glow" if in_glow => {
                    in_glow = false;
                    let mut opacity = cur_alpha;
                    if let Some(m) = cur_alpha_mod {
                        opacity *= m.max(0) as f32 / 100_000.0;
                    }
                    if let Some(o) = cur_alpha_off {
                        opacity += o as f32 / 1000.0;
                    }
                    let transparency = 100.0 - opacity.clamp(0.0, 100.0);
                    cur.effects.glow = Some(Glow { color: cur_rgb.unwrap_or(Glow::default().color), size: glow_rad, transparency });
                }
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

fn apply_extras_to_blocks(blocks: &mut Blocks, iter: &mut std::slice::Iter<'_, PicExtra>) {
    for block in blocks.iter_mut() {
        match std::sync::Arc::make_mut(block) {
            Block::Para(p) => {
                for obj in &mut p.objects {
                    apply_extras_to_obj(obj, iter);
                }
            }
            Block::Table(t) => {
                for r in &mut t.rows {
                    for c in &mut r.cells {
                        apply_extras_to_blocks(&mut c.blocks, iter);
                    }
                }
            }
        }
    }
}

fn apply_extras_to_obj(obj: &mut InlineObject, iter: &mut std::slice::Iter<'_, PicExtra>) {
    match obj {
        InlineObject::Image { crop, float, .. } => {
            if let Some(ex) = iter.next() {
                if let Some(c) = ex.crop {
                    *crop = c;
                }
                if ex.stroke.is_some() {
                    float.pic_stroke = ex.stroke;
                    float.pic_stroke_width = ex.stroke_width;
                }
                if !ex.effects.is_empty() {
                    float.pic_effects = ex.effects;
                }
            }
        }
        InlineObject::Group { children, .. } => {
            for ch in children {
                apply_extras_to_obj(&mut ch.obj, iter);
            }
        }
        _ => {}
    }
}

pub(crate) fn apply_read_extras(bytes: &[u8], doc: &mut Document) {
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
    let extras = parse_pic_extras(&xml);
    if !extras.is_empty() {
        let mut iter = extras.iter();
        apply_extras_to_blocks(&mut doc.body, &mut iter);
    }
}

fn collect_doc_images(blocks: &Blocks, out: &mut Vec<PicExtra>) {
    for block in blocks.iter() {
        match block.as_ref() {
            Block::Para(p) => {
                for obj in &p.objects {
                    collect_obj_images(obj, out);
                }
            }
            Block::Table(t) => {
                for r in &t.rows {
                    for c in &r.cells {
                        collect_doc_images(&c.blocks, out);
                    }
                }
            }
        }
    }
}

fn collect_obj_images(obj: &InlineObject, out: &mut Vec<PicExtra>) {
    match obj {
        InlineObject::Image { crop, float, .. } => {
            out.push(PicExtra {
                crop: crop.iter().any(|&v| v != 0.0).then_some(*crop),
                stroke: float.pic_stroke,
                stroke_width: float.pic_stroke_width,
                effects: float.pic_effects,
            });
        }
        InlineObject::Group { children, .. } => {
            for ch in children {
                collect_obj_images(&ch.obj, out);
            }
        }
        _ => {}
    }
}

fn format_sppr_extras(ex: &PicExtra) -> String {
    let mut s = String::new();
    if let Some(c) = ex.stroke {
        let w = wordcraft_geom::to_emu(ex.stroke_width.clamp(0.0, 100.0));
        s.push_str(&format!(r#"<a:ln w="{w}"><a:solidFill><a:srgbClr val="{}"/></a:solidFill></a:ln>"#, c.hex()));
    }
    let fx = &ex.effects;
    if !fx.is_empty() {
        s.push_str("<a:effectLst>");
        if let Some(sh) = &fx.shadow {
            let blur = wordcraft_geom::to_emu(sh.blur.max(0.0));
            let dist = wordcraft_geom::to_emu(sh.distance.max(0.0));
            let dir = ((sh.angle.rem_euclid(360.0) * 60_000.0).round() as i64).clamp(0, 21_599_999);
            let rot = if sh.rot_with_shape { "1" } else { "0" };
            let alpha = (((100.0 - sh.transparency.clamp(0.0, 100.0)) * 1000.0).round() as i64).clamp(0, 100_000);
            s.push_str(&format!(
                r#"<a:outerShdw blurRad="{blur}" dist="{dist}" dir="{dir}" rotWithShape="{rot}"><a:srgbClr val="{}"><a:alpha val="{alpha}"/></a:srgbClr></a:outerShdw>"#,
                sh.color.hex()
            ));
        }
        if let Some(gl) = &fx.glow {
            let rad = wordcraft_geom::to_emu(gl.size.max(0.0));
            let alpha = (((100.0 - gl.transparency.clamp(0.0, 100.0)) * 1000.0).round() as i64).clamp(0, 100_000);
            s.push_str(&format!(r#"<a:glow rad="{rad}"><a:srgbClr val="{}"><a:alpha val="{alpha}"/></a:srgbClr></a:glow>"#, gl.color.hex()));
        }
        if let Some(se) = fx.soft_edge {
            let rad = wordcraft_geom::to_emu(se.max(0.0));
            s.push_str(&format!(r#"<a:softEdge rad="{rad}"/>"#));
        }
        s.push_str("</a:effectLst>");
    }
    s
}

fn rewrite_pic_xml(xml: &str, extras: &[PicExtra]) -> Option<String> {
    if !extras.iter().any(|e| e.crop.is_some() || e.stroke.is_some() || !e.effects.is_empty()) {
        return None;
    }
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut writer = quick_xml::Writer::new(Vec::with_capacity(xml.len() + 256));
    let mut buf = Vec::new();
    let mut pic_idx = 0usize;
    let mut in_pic = false;
    let mut wrote_srcrect = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = e.name();
                if name.as_ref() == b"pic:pic" {
                    in_pic = true;
                    wrote_srcrect = false;
                } else if in_pic
                    && name.as_ref() == b"a:stretch"
                    && !wrote_srcrect
                    && let Some(Some(crop)) = extras.get(pic_idx).map(|x| x.crop)
                {
                    let c = |v: f32| ((wordcraft_geom::finite(v).clamp(-1000.0, 1000.0) * 100_000.0).round() as i64).to_string();
                    let mut sr = BytesStart::new("a:srcRect");
                    sr.push_attribute(("l", c(crop[0]).as_str()));
                    sr.push_attribute(("t", c(crop[1]).as_str()));
                    sr.push_attribute(("r", c(crop[2]).as_str()));
                    sr.push_attribute(("b", c(crop[3]).as_str()));
                    writer.write_event(Event::Empty(sr)).ok()?;
                    wrote_srcrect = true;
                }
                writer.write_event(Event::Start(e)).ok()?;
            }
            Ok(Event::Empty(e)) => {
                if in_pic && e.name().as_ref() == b"a:srcRect" {
                    if let Some(Some(crop)) = extras.get(pic_idx).map(|x| x.crop) {
                        let c = |v: f32| ((wordcraft_geom::finite(v).clamp(-1000.0, 1000.0) * 100_000.0).round() as i64).to_string();
                        let mut sr = BytesStart::new("a:srcRect");
                        sr.push_attribute(("l", c(crop[0]).as_str()));
                        sr.push_attribute(("t", c(crop[1]).as_str()));
                        sr.push_attribute(("r", c(crop[2]).as_str()));
                        sr.push_attribute(("b", c(crop[3]).as_str()));
                        writer.write_event(Event::Empty(sr)).ok()?;
                        wrote_srcrect = true;
                        buf.clear();
                        continue;
                    }
                    wrote_srcrect = true;
                }
                writer.write_event(Event::Empty(e)).ok()?;
            }
            Ok(Event::End(e)) => {
                if in_pic && e.name().as_ref() == b"pic:spPr" {
                    if let Some(ex) = extras.get(pic_idx) {
                        let snippet = format_sppr_extras(ex);
                        if !snippet.is_empty() {
                            writer.get_mut().extend_from_slice(snippet.as_bytes());
                        }
                    }
                } else if in_pic && e.name().as_ref() == b"pic:pic" {
                    in_pic = false;
                    pic_idx += 1;
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

pub(crate) fn apply_write_extras(doc: &Document, bytes: Vec<u8>) -> Vec<u8> {
    let mut extras = Vec::new();
    collect_doc_images(&doc.body, &mut extras);
    if !extras.iter().any(|e| e.crop.is_some() || e.stroke.is_some() || !e.effects.is_empty()) {
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
            && let Some(rewritten) = rewrite_pic_xml(xml, &extras)
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
