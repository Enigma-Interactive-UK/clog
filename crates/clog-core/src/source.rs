//! Line-source abstraction. The v1 impl streams a local file via `BufReader`;
//! future impls (mmap for local, socket for the WSL companion) drop in behind
//! the same trait.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::index::LineIndex;
use crate::CoreError;

/// Read-on-demand byte access to a log file.
pub trait LineSource {
    /// Read `len` bytes starting at `start`.
    ///
    /// # Errors
    ///
    /// Bubbles up I/O errors.
    fn read_range(&mut self, start: u64, len: usize) -> io::Result<Vec<u8>>;

    /// File size in bytes, as known at open time.
    fn file_size(&self) -> u64;
}

/// `BufReader`-backed `LineSource`. Re-seeks for every read; the OS page
/// cache makes that cheap for warm files.
pub struct StreamedFile {
    file: File,
    size: u64,
    path: PathBuf,
}

impl StreamedFile {
    /// Open `path` for reading and capture its current size.
    ///
    /// # Errors
    ///
    /// Returns `CoreError::Io` if the file cannot be opened or stat'd.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, CoreError> {
        let path = path.into();
        let file = File::open(&path).map_err(|source| CoreError::Io {
            path: path.clone(),
            source,
        })?;
        let size = file
            .metadata()
            .map_err(|source| CoreError::Io {
                path: path.clone(),
                source,
            })?
            .len();
        Ok(Self { file, size, path })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Build a `LineIndex` for the whole file by streaming from offset 0.
    ///
    /// # Errors
    ///
    /// Returns `CoreError::Io` on read failure.
    pub fn build_line_index(&mut self) -> Result<LineIndex, CoreError> {
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|source| CoreError::Io {
                path: self.path.clone(),
                source,
            })?;
        let reader = BufReader::with_capacity(64 * 1024, &mut self.file);
        LineIndex::build(reader).map_err(|source| CoreError::Io {
            path: self.path.clone(),
            source,
        })
    }

    /// Read the whole file into memory. Used at index time only; the bytes
    /// are dropped after `RecordHeader`s are built.
    ///
    /// # Errors
    ///
    /// Returns `CoreError::Io` on read failure.
    pub fn read_all(&mut self) -> Result<Vec<u8>, CoreError> {
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|source| CoreError::Io {
                path: self.path.clone(),
                source,
            })?;
        let mut buf = Vec::with_capacity(usize::try_from(self.size).unwrap_or(0));
        self.file
            .read_to_end(&mut buf)
            .map_err(|source| CoreError::Io {
                path: self.path.clone(),
                source,
            })?;
        Ok(buf)
    }
}

impl LineSource for StreamedFile {
    fn read_range(&mut self, start: u64, len: usize) -> io::Result<Vec<u8>> {
        self.file.seek(SeekFrom::Start(start))?;
        let mut buf = vec![0u8; len];
        self.file.read_exact(&mut buf)?;
        Ok(buf)
    }

    fn file_size(&self) -> u64 {
        self.size
    }
}
