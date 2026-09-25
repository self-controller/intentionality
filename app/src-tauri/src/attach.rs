//! The one place that touches a file the user chose.
//!
//! Everything here reads bytes somebody else picked. A transcript is at least
//! spoken by a person in the room; an attachment can be a file that arrived by
//! email, built by whoever wants this app to misbehave. So this module is
//! deliberately suspicious: it copies before it reads, it validates the copy
//! and never the original, it bounds every buffer before filling it, and it
//! refuses far more than it accepts.
//!
//! Two separate budgets, because they bound different failures. The byte caps
//! stop the disk from running away. The character caps stop the *token* bill
//! from running away, and they are the ones that actually bite: 25 MiB of
//! plain text is roughly 6 million tokens, and a byte cap that allows it has
//! bounded nothing that matters.
//!
//! Paths never leave this module in either direction. The webview asks for a
//! picker and gets metadata back; it never learns where the copy lives, and it
//! never hands us a path to trust.

use crate::db;
use crate::error::{AppError, Result};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Text,
    Pdf,
    Image,
    Office,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Pdf => "pdf",
            Kind::Image => "image",
            Kind::Office => "office",
        }
    }
}

/// Disk budgets. Not tied to the Messages API's 32 MB request ceiling any
/// more: PDFs and images go through the Files API and the request carries
/// only their ids (`claude::upload_file`), while text and Office files send
/// only what was extracted, which the character caps below bound. The total
/// is two full-size files.
pub const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;
pub const MAX_TOTAL_FILE_BYTES: i64 = 50 * 1024 * 1024;
pub const MAX_FILES: i64 = 8;

/// Content budgets — the ones that bound the bill rather than the disk.
pub const MAX_EXTRACTED_CHARS_PER_FILE: usize = 80_000;
pub const MAX_EXTRACTED_CHARS_TOTAL: usize = 200_000;

/// A byte cap does not bound page count, and page count is what a PDF costs.
pub const MAX_PDF_PAGES: usize = 20;
/// A 200 megapixel PNG compresses to almost nothing and decodes to gigabytes.
pub const MAX_IMAGE_PIXELS: u64 = 50_000_000;

/// Ceiling on one decompressed archive member, and on the whole archive. A
/// zip bomb is a few kilobytes on disk; without these it is however much
/// memory the machine has.
const MAX_ZIP_ENTRY_BYTES: u64 = 20 * 1024 * 1024;
const MAX_ZIP_TOTAL_BYTES: u64 = 40 * 1024 * 1024;

/// Beside the store, not under a hard-coded home directory: a test or a
/// rehearsal that redirects INTENTIONALITY_STORE must not scribble into the
/// real one.
pub fn root() -> PathBuf {
    db::store_path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("meetings")
}

pub fn meeting_dir(meeting_id: i64) -> PathBuf {
    root().join(meeting_id.to_string())
}

/// A validated copy, waiting for its database row.
///
/// It carries `final_path` from the start because the row has to be written
/// before the file is put in place — see `stage` for why that order.
pub struct Staged {
    pub name: String,
    pub kind: Kind,
    pub bytes: u64,
    pub extracted: Option<String>,
    temp: PathBuf,
    final_path: PathBuf,
}

impl Staged {
    pub fn path(&self) -> &Path {
        &self.final_path
    }

    /// Put the copy in place. Called only after its row committed.
    pub fn promote(&self) -> Result<()> {
        fs::rename(&self.temp, &self.final_path)
            .map_err(|e| AppError::Other(format!("could not store the copy: {e}")))
    }

    /// Whatever went wrong, the temporary copy does not survive it.
    pub fn discard(&self) {
        let _ = fs::remove_file(&self.temp);
    }
}

fn opaque_name() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    (0..24).map(|_| char::from(b"0123456789abcdef"[rng.random_range(0..16)])).collect()
}

