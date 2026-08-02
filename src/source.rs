//! Owns decoded source text, assigns stable file IDs, and converts byte
//! offsets to line/column positions, per `docs/roadmap.md` §2.1 and Milestone 1.
//!
//! Source files are assumed to be UTF-8 text; `docs/spec.md` does not state this
//! explicitly, but every text-level construct it defines (`str`, `String`,
//! identifiers, string literals) is specified in terms of Unicode text, so
//! this is the natural reading rather than a new decision.

use std::fmt;
use std::path::{Path, PathBuf};

/// Identifies one loaded source file. Stable for the lifetime of the
/// [`SourceManager`] that issued it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileId(u32);

impl FileId {
    #[must_use]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A byte-offset range within one source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    pub file: FileId,
    pub start: u32,
    pub end: u32,
}

impl Span {
    #[must_use]
    pub fn new(file: FileId, start: u32, end: u32) -> Self {
        debug_assert!(start <= end, "span start must not exceed its end");
        Self { file, start, end }
    }
}

/// A one-based line and column position, for diagnostic display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    pub line: u32,
    pub column: u32,
}

struct SourceFile {
    path: PathBuf,
    text: String,
    /// Byte offset of the start of each line; `line_starts[0] == 0`.
    line_starts: Vec<u32>,
}

fn line_starts(text: &str) -> Vec<u32> {
    let mut starts = vec![0u32];
    for (offset, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(u32::try_from(offset + 1).expect("source file larger than 4GiB"));
        }
    }
    starts
}

/// A source file could not be read as UTF-8 text.
#[derive(Debug)]
pub struct SourceReadError {
    pub path: PathBuf,
    pub cause: std::io::Error,
}

impl fmt::Display for SourceReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot read {}: {}", self.path.display(), self.cause)
    }
}

impl std::error::Error for SourceReadError {}

/// Owns every source file read during one compilation.
#[derive(Default)]
pub struct SourceManager {
    files: Vec<SourceFile>,
}

impl SourceManager {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads `path` as UTF-8 source text and assigns it a new [`FileId`].
    pub fn load_file(&mut self, path: &Path) -> Result<FileId, SourceReadError> {
        let text = std::fs::read_to_string(path).map_err(|cause| SourceReadError {
            path: path.to_path_buf(),
            cause,
        })?;
        Ok(self.add_text(path.to_path_buf(), text))
    }

    /// Registers already-decoded text as a source file. Used by tests, and by
    /// any future caller that obtains source text through another path (an
    /// editor buffer, for example) rather than reading it from disk.
    pub fn add_text(&mut self, path: PathBuf, text: String) -> FileId {
        let starts = line_starts(&text);
        self.files.push(SourceFile {
            path,
            text,
            line_starts: starts,
        });
        FileId(u32::try_from(self.files.len() - 1).expect("too many source files"))
    }

    #[must_use]
    pub fn path(&self, file: FileId) -> &Path {
        &self.files[file.0 as usize].path
    }

    #[must_use]
    pub fn text(&self, file: FileId) -> &str {
        &self.files[file.0 as usize].text
    }

    /// Loaded files in stable ID order.
    pub fn files(&self) -> impl Iterator<Item = (FileId, &Path)> {
        self.files
            .iter()
            .enumerate()
            .map(|(index, file)| (FileId(index as u32), file.path.as_path()))
    }

    #[must_use]
    pub fn snippet(&self, span: Span) -> &str {
        &self.text(span.file)[span.start as usize..span.end as usize]
    }

    /// Converts a byte offset into a one-based line/column position.
    #[must_use]
    pub fn line_col(&self, file: FileId, offset: u32) -> LineCol {
        let starts = &self.files[file.0 as usize].line_starts;
        let line_index = match starts.binary_search(&offset) {
            Ok(exact) => exact,
            Err(insert_at) => insert_at - 1,
        };
        let line_start = starts[line_index];
        LineCol {
            line: u32::try_from(line_index + 1).expect("line count overflow"),
            column: offset - line_start + 1,
        }
    }
}

/// Lets `codespan_reporting::term::emit` render a [`Diagnostic`](crate::diagnostics::Diagnostic)
/// directly against this manager's file table — see `docs/ledger.md` §18.
impl<'a> codespan_reporting::files::Files<'a> for SourceManager {
    type FileId = FileId;
    type Name = String;
    type Source = &'a str;

    fn name(&'a self, id: FileId) -> Result<Self::Name, codespan_reporting::files::Error> {
        Ok(self.path(id).display().to_string())
    }

    fn source(&'a self, id: FileId) -> Result<Self::Source, codespan_reporting::files::Error> {
        Ok(self.text(id))
    }

    fn line_index(
        &'a self,
        id: FileId,
        byte_index: usize,
    ) -> Result<usize, codespan_reporting::files::Error> {
        let offset = u32::try_from(byte_index).unwrap_or(u32::MAX);
        Ok(self.line_col(id, offset).line as usize - 1)
    }

    fn line_range(
        &'a self,
        id: FileId,
        line_index: usize,
    ) -> Result<std::ops::Range<usize>, codespan_reporting::files::Error> {
        let file = &self.files[id.0 as usize];
        let start = *file.line_starts.get(line_index).ok_or(
            codespan_reporting::files::Error::LineTooLarge {
                given: line_index,
                max: file.line_starts.len().saturating_sub(1),
            },
        )? as usize;
        let end = file
            .line_starts
            .get(line_index + 1)
            .map_or(file.text.len(), |&next| next as usize);
        Ok(start..end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_finds_first_line() {
        let mut sources = SourceManager::new();
        let file = sources.add_text(PathBuf::from("test.elx"), "abc\ndef\n".to_string());
        assert_eq!(sources.line_col(file, 0), LineCol { line: 1, column: 1 });
        assert_eq!(sources.line_col(file, 2), LineCol { line: 1, column: 3 });
    }

    #[test]
    fn line_col_finds_second_line() {
        let mut sources = SourceManager::new();
        let file = sources.add_text(PathBuf::from("test.elx"), "abc\ndef\n".to_string());
        assert_eq!(sources.line_col(file, 4), LineCol { line: 2, column: 1 });
        assert_eq!(sources.line_col(file, 6), LineCol { line: 2, column: 3 });
    }

    #[test]
    fn snippet_returns_span_text() {
        let mut sources = SourceManager::new();
        let file = sources.add_text(PathBuf::from("test.elx"), "hello world".to_string());
        assert_eq!(sources.snippet(Span::new(file, 0, 5)), "hello");
    }

    #[test]
    fn distinct_files_get_distinct_ids() {
        let mut sources = SourceManager::new();
        let first = sources.add_text(PathBuf::from("a.elx"), String::new());
        let second = sources.add_text(PathBuf::from("b.elx"), String::new());
        assert_ne!(first, second);
    }
}
