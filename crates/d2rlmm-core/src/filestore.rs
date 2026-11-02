//! The in-memory data tree an install builds.
//!
//! Every mod reads and writes game files; nothing reaches the disk until all of them
//! have run. That buys three things: a mod that throws leaves the previous install
//! intact, mods can layer edits onto one file in order (the merge everyone relies
//! on), and a failed write mid-flush cannot leave a half-written mod folder.
//!
//! Keys are case-folded with `/` separators because D2R's own paths are
//! case-insensitive on Windows and mods are written there: a mod that reads
//! `Global\Excel\Misc.txt` after another wrote `global/excel/misc.txt` must see the
//! same file, or the merge silently splits in two.
//!
//! The operation log is not bookkeeping for its own sake: it is what lets the
//! uninstall path know whether a file came from the game (restore it) or from a mod
//! (delete it), and it produces the "this mod overwrote that one" warnings that
//! explain most mysterious mod conflicts.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Read,
    Write,
    Extract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningKind {
    /// Two mods wrote the same file, and the later one never read it first — so it
    /// probably clobbered the earlier mod's changes.
    Conflict,
    /// A mod wrote a game text/JSON file without reading it first, so game updates
    /// to that file are being discarded.
    OverwritesGameFile,
}

#[derive(Debug, Clone)]
pub struct Warning {
    pub kind: WarningKind,
    pub path: String,
    pub mod_id: String,
    pub others: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct FileEntry {
    data: Option<Vec<u8>>,
    exists: bool,
    extracted: bool,
    modified: bool,
    ops: Vec<(String, OpKind)>,
}

impl FileEntry {
    fn written_by(&self, except: &str) -> Vec<String> {
        let mut ids: Vec<String> = self
            .ops
            .iter()
            .filter(|(id, kind)| id != except && *kind == OpKind::Write)
            .map(|(id, _)| id.clone())
            .collect();
        ids.dedup();
        ids
    }
}

#[derive(Debug, Default)]
pub struct FileStore {
    files: BTreeMap<String, FileEntry>,
}

/// Case-folded, slash-separated key for a game path.
pub fn key(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches('/').to_lowercase()
}

impl FileStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn entry(&mut self, path: &str) -> &mut FileEntry {
        self.files.entry(key(path)).or_default()
    }

    /// Reads an entry without creating one.
    fn peek(&self, path: &str) -> Option<&FileEntry> {
        self.files.get(&key(path))
    }

    pub fn exists(&self, path: &str) -> bool {
        self.peek(path).map(|e| e.exists).unwrap_or(false)
    }

    pub fn is_extracted(&self, path: &str) -> bool {
        self.peek(path).map(|e| e.extracted).unwrap_or(false)
    }

    pub fn data(&self, path: &str) -> Option<&[u8]> {
        self.peek(path).and_then(|e| e.data.as_deref())
    }

    /// Records that vanilla data for `path` has been pulled into memory.
    pub fn insert_extracted(&mut self, path: &str, data: Vec<u8>, mod_id: &str) {
        let entry = self.entry(path);
        entry.data = Some(data);
        entry.exists = true;
        entry.extracted = true;
        entry.ops.push((mod_id.to_string(), OpKind::Extract));
    }

    /// Records a read, so a later write by this mod counts as read-modify-write.
    pub fn note_read(&mut self, path: &str, mod_id: &str) {
        let entry = self.entry(path);
        entry.ops.push((mod_id.to_string(), OpKind::Read));
    }

