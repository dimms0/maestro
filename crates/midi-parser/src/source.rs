use std::{fs::File, ops::Deref, path::Path};

use crate::error::Result;

/// The bytes a [`crate::MidiFile`] reads from.
///
/// All three variants hand out a plain `&[u8]`, so parsing has one code path
/// whether the file is mapped, read into memory, or already owned by the caller.
pub enum Source<'a> {
    /// Streaming mode: the OS pages the file in on demand and can evict it
    /// again, so resident memory stays far below the file size.
    Mapped(memmap2::Mmap),
    Owned(Vec<u8>),
    Borrowed(&'a [u8]),
}

impl Source<'static> {
    pub fn map(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        // Safe as long as nothing else truncates the file while it is mapped.
        let map = unsafe { memmap2::Mmap::map(&file)? };
        Ok(Self::Mapped(map))
    }

    pub fn read(path: &Path) -> Result<Self> {
        Ok(Self::Owned(std::fs::read(path)?))
    }
}

impl Deref for Source<'_> {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        match self {
            Self::Mapped(map) => map,
            Self::Owned(vec) => vec,
            Self::Borrowed(slice) => slice,
        }
    }
}