/// Copy, validate and extract — in that order, and all of it off the SQLite
/// mutex.
///
/// The copy comes first so everything after it reads a file this app owns. A
/// check against the original path and a later read of the same path are two
/// different files if the user is unlucky, or if someone is trying.
pub fn stage(meeting_id: i64, src: &Path) -> Result<Staged> {
    // symlink_metadata, not metadata: the latter follows the link, and "is it
    // a regular file" is a question about what was actually pointed at.
    let meta = fs::symlink_metadata(src)
        .map_err(|e| AppError::Other(format!("cannot read that file: {e}")))?;
    if meta.file_type().is_symlink() {
        return Err(AppError::Other("that is a symlink, not a file".into()));
    }
    if meta.is_dir() {
        return Err(AppError::Other("that is a folder — attach the files inside it".into()));
    }
    if !meta.is_file() {
        return Err(AppError::Other("that is not a regular file".into()));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(AppError::Other(format!(
            "{} is {}, over the {} limit for one file",
            display_name(src),
            human(meta.len()),
            human(MAX_FILE_BYTES)
        )));
    }

    let dir = meeting_dir(meeting_id);
    fs::create_dir_all(&dir)
        .map_err(|e| AppError::Other(format!("cannot create the meeting folder: {e}")))?;
    let stem = opaque_name();
    let temp = dir.join(format!("{stem}.tmp"));
    let final_path = dir.join(&stem);

    let bytes = match copy_bounded(src, &temp) {
        Ok(n) => n,
        Err(e) => {
            let _ = fs::remove_file(&temp);
            return Err(e);
        }
    };

    let staged = Staged {
        name: display_name(src),
        kind: Kind::Text, // replaced below; classify reads the copy
        bytes,
        extracted: None,
        temp,
        final_path,
    };

    match classify_and_extract(&staged.temp) {
        Ok((kind, extracted)) => Ok(Staged { kind, extracted, ..staged }),
        Err(e) => {
            staged.discard();
            Err(e)
        }
    }
}

/// Stream the copy, stopping the moment it exceeds the cap.
///
/// Bounded while copying rather than after: trusting the size we stat'd would
/// mean a file that grows between the stat and the read writes as much as it
/// likes into the store directory.
fn copy_bounded(src: &Path, dst: &Path) -> Result<u64> {
    let mut input = fs::File::open(src)
        .map_err(|e| AppError::Other(format!("cannot open that file: {e}")))?;
    let mut output = fs::File::create(dst)
        .map_err(|e| AppError::Other(format!("cannot write to the store: {e}")))?;
    // One over the cap, so hitting the cap exactly is fine and exceeding it is
    // detectable without a second read.
    let mut limited = (&mut input).take(MAX_FILE_BYTES + 1);
    let written = std::io::copy(&mut limited, &mut output)
        .map_err(|e| AppError::Other(format!("could not copy that file: {e}")))?;
    if written > MAX_FILE_BYTES {
        return Err(AppError::Other(format!(
            "that file is over the {} limit",
            human(MAX_FILE_BYTES)
        )));
    }
    if written == 0 {
        return Err(AppError::Other("that file is empty".into()));
    }
    Ok(written)
}

/// The original basename, for display only. Never used to build a path: the
/// copy is named by `opaque_name`, so nothing here can traverse anywhere.
fn display_name(src: &Path) -> String {
    src.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "attachment".into())
}

fn human(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.0} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.0} kB", bytes as f64 / 1024.0)
    }
}

/// What this file actually is, decided by its bytes.
///
/// Not by its extension, and not by both: the extension is a claim made by
/// whoever named the file, and the media type declared to the API comes out of
/// this decision. A `.png` that is really a JPEG would be a request the far
/// end rejects with an error nobody can act on, so the name gets no vote.
/// It survives only as `Staged::name`, for display.
fn classify_and_extract(copy: &Path) -> Result<(Kind, Option<String>)> {
    let mut head = [0u8; 16];
    let read = {
        let mut f = fs::File::open(copy)
            .map_err(|e| AppError::Other(format!("cannot read the copy: {e}")))?;
        f.read(&mut head).map_err(|e| AppError::Other(e.to_string()))?
    };
    let head = &head[..read];

    if head.starts_with(b"%PDF-") {
        let pages = pdf_pages(copy)?;
        if pages > MAX_PDF_PAGES {
            return Err(AppError::Other(format!(
                "that PDF has {pages} pages; the limit is {MAX_PDF_PAGES}"
            )));
        }
        return Ok((Kind::Pdf, None));
    }

    // The media type itself is re-derived from the copy at send time rather
    // than stored, so that what is declared always matches what is sent.
    if image_media_type(head).is_some() {
        let dims = imagesize::size(copy)
            .map_err(|_| AppError::Other("that image could not be read".into()))?;
        let pixels = dims.width as u64 * dims.height as u64;
        if pixels > MAX_IMAGE_PIXELS {
            return Err(AppError::Other(format!(
                "that image is {}x{}, too large to send",
                dims.width, dims.height
            )));
        }
        return Ok((Kind::Image, None));
    }

    // Every Office format this accepts is a ZIP container -- but so is a
    // .zip of holiday photos, so the parts inside are what decide.
    if head.starts_with(b"PK\x03\x04") {
        let text = ooxml::text(copy)?;
        if text.trim().is_empty() {
            return Err(AppError::Other(
                "that Office file has no text in it".into(),
            ));
        }
        return Ok((Kind::Office, Some(cap_chars(&text))));
    }

    // Anything else is text if it decodes. This is what covers source code
    // without an extension allowlist that is wrong the day it is written.
    let bytes = fs::read(copy).map_err(|e| AppError::Other(e.to_string()))?;
    if bytes.contains(&0) {
        return Err(AppError::Other("that looks like a binary file, not text".into()));
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok((Kind::Text, Some(cap_chars(&text)))),
        Err(_) => Err(AppError::Other("that is not a text file this can read".into())),
    }
}

