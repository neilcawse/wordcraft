//! Saves handed to the host page (`?host=parent`) that mark their document saved only once the
//! page says it stored them (`?ack=1`): which document each came from, and that document's
//! revision then, so only the content of that save counts as saved (later edits stay unsaved).

/// The most saves kept waiting for the page. A page that never answers some of them (a
/// cancelled picker it didn't report) can't make the list grow for as long as the tab is open;
/// a save dropped from it is one the page can no longer confirm, so its document stays unsaved.
pub const MAX_WAITING: usize = 64;

/// A save the page hasn't confirmed yet.
struct Waiting {
    save_id: u64,
    document: u64,
    rev: u64,
}

/// The saves waiting for the page, oldest first.
#[derive(Default)]
pub struct Unacked(Vec<Waiting>);

impl Unacked {
    /// Save `save_id` of `document` at revision `rev` waits for the page.
    pub fn push(&mut self, save_id: u64, document: u64, rev: u64) {
        if self.0.len() >= MAX_WAITING {
            self.0.remove(0);
        }
        self.0.push(Waiting { save_id, document, rev });
    }

    /// The page stored save `save_id`: its document and revision, or `None` for a save that
    /// isn't waiting (unknown, already answered, discarded, or a copy that saves nothing). The
    /// document's older saves are forgotten too: the page won't store them after a newer one.
    pub fn stored(&mut self, save_id: u64) -> Option<(u64, u64)> {
        let at = self.0.iter().position(|w| w.save_id == save_id)?;
        let save = self.0.remove(at);
        self.0.retain(|w| w.document != save.document || w.save_id > save_id);
        Some((save.document, save.rev))
    }

    /// The page will never store save `save_id` (`wordcraft:discard`: the picker was cancelled,
    /// the write failed, a newer save replaced it): forget it; its document stays unsaved.
    pub fn discard(&mut self, save_id: u64) {
        self.0.retain(|w| w.save_id != save_id);
    }

    /// Forget the saves of documents that are no longer open.
    pub fn retain_open(&mut self, is_open: impl Fn(u64) -> bool) {
        self.0.retain(|w| is_open(w.document));
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.0.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_save_gives_its_document_and_revision_and_drops_older_saves_of_it() {
        let mut u = Unacked::default();
        u.push(1, 10, 3);
        u.push(2, 20, 7);
        u.push(3, 10, 5);
        u.push(4, 10, 6);
        assert_eq!(u.stored(3), Some((10, 5)));
        assert_eq!(u.stored(1), None, "an older save of document 10 is no longer confirmed");
        assert_eq!(u.stored(2), Some((20, 7)), "another document's save still is");
        assert_eq!(u.stored(4), Some((10, 6)), "a newer save still is");
        assert_eq!(u.stored(4), None, "a save is confirmed once");
    }

    #[test]
    fn a_discarded_save_is_forgotten_and_never_marks_its_document_saved() {
        let mut u = Unacked::default();
        u.push(1, 10, 3);
        u.push(2, 10, 4);
        u.discard(1);
        assert_eq!(u.len(), 1);
        assert_eq!(u.stored(1), None, "a late `saved` for a discarded save does nothing");
        assert_eq!(u.stored(2), Some((10, 4)));
        u.discard(99);
        assert_eq!(u.len(), 0);
    }

    #[test]
    fn saves_the_page_never_answers_stay_bounded() {
        let mut u = Unacked::default();
        for id in 0..(MAX_WAITING as u64 * 4) {
            u.push(id, 10, id);
        }
        assert_eq!(u.len(), MAX_WAITING);
        assert_eq!(u.stored(0), None, "the oldest went first");
        let newest = MAX_WAITING as u64 * 4 - 1;
        assert_eq!(u.stored(newest), Some((10, newest)));
    }

    #[test]
    fn saves_of_closed_documents_are_forgotten() {
        let mut u = Unacked::default();
        u.push(1, 10, 3);
        u.push(2, 20, 4);
        u.retain_open(|d| d == 20);
        assert_eq!(u.stored(1), None);
        assert_eq!(u.stored(2), Some((20, 4)));
    }
}
