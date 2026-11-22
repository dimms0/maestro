//! MIDI files inside archives (zip, 7z, rar, tar) and compressed files (gz,
//! xz, bz2, zst, also around a tar). The chosen entry is decompressed into an
//! unnamed temp file and mapped, so the parser sees an ordinary mapped file.

use std::{
    fmt::Display,
    fs::File,
    io::{self, BufReader, Read},
    path::Path,
};

use crate::{error::Error, error::Result, source::Source};

const MIDI_EXTENSIONS: [&str; 4] = ["mid", "midi", "kar", "rmi"];

type Decoder = fn(File) -> io::Result<Box<dyn Read>>;

enum Kind {
    Zip,
    SevenZip,
    Rar,
    Stream(Decoder),
}

fn plain(f: File) -> io::Result<Box<dyn Read>> {
    Ok(Box::new(BufReader::new(f)))
}
fn gz(f: File) -> io::Result<Box<dyn Read>> {
    Ok(Box::new(flate2::read::MultiGzDecoder::new(BufReader::new(
        f,
    ))))
}
fn xz(f: File) -> io::Result<Box<dyn Read>> {
    Ok(Box::new(lzma_rust2::XzReader::new(BufReader::new(f), true)))
}
fn bz2(f: File) -> io::Result<Box<dyn Read>> {
    Ok(Box::new(bzip2::read::MultiBzDecoder::new(BufReader::new(
        f,
    ))))
}
fn zst(f: File) -> io::Result<Box<dyn Read>> {
    Ok(Box::new(zstd::Decoder::new(f)?))
}

fn err(e: impl Display) -> Error {
    Error::Archive(e.to_string())
}

fn head(mut r: impl Read) -> io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(512);
    r.by_ref().take(512).read_to_end(&mut buf)?;
    Ok(buf)
}

fn is_tar(head: &[u8]) -> bool {
    head.get(257..262) == Some(b"ustar")
}

fn kind(path: &Path) -> Result<Option<Kind>> {
    let head = head(File::open(path)?)?;
    Ok(Some(match head.as_slice() {
        [b'P', b'K', 3, 4, ..] => Kind::Zip,
        [b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C, ..] => Kind::SevenZip,
        [b'R', b'a', b'r', b'!', 0x1A, 7, ..] => Kind::Rar,
        [0x1F, 0x8B, ..] => Kind::Stream(gz),
        [0xFD, b'7', b'z', b'X', b'Z', 0, ..] => Kind::Stream(xz),
        [b'B', b'Z', b'h', ..] => Kind::Stream(bz2),
        [0x28, 0xB5, 0x2F, 0xFD, ..] => Kind::Stream(zst),
        h if is_tar(h) => Kind::Stream(plain),
        _ => return Ok(None),
    }))
}

fn is_midi(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| MIDI_EXTENSIONS.iter().any(|m| e.eq_ignore_ascii_case(m)))
}

/// The MIDI files in `path`, or `None` when it is neither an archive nor a
/// compressed file. A bare compressed file (`song.mid.xz`) holds one entry,
/// named after it without the compression extension.
pub fn list(path: &Path) -> Result<Option<Vec<String>>> {
    let Some(kind) = kind(path)? else {
        return Ok(None);
    };
    let names: Vec<String> = match kind {
        Kind::Zip => {
            let zip = zip::ZipArchive::new(File::open(path)?).map_err(err)?;
            zip.file_names()
                .map(|n| n.map(|n| n.into_owned()))
                .collect::<std::result::Result<_, _>>()
                .map_err(err)?
        }
        Kind::SevenZip => {
            let archive = sevenz_rust2::Archive::open(path).map_err(err)?;
            archive
                .files
                .into_iter()
                .filter(|e| e.has_stream)
                .map(|e| e.name)
                .collect()
        }
        Kind::Rar => unrar::Archive::new(path)
            .open_for_listing()
            .map_err(err)?
            .map(|h| h.map(|h| h.filename.to_string_lossy().into_owned()))
            .collect::<std::result::Result<_, _>>()
            .map_err(err)?,
        Kind::Stream(decode) if is_tar(&head(decode(File::open(path)?)?)?) => {
            let mut tar = tar::Archive::new(decode(File::open(path)?)?);
            tar.entries()?
                .map(|e| Ok(e?.path()?.to_string_lossy().into_owned()))
                .collect::<io::Result<_>>()?
        }
        Kind::Stream(_) => {
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
            return Ok(Some(stem.into_iter().collect()));
        }
    };
    Ok(Some(names.into_iter().filter(|n| is_midi(n)).collect()))
}

/// Decompresses `entry` (a name from [`list`]) out of `path`.
pub(crate) fn open(path: &Path, entry: &str) -> Result<Source<'static>> {
    let missing = || Error::NotInArchive(entry.to_string());
    let mut out = tempfile::tempfile()?;
    match kind(path)?.ok_or_else(|| err("not an archive or compressed file"))? {
        Kind::Zip => {
            let mut zip = zip::ZipArchive::new(File::open(path)?).map_err(err)?;
            io::copy(&mut zip.by_name(entry).map_err(|_| missing())?, &mut out)?;
        }
        Kind::SevenZip => {
            let mut found = false;
            let mut reader =
                sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty())
                    .map_err(err)?;
            reader
                .for_each_entries_with_decision(|e, r| {
                    if e.name() != entry {
                        return Ok(sevenz_rust2::EntryDecision::Continue);
                    }
                    io::copy(r, &mut out)?;
                    found = true;
                    Ok(sevenz_rust2::EntryDecision::Stop)
                })
                .map_err(err)?;
            if !found {
                return Err(missing());
            }
        }
        Kind::Rar => {
            // unrar only extracts to a path
            let tmp = tempfile::NamedTempFile::new()?;
            let mut archive = unrar::Archive::new(path)
                .open_for_processing()
                .map_err(err)?;
            loop {
                let header = archive.read_header().map_err(err)?.ok_or_else(missing)?;
                if header.entry().filename.to_string_lossy() == entry {
                    header.extract_to(tmp.path()).map_err(err)?;
                    out = tmp.reopen()?;
                    break;
                }
                archive = header.skip().map_err(err)?;
            }
        }
        Kind::Stream(decode) if is_tar(&head(decode(File::open(path)?)?)?) => {
            let mut tar = tar::Archive::new(decode(File::open(path)?)?);
            let mut found = tar
                .entries()?
                .find(|e| {
                    e.as_ref()
                        .is_ok_and(|e| e.path().is_ok_and(|p| p.to_string_lossy() == entry))
                })
                .ok_or_else(missing)??;
            io::copy(&mut found, &mut out)?;
        }
        Kind::Stream(decode) => {
            io::copy(&mut decode(File::open(path)?)?, &mut out)?;
        }
    }
    // Safe: the file is unnamed, so nothing else can truncate it.
    Ok(Source::Mapped(unsafe { memmap2::Mmap::map(&out)? }))
}
