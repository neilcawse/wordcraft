//! Which document a file sent by the host page (`?host=parent`) became, so the host writes a
//! document back only to the file it came from. The bridge hands the app one sent file per frame
//! and looks at the documents after the next frame: a document made with File › New, or opened
//! with the app's own File › Open, is never taken for the sent file, and the host asks where to
//! save it instead of overwriting the file it sent.

/// The document `name` became: the first one not open in `before` whose title starts with the
/// file's stem (a document's title is its file name without the extension). `None` when the
/// file didn't open, or isn't open yet (a password prompt): its document then counts as the
/// user's own and is never written over the sent file.
pub fn opened_doc<'a>(before: &[u64], after: impl IntoIterator<Item = (u64, &'a str)>, name: &str) -> Option<u64> {
    let stem = std::path::Path::new(name).file_stem().and_then(std::ffi::OsStr::to_str).unwrap_or(name);
    after.into_iter().find(|(id, title)| !before.contains(id) && title.starts_with(stem)).map(|(id, _)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_new_document_named_after_the_file_is_the_one_it_became() {
        assert_eq!(opened_doc(&[1], [(1, "Document1"), (2, "Report")], "Report.docx"), Some(2));
        assert_eq!(opened_doc(&[1], [(1, "Document1"), (2, "notes.v2")], "notes.v2.odt"), Some(2));
    }

    #[test]
    fn a_blank_document_replaced_by_the_file_is_a_new_document() {
        // The untouched blank document 1 gave way to the sent file, which got id 2.
        assert_eq!(opened_doc(&[1], [(2, "Report")], "Report.docx"), Some(2));
    }

    #[test]
    fn a_document_the_user_made_is_not_taken_for_a_file_that_failed_to_open() {
        // The sent file was refused; the user's File › New in the same frame made `Document2`.
        assert_eq!(opened_doc(&[1], [(1, "Document1"), (2, "Document2")], "Report.docx"), None);
    }

    #[test]
    fn an_already_open_document_with_the_same_name_is_not_the_new_one() {
        // `Report.docx` was open before; the second copy sent is document 3.
        assert_eq!(opened_doc(&[1, 2], [(1, "Document1"), (2, "Report"), (3, "Report")], "Report.docx"), Some(3));
        assert_eq!(opened_doc(&[1, 2], [(1, "Document1"), (2, "Report")], "Report.docx"), None);
    }
}
