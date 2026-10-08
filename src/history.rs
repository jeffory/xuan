use crate::document::Document;

const MAX_STEPS: usize = 64;

#[derive(Clone)]
struct Entry {
    name: String,
    document: Document,
    revision: u64,
}

/// Pixel assets are shared with Arc; only an edited asset is copied.
#[derive(Default)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    pending: Option<Entry>,
    pub revision: u64,
    saved_revision: u64,
    next_revision: u64,
}

impl History {
    pub fn begin(&mut self, name: impl Into<String>, document: &Document) {
        if self.pending.is_none() {
            self.pending = Some(Entry {
                name: name.into(),
                document: document.clone(),
                revision: self.revision,
            });
        }
    }

    /// The document as it was when the open edit began.
    pub fn pending_document(&self) -> Option<&Document> {
        self.pending.as_ref().map(|entry| &entry.document)
    }

    pub fn commit(&mut self) {
        if let Some(entry) = self.pending.take() {
            self.undo.push(entry);
            // How much undo may hold follows the memory; see `crate::limits`.
            let budget = crate::limits::get().history_bytes;
            while self.undo.len() > 1
                && (self.undo.len() > MAX_STEPS || self.retained_bytes() as u64 > budget)
            {
                self.undo.remove(0);
            }
            self.redo.clear();
            self.next_revision += 1;
            self.revision = self.next_revision;
        }
    }

    fn retained_bytes(&self) -> usize {
        let mut seen = std::collections::HashSet::new();
        let mut bytes = 0;
        for entry in &self.undo {
            for layer in &entry.document.layers {
                if let Some(raw) = &layer.raw
                    && seen.insert(std::sync::Arc::as_ptr(&raw.bytes) as usize)
                {
                    bytes += raw.bytes.len();
                }
                if let Some(pixels) = &layer.pixels
                    && seen.insert(std::sync::Arc::as_ptr(pixels) as usize)
                {
                    bytes += pixels.as_raw().len();
                }
                if let Some(mask) = &layer.mask
                    && seen.insert(std::sync::Arc::as_ptr(&mask.pixels) as usize)
                {
                    bytes += mask.pixels.as_raw().len();
                }
            }
            if let Some(selection) = &entry.document.selection
                && seen.insert(std::sync::Arc::as_ptr(selection) as usize)
            {
                bytes += selection.as_raw().len();
            }
        }
        bytes
    }

    pub fn cancel(&mut self, document: &mut Document) {
        if let Some(entry) = self.pending.take() {
            *document = entry.document;
        }
    }

    pub fn undo(&mut self, document: &mut Document) -> bool {
        self.commit();
        if let Some(entry) = self.undo.pop() {
            self.redo.push(Entry {
                name: entry.name,
                document: document.clone(),
                revision: self.revision,
            });
            *document = entry.document;
            self.revision = entry.revision;
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self, document: &mut Document) -> bool {
        if let Some(entry) = self.redo.pop() {
            self.undo.push(Entry {
                name: entry.name,
                document: document.clone(),
                revision: self.revision,
            });
            *document = entry.document;
            self.revision = entry.revision;
            true
        } else {
            false
        }
    }

    pub fn mark_saved(&mut self) {
        self.saved_revision = self.revision;
    }
    /// A newly imported document needs saving but has no earlier document to undo to.
    pub fn mark_modified(&mut self) {
        self.next_revision += 1;
        self.revision = self.next_revision;
    }
    pub fn dirty(&self) -> bool {
        self.revision != self.saved_revision || self.pending.is_some()
    }
    /// Whether a committed edit separates the document from its saved state. Unlike
    /// [`History::dirty`], an open live preview does not count, so titles and tabs get their
    /// bullet when an edit is applied, not when its dialog opens.
    pub fn edited(&self) -> bool {
        self.revision != self.saved_revision
    }
    pub fn undo_name(&self) -> Option<&str> {
        self.undo.last().map(|e| e.name.as_str())
    }
    pub fn redo_name(&self) -> Option<&str> {
        self.redo.last().map(|e| e.name.as_str())
    }
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.undo.iter().rev().map(|e| e.name.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_state_tracks_undo_and_divergent_edits() {
        let mut doc = Document::new(10, 10).unwrap();
        let mut history = History::default();
        history.begin("Rename", &doc);
        doc.layers[0].name = "Renamed".into();
        history.commit();
        history.mark_saved();
        assert!(!history.dirty());
        assert!(history.undo(&mut doc));
        assert!(history.dirty());
        assert_eq!(doc.layers[0].name, "Layer 1");
        assert!(history.redo(&mut doc));
        assert!(!history.dirty());
        history.undo(&mut doc);
        history.begin("Different edit", &doc);
        doc.layers[0].name = "Other".into();
        history.commit();
        assert!(history.dirty());
        assert!(!history.redo(&mut doc));
    }
}