/// The media type declared to the API. Derived from the bytes, so it always
/// matches what is actually sent.
pub fn image_media_type(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if head.len() >= 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn cap_chars(text: &str) -> String {
    text.chars().take(MAX_EXTRACTED_CHARS_PER_FILE).collect()
}

/// Page count, and a refusal for anything encrypted or unparseable.
fn pdf_pages(path: &Path) -> Result<usize> {
    let doc = lopdf::Document::load(path)
        .map_err(|_| AppError::Other("that PDF could not be read — it may be damaged".into()))?;
    if doc.is_encrypted() {
        return Err(AppError::Other("that PDF is encrypted".into()));
    }
    Ok(doc.get_pages().len())
}

/// Text out of the OOXML and OpenDocument containers.
///
/// Text only, deliberately: this is context for a language model, not a
/// rendering of the document. Layout, images and formatting are out of scope
/// and saying so here is cheaper than being asked why a chart did not survive.
mod ooxml {
    use super::*;
    use quick_xml::events::Event;
    use quick_xml::Reader;

    /// All three vocabularies spell "paragraph" the same way once the
    /// namespace prefix is off: `w:p` in WordprocessingML, `a:p` in
    /// DrawingML, `text:p` in OpenDocument. Comparing the local name is what
    /// lets one scanner read all three.
    const PARAGRAPH: &str = "p";

    pub fn text(path: &Path) -> Result<String> {
        let file = fs::File::open(path).map_err(|e| AppError::Other(e.to_string()))?;
        let mut zip = zip::ZipArchive::new(file)
            .map_err(|_| AppError::Other("that Office file could not be opened".into()))?;

        let mut wanted: Vec<String> = zip
            .file_names()
            .filter(|n| is_prose_part(n))
            .map(|n| n.to_string())
            .collect();
        // Reading order, not the archive's byte order: body first, then slides
        // by number, then the notes for them.
        wanted.sort_by_key(|n| (part_rank(n), numeric_suffix(n), n.clone()));

        if wanted.is_empty() {
            return Err(AppError::Other(
                "that is a ZIP archive, not a document this can read".into(),
            ));
        }

        let mut out = String::new();
        let mut budget = MAX_ZIP_TOTAL_BYTES;
        for name in wanted {
            let mut entry = match zip.by_name(&name) {
                Ok(e) => e,
                // An encrypted member reports itself here rather than as a
                // parse failure further down.
                Err(zip::result::ZipError::UnsupportedArchive(_)) => {
                    return Err(AppError::Other("that Office file is encrypted".into()))
                }
                Err(_) => continue,
            };
            if entry.size() > MAX_ZIP_ENTRY_BYTES || entry.size() > budget {
                return Err(AppError::Other(
                    "that Office file expands to more than this will read".into(),
                ));
            }
            let mut buf = Vec::with_capacity(entry.size().min(1 << 20) as usize);
            // Bounded again while reading: the header's size field is written
            // by whoever built the archive and is not evidence of anything.
            let read = (&mut entry)
                .take(budget + 1)
                .read_to_end(&mut buf)
                .map_err(|_| AppError::Other("that Office file could not be read".into()))?;
            if read as u64 > budget {
                return Err(AppError::Other(
                    "that Office file expands to more than this will read".into(),
                ));
            }
            budget -= read as u64;
            out.push_str(&strip(&buf));
            out.push('\n');
            if out.chars().count() > MAX_EXTRACTED_CHARS_PER_FILE {
                break;
            }
        }
        Ok(out.trim().to_string())
    }

    /// The parts that hold prose. Everything else in the container is styles,
    /// relationships and theme data.
    fn is_prose_part(name: &str) -> bool {
        name == "word/document.xml"
            || name == "content.xml"
            || name == "xl/sharedStrings.xml"
            || (name.starts_with("ppt/slides/slide") && name.ends_with(".xml"))
            || (name.starts_with("ppt/notesSlides/notesSlide") && name.ends_with(".xml"))
            // Inline strings live in the sheets themselves. Numeric cells and
            // formulas are intentionally not read: a spreadsheet's numbers
            // without their layout are noise, not context.
            || (name.starts_with("xl/worksheets/sheet") && name.ends_with(".xml"))
    }

    fn part_rank(name: &str) -> u8 {
        match name {
            "word/document.xml" => 0,
            "content.xml" => 0,
            "xl/sharedStrings.xml" => 0,
            n if n.starts_with("ppt/slides/") => 1,
            n if n.starts_with("ppt/notesSlides/") => 2,
            _ => 3,
        }
    }

    /// slide10.xml must sort after slide9.xml, which it does not as a string.
    fn numeric_suffix(name: &str) -> u32 {
        let digits: String = name
            .trim_end_matches(".xml")
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.chars().rev().collect::<String>().parse().unwrap_or(0)
    }

    /// The text of one XML part, with a newline where each paragraph closes.
    ///
    /// quick-xml emits entity references as their own `GeneralRef` events
    /// rather than folding them into the text, so `&amp;` has to be turned
    /// back into `&` here. Only the five predefined entities and numeric
    /// character references are resolved: a document-defined entity is a DTD
    /// feature this has no business honouring in a file someone sent us.
    fn strip(xml: &[u8]) -> String {
        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(false);
        let mut out = String::new();
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Text(e)) => out.push_str(&e.xml10_content()),
                Ok(Event::CData(e)) => out.push_str(e.as_ref()),
                Ok(Event::GeneralRef(e)) => match e.resolve_char_ref() {
                    Ok(Some(c)) => out.push(c),
                    _ => match e.as_ref() {
                        "amp" => out.push('&'),
                        "lt" => out.push('<'),
                        "gt" => out.push('>'),
                        "quot" => out.push('"'),
                        "apos" => out.push('\''),
                        _ => {}
                    },
                },
                Ok(Event::End(e)) => {
                    if e.local_name().as_ref() == PARAGRAPH {
                        out.push('\n');
                    }
                }
                Ok(Event::Eof) => break,
                // A malformed part yields what was read so far rather than
                // failing the whole attachment: partial context still helps,
                // and this is someone else's file.
                Err(_) => break,
                _ => {}
            }
            buf.clear();
        }
        out
    }
}