    /// Stores new content and records the write, returning any warning it deserves.
    pub fn write(&mut self, path: &str, data: Vec<u8>, mod_id: &str, is_game_file: bool) -> Option<Warning> {
        let path_key = key(path);
        // Warning logic runs on the state *before* this write, which is why it is
        // computed first.
        let warning = {
            let (read_or_wrote, earlier_writers) = match self.peek(path) {
                Some(entry) => {
                    let read_or_wrote = entry
                        .ops
                        .iter()
                        .any(|(id, kind)| id == mod_id && *kind != OpKind::Extract);
                    let others = if read_or_wrote {
                        Vec::new()
                    } else {
                        entry.written_by(mod_id)
                    };
                    (read_or_wrote, others)
                }
                None => (false, Vec::new()),
            };
            if !earlier_writers.is_empty() {
                Some(Warning {
                    kind: WarningKind::Conflict,
                    path: path_key.clone(),
                    mod_id: mod_id.to_string(),
                    others: earlier_writers,
                })
            } else if !read_or_wrote
                && is_game_file
                && (path_key.ends_with(".txt") || path_key.ends_with(".json"))
            {
                // Only a *blind* write is worth warning about; a mod that read the
                // file first is merging, which is what every well-behaved mod does.
                Some(Warning {
                    kind: WarningKind::OverwritesGameFile,
                    path: path_key.clone(),
                    mod_id: mod_id.to_string(),
                    others: Vec::new(),
                })
            } else {
                None
            }
        };

        let entry = self.entry(path);
        entry.data = Some(data);
        entry.exists = true;
        entry.modified = true;
        entry.ops.push((mod_id.to_string(), OpKind::Write));
        warning
    }

    /// Marks a file as deleted by a mod, so the flush removes it instead of writing.
    pub fn delete(&mut self, path: &str, mod_id: &str) {
        let entry = self.entry(path);
        entry.data = None;
        entry.exists = false;
        entry.modified = true;
        entry.ops.push((mod_id.to_string(), OpKind::Write));
    }

    /// Files with content to write, in key order.
    pub fn modified_files(&self) -> Vec<(String, Vec<u8>)> {
        self.files
            .iter()
            .filter(|(_, e)| e.modified)
            .filter_map(|(path, e)| e.data.clone().map(|data| (path.clone(), data)))
            .collect()
    }

    /// Paths a mod touched that ended up with no content (deletions).
    pub fn modified_paths(&self) -> Vec<String> {
        self.files
            .iter()
            .filter(|(_, e)| e.modified)
            .map(|(path, _)| path.clone())
            .collect()
    }

    pub fn modified_count(&self) -> usize {
        self.files.values().filter(|e| e.modified).count()
    }

    /// Files the install pulled out of the game (as opposed to ones a mod created).
    pub fn extracted_paths(&self) -> Vec<String> {
        self.files
            .iter()
            .filter(|(_, e)| e.modified && e.extracted)
            .map(|(path, _)| path.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_case_insensitive() {
        let mut store = FileStore::new();
        store.write("Global\\Excel\\Misc.txt", b"a".to_vec(), "m1", true);
        assert!(store.exists("global/excel/misc.txt"));
        assert_eq!(store.data("GLOBAL/EXCEL/MISC.TXT"), Some(&b"a"[..]));
    }

    #[test]
    fn second_writer_without_read_is_a_conflict() {
        let mut store = FileStore::new();
        // A non-game path so the "blind overwrite" warning does not fire instead.
        assert!(store.write("hd/x.bin", b"1".to_vec(), "a", false).is_none());
        let warning = store.write("hd/x.bin", b"2".to_vec(), "b", false).unwrap();
        assert_eq!(warning.kind, WarningKind::Conflict);
        assert_eq!(warning.others, vec!["a".to_string()]);
    }

    #[test]
    fn read_modify_write_is_not_a_conflict() {
        let mut store = FileStore::new();
        store.write("hd/x.bin", b"1".to_vec(), "a", false);
        store.note_read("hd/x.bin", "b");
        assert!(store.write("hd/x.bin", b"2".to_vec(), "b", false).is_none());
    }

    #[test]
    fn writing_untouched_game_text_warns() {
        let mut store = FileStore::new();
        let warning = store.write("global/excel/misc.txt", b"x".to_vec(), "a", true).unwrap();
        assert_eq!(warning.kind, WarningKind::OverwritesGameFile);
    }

    #[test]
    fn binary_assets_do_not_warn() {
        let mut store = FileStore::new();
        assert!(store.write("hd/items/flip/a.webm", b"x".to_vec(), "a", true).is_none());
    }

    #[test]
    fn tracks_modified_and_extracted() {
        let mut store = FileStore::new();
        store.insert_extracted("global/a.txt", b"1".to_vec(), "a");
        store.write("global/a.txt", b"2".to_vec(), "a", true);
        store.write("global/b.txt", b"3".to_vec(), "a", true);
        assert_eq!(store.modified_count(), 2);
        assert_eq!(store.extracted_paths(), vec!["global/a.txt".to_string()]);
        assert_eq!(store.modified_files().len(), 2);
    }
}
