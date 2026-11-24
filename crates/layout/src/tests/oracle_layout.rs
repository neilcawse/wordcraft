//! Synthetic regressions for oversized repeated table headers and body paragraph-relative
//! wrapping float clamping.

use super::lay;
use crate::{Placed, StoryRef};
use wordcraft_doc::para::{Anchor, Float, InlineObject, Wrap};
use wordcraft_doc::props::HeightRule;
use wordcraft_doc::{Block, Document, Paragraph, Path, Table, para_block};

fn float_picture(w: f32, h: f32, float: Float) -> InlineObject {
    InlineObject::Image { media: "synthetic-float".into(), w, h, alt: "synthetic".into(), float, crop: [0.0; 4], fx: Default::default(), ole: None }
}

/// When consecutive leading `tblHeader` rows alone fill or exceed the page body height, Word does
/// not repeat them on continuation pages (repeating them would push every body row off the page
/// and spin one page per body row).
#[test]
fn oversized_table_header_block_does_not_repeat_on_continuation_pages() {
    let mut doc = Document::new();
    // 10 header rows x 80 pt = 800 pt > default Letter body height (648 pt), followed by 6 body rows.
    let mut t = Table::new(16, 1, 468.0);
    for (ri, row) in t.rows.iter_mut().enumerate() {
        row.props.header = ri < 10;
        row.props.height = Some(80.0);
        row.props.height_rule = HeightRule::AtLeast;
        row.cells[0].blocks = vec![para_block(Paragraph::with_text(&format!("Row {ri}"), Default::default()))];
    }
    doc.insert_block(StoryRef::Body, &Path::top(0), Block::Table(t)).unwrap();

    let laid = lay(&doc);
    // 16 rows x 80 pt = 1280 pt across 648 pt pages -> 2 or 3 pages (never 8+ repeated-header pages).
    assert!(laid.pages.len() <= 3, "oversized header block inflated page count to {}", laid.pages.len());

    // Header row 0 must appear exactly once, never repeated on continuation pages.
    let row0_count = laid.pages.iter().flat_map(|p| &p.items).filter(|it| matches!(it, Placed::Cell { row: 0, cell: 0, .. })).count();
    assert_eq!(row0_count, 1, "oversized header row repeated {row0_count} times");

    // Every body row (10..16) must be placed within its page bounds (not pushed off the bottom).
    for body_row in 10..16 {
        let placed = laid.pages.iter().find_map(|p| {
            p.items.iter().find_map(|it| match it {
                Placed::Cell { row, cell: 0, rect, .. } if *row == body_row => Some((rect.y, p.body.bottom())),
                _ => None,
            })
        });
        let (y, bottom) = placed.expect("body row must be placed");
        assert!(y < bottom, "body row {body_row} placed off-page at y={y} >= bottom={bottom}");
    }
}

/// Normal-sized repeated table header rows (fitting comfortably in a page) continue to repeat on
/// continuation pages.
#[test]
fn fitting_table_header_block_still_repeats_on_continuation_pages() {
    let mut doc = Document::new();
    let mut t = Table::new(12, 1, 468.0);
    for (ri, row) in t.rows.iter_mut().enumerate() {
        row.props.header = ri == 0;
        row.props.height = Some(if ri == 0 { 40.0 } else { 80.0 });
        row.props.height_rule = HeightRule::AtLeast;
        row.cells[0].blocks = vec![para_block(Paragraph::with_text(&format!("Row {ri}"), Default::default()))];
    }
    doc.insert_block(StoryRef::Body, &Path::top(0), Block::Table(t)).unwrap();

    let laid = lay(&doc);
    assert!(laid.pages.len() >= 2, "table should span at least two pages");
    let row0_count = laid.pages.iter().flat_map(|p| &p.items).filter(|it| matches!(it, Placed::Cell { row: 0, cell: 0, .. })).count();
    assert_eq!(row0_count, laid.pages.len(), "fitting header row should repeat on every page");
}

/// A body wrapping float positioned relative to its paragraph with a negative offset is clamped
/// to the page's top body edge (margin_top), both on its anchor paragraph's first piece and when
/// placed as a late float after an explicit page break.
#[test]
fn body_wrapping_paragraph_float_clamps_to_page_top() {
    let margin_top = 72.0;

    // 1. First-piece anchor on page 1.
    let mut doc = Document::new();
    let mut p = Paragraph::with_text("", Default::default());
    let float = Float { wrap: Wrap::Square, h_rel: Anchor::Column, v_rel: Anchor::Paragraph, x: -72.0, y: -72.0, ..Default::default() };
    p.insert_object(0, float_picture(595.0, 841.0, float), &Default::default()).unwrap();
    doc.body = vec![para_block(p)];

    let laid = lay(&doc);
    let img_y = laid.pages[0]
        .items
        .iter()
        .find_map(|it| match it {
            Placed::Image { rect, .. } => Some(rect.y),
            _ => None,
        })
        .expect("image placed on page 1");
    assert!((img_y - margin_top).abs() < 0.01, "expected clamped y={margin_top}, got {img_y}");

    // 2. Late float anchored after an explicit page break on page 2.
    let mut doc2 = Document::new();
    let mut p2 = Paragraph::with_text(&format!("{}A", wordcraft_doc::para::PAGE_BREAK), Default::default());
    let float2 = Float { wrap: Wrap::Square, h_rel: Anchor::Column, v_rel: Anchor::Paragraph, x: -72.0, y: -72.0, ..Default::default() };
    p2.insert_object(p2.text.len(), float_picture(595.0, 841.0, float2), &Default::default()).unwrap();
    doc2.body = vec![para_block(p2)];

    let laid2 = lay(&doc2);
    assert!(laid2.pages.len() >= 2, "page break should create page 2");
    let img2_y = laid2.pages[1]
        .items
        .iter()
        .find_map(|it| match it {
            Placed::Image { rect, .. } => Some(rect.y),
            _ => None,
        })
        .expect("late image placed on page 2");
    assert!((img2_y - margin_top).abs() < 0.01, "expected clamped late y={margin_top}, got {img2_y}");
}

/// Margin-anchored wrapping floats and BehindText/InFrontOfText paragraph floats are NOT clamped
/// to page_top and may extend into the top margin.
#[test]
fn margin_and_non_wrapping_floats_are_not_clamped_to_page_top() {
    for (v_rel, wrap) in [(Anchor::Margin, Wrap::TopAndBottom), (Anchor::Paragraph, Wrap::BehindText), (Anchor::Paragraph, Wrap::InFrontOfText)] {
        let mut doc = Document::new();
        let mut p = Paragraph::with_text("Body", Default::default());
        let float = Float { wrap, h_rel: Anchor::Margin, v_rel, x: 0.0, y: -72.0, ..Default::default() };
        p.insert_object(0, float_picture(200.0, 60.0, float), &Default::default()).unwrap();
        doc.body = vec![para_block(p)];

        let laid = lay(&doc);
        let img_y = laid.pages[0]
            .items
            .iter()
            .find_map(|it| match it {
                Placed::Image { rect, .. } => Some(rect.y),
                _ => None,
            })
            .expect("image placed");
        assert!(img_y.abs() < 0.01, "expected unclamped y=0.0 for ({v_rel:?}, {wrap:?}), got {img_y}");
    }
}