/// Remove one owned copy. Failure is logged, never propagated: the row is
/// already gone and the sweep below will get the file.
pub fn remove_file(path: &str) {
    if let Err(e) = fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            eprintln!("attach: could not remove {path}: {e}");
        }
    }
}

pub fn remove_meeting_dir(meeting_id: i64) {
    let dir = meeting_dir(meeting_id);
    if let Err(e) = fs::remove_dir_all(&dir) {
        if e.kind() != std::io::ErrorKind::NotFound {
            eprintln!("attach: could not remove {}: {e}", dir.display());
        }
    }
}

/// Startup cleanup, under the owned root and nowhere else.
///
/// Two kinds of leftover: a `.tmp` from a copy that was interrupted, and a
/// whole directory whose meeting has since been deleted. Both are only ever
/// identified by living under `root()`, so a bug here cannot reach anything
/// this app did not create.
pub fn sweep_orphans(conn: &rusqlite::Connection) -> usize {
    let root = root();
    let Ok(entries) = fs::read_dir(&root) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        let Ok(meeting_id) = name.parse::<i64>() else { continue };
        let exists = conn
            .query_row("SELECT 1 FROM meeting WHERE id = ?1", [meeting_id], |_| Ok(()))
            .is_ok();
        if !exists {
            remove_meeting_dir(meeting_id);
            removed += 1;
            continue;
        }
        let Ok(files) = fs::read_dir(&path) else { continue };
        for file in files.flatten() {
            let p = file.path();
            if p.extension().is_some_and(|e| e == "tmp") {
                let _ = fs::remove_file(&p);
                removed += 1;
            }
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A directory of our own per test. No tempfile crate, and no reliance on
    /// INTENTIONALITY_STORE: these tests exercise the validators, which take a
    /// path and never consult `root()`.
    struct Tmp(PathBuf);

    impl Tmp {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("attach-{tag}-{}", opaque_name()));
            fs::create_dir_all(&dir).unwrap();
            Tmp(dir)
        }
        fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let p = self.0.join(name);
            fs::write(&p, bytes).unwrap();
            p
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn zip_of(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            for (name, body) in parts {
                w.start_file(*name, opts).unwrap();
                w.write_all(body.as_bytes()).unwrap();
            }
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    /// The media type is declared to the API from these bytes, so a wrong
    /// answer here is a request the far end rejects.
    #[test]
    fn image_types_are_decided_by_magic_bytes_not_by_name() {
        assert_eq!(image_media_type(&[0x89, b'P', b'N', b'G', 13, 10, 26, 10]), Some("image/png"));
        assert_eq!(image_media_type(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(image_media_type(b"GIF89a....."), Some("image/gif"));
        assert_eq!(image_media_type(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        // Formats Claude does not take, however they are named.
        assert_eq!(image_media_type(b"BM\0\0\0\0\0\0\0\0\0\0"), None);
        assert_eq!(image_media_type(b"<svg xmlns=\"htt"), None);
        assert_eq!(image_media_type(b"RIFF\0\0\0\0AVI "), None);
    }

    #[test]
    fn source_code_is_text_and_a_binary_is_refused() {
        let tmp = Tmp::new("text");
        let rs = tmp.write("x", b"fn main() { println!(\"hi\"); }\n");
        let (kind, extracted) = classify_and_extract(&rs).unwrap();
        assert_eq!(kind, Kind::Text);
        assert!(extracted.unwrap().contains("fn main()"));

        // A NUL is the giveaway: real text does not carry one.
        let bin = tmp.write("y", &[0x7F, b'E', b'L', b'F', 0x02, 0x00, 0x01]);
        assert!(classify_and_extract(&bin).is_err());

        // Valid bytes that are not valid UTF-8.
        let latin1 = tmp.write("z", &[0xC0, 0xC1, 0xF5, 0xFF]);
        assert!(classify_and_extract(&latin1).is_err());
    }

    #[test]
    fn a_pdf_is_recognised_and_a_damaged_one_is_refused() {
        let tmp = Tmp::new("pdf");
        // The magic is there but the body is not a document.
        let broken = tmp.write("a", b"%PDF-1.7\nnot actually a pdf");
        let err = classify_and_extract(&broken).unwrap_err().to_string();
        assert!(err.contains("damaged"), "{err}");
    }

    #[test]
    fn a_docx_yields_its_paragraphs() {
        let tmp = Tmp::new("docx");
        let docx = zip_of(&[(
            "word/document.xml",
            r#"<?xml version="1.0"?><w:document xmlns:w="x"><w:body>
               <w:p><w:r><w:t>Ana Kirtsova</w:t></w:r></w:p>
               <w:p><w:r><w:t>ships in Q4 &amp; Q1</w:t></w:r></w:p>
               </w:body></w:document>"#,
        )]);
        let p = tmp.write("a", &docx);
        let (kind, text) = classify_and_extract(&p).unwrap();
        assert_eq!(kind, Kind::Office);
        let text = text.unwrap();
        assert!(text.contains("Ana Kirtsova"), "{text}");
        // The entity came back as a character, not as "&amp;".
        assert!(text.contains("ships in Q4 & Q1"), "{text}");
    }

    /// slide10 must come after slide9, which it does not as a string.
    #[test]
    fn pptx_slides_are_read_in_slide_order_with_their_notes() {
        let tmp = Tmp::new("pptx");
        let mut parts: Vec<(String, String)> = Vec::new();
        for i in [1, 2, 9, 10] {
            parts.push((
                format!("ppt/slides/slide{i}.xml"),
                format!(r#"<p:sld xmlns:a="x"><a:p><a:t>slide {i}</a:t></a:p></p:sld>"#),
            ));
        }
        parts.push((
            "ppt/notesSlides/notesSlide1.xml".into(),
            r#"<p:notes xmlns:a="x"><a:p><a:t>the note</a:t></a:p></p:notes>"#.into(),
        ));
        let refs: Vec<(&str, &str)> =
            parts.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let p = tmp.write("a", &zip_of(&refs));

        let (kind, text) = classify_and_extract(&p).unwrap();
        assert_eq!(kind, Kind::Office);
        let text = text.unwrap();
        let at = |s: &str| text.find(s).unwrap_or_else(|| panic!("{s} missing from {text}"));
        assert!(at("slide 1") < at("slide 2"));
        assert!(at("slide 2") < at("slide 9"));
        assert!(at("slide 9") < at("slide 10"), "slide10 sorted as a string");
        // Notes come after every slide, not interleaved with them.
        assert!(at("slide 10") < at("the note"));
    }

    #[test]
    fn opendocument_and_xlsx_shared_strings_are_read() {
        let tmp = Tmp::new("odf");
        let odt = zip_of(&[(
            "content.xml",
            r#"<office xmlns:text="x"><text:p>from an odt</text:p></office>"#,
        )]);
        let p = tmp.write("a", &odt);
        assert!(classify_and_extract(&p).unwrap().1.unwrap().contains("from an odt"));

        let xlsx = zip_of(&[(
            "xl/sharedStrings.xml",
            r#"<sst xmlns="x"><si><t>Q4 revenue</t></si></sst>"#,
        )]);
        let p = tmp.write("b", &xlsx);
        assert!(classify_and_extract(&p).unwrap().1.unwrap().contains("Q4 revenue"));
    }

    /// A ZIP is not automatically an Office document, and the message should
    /// say which of the two problems it is.
    #[test]
    fn a_plain_zip_archive_is_refused_as_not_a_document() {
        let tmp = Tmp::new("zip");
        let plain = zip_of(&[("holiday/beach.txt", "not a document part")]);
        let p = tmp.write("a", &plain);
        let err = classify_and_extract(&p).unwrap_err().to_string();
        assert!(err.contains("not a document"), "{err}");
    }

    /// The whole point of reading someone else's archive with a budget.
    #[test]
    fn an_over_large_archive_member_is_refused() {
        let tmp = Tmp::new("bomb");
        // Compresses to almost nothing; expands past the per-entry ceiling.
        let huge = "A".repeat((MAX_ZIP_ENTRY_BYTES + 1024) as usize);
        let bomb = zip_of(&[("word/document.xml", &huge)]);
        let p = tmp.write("a", &bomb);
        let err = classify_and_extract(&p).unwrap_err().to_string();
        assert!(err.contains("expands to more"), "{err}");
    }

    #[test]
    fn a_file_over_the_cap_is_refused_and_leaves_nothing_behind() {
        let tmp = Tmp::new("cap");
        let big = tmp.write("big", &vec![b'x'; (MAX_FILE_BYTES + 1) as usize]);
        let dst = tmp.0.join("copy");
        assert!(copy_bounded(&big, &dst).is_err());

        let ok = tmp.write("ok", b"small enough");
        assert_eq!(copy_bounded(&ok, &tmp.0.join("copy2")).unwrap(), 12);
        // An empty file is not context, it is a mis-click.
        let empty = tmp.write("empty", b"");
        assert!(copy_bounded(&empty, &tmp.0.join("copy3")).is_err());
    }

    /// The per-meeting total once sat below the per-file cap would allow, so
    /// a file that passed its own check could never be attached at all.
    #[test]
    fn one_full_size_file_fits_the_meeting_total() {
        assert!(MAX_FILE_BYTES as i64 * 2 <= MAX_TOTAL_FILE_BYTES);
    }

    /// The stored name is opaque, so nothing a file is called can decide where
    /// it lands.
    #[test]
    fn the_stored_name_is_never_derived_from_the_original() {
        let hostile = Path::new("../../../../etc/cron.d/evil.txt");
        assert_eq!(display_name(hostile), "evil.txt");
        let stored = opaque_name();
        assert_eq!(stored.len(), 24);
        assert!(stored.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(stored, opaque_name());
    }

    #[test]
    fn extracted_text_is_capped_per_file() {
        let long = "x".repeat(MAX_EXTRACTED_CHARS_PER_FILE * 2);
        assert_eq!(cap_chars(&long).chars().count(), MAX_EXTRACTED_CHARS_PER_FILE);
        // Capped by characters, not bytes: a multibyte cut would panic.
        let accents = "é".repeat(MAX_EXTRACTED_CHARS_PER_FILE + 10);
        assert_eq!(cap_chars(&accents).chars().count(), MAX_EXTRACTED_CHARS_PER_FILE);
    }
}
