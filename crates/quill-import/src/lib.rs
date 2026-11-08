//! Converts documents from other apps into Quill documents. Everything runs
//! locally; source files are only read, never changed.
//!
//! Supported: .docx, .odt, .md,
//! .txt, .rtf, .html, plus folders and .zip archives of any of these.

mod builder;
mod docx;
mod odt;
mod rtf;
mod text;
mod xml;

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use quill_doc::Document;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("couldn't read archive: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("malformed XML: {0}")]
    Xml(String),
    #[error("{0}")]
    Invalid(String),
}

/// One converted document.
#[derive(Debug)]
pub struct Imported {
    pub doc: Document,
    /// Comments from the source, to become board text.
    pub notes: Vec<String>,
}

#[derive(Debug)]
pub struct Item {
    /// Where it came from, for the progress list ("Drive/Stories/x.docx").
    pub source: String,
    /// The folder it was in, when folders become collections.
    pub collection: Option<String>,
    pub result: Result<Imported, Error>,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub folders_as_collections: bool,
    pub comments_as_notes: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { folders_as_collections: true, comments_as_notes: true }
    }
}

const EXTENSIONS: &[&str] = &["docx", "odt", "md", "markdown", "txt", "text", "rtf", "html", "htm"];

/// Folder names that only describe where an export came from.
const NOT_COLLECTIONS: &[&str] = &["takeout", "drive", "my drive", "google docs"];

pub fn is_supported(path: &Path) -> bool {
    extension(path).is_some_and(|e| EXTENSIONS.contains(&e.as_str()) || e == "zip") || path.is_dir()
}

/// Imports files, folders (recursively) and zip archives. Unsupported files
/// inside folders and archives are skipped silently; each supported one
/// yields an [`Item`], successful or not.
pub fn import_paths(paths: &[PathBuf], options: Options) -> Vec<Item> {
    let mut items = Vec::new();
    for path in paths {
        if path.is_dir() {
            let root = path.file_name().map(|n| n.to_string_lossy().into_owned());
            walk_dir(path, path, root.as_deref(), options, &mut items);
        } else if extension(path).as_deref() == Some("zip") {
            import_zip(path, options, &mut items);
        } else {
            let source = display_name(path);
            let result = std::fs::read(path).map_err(Error::from).and_then(|bytes| convert(path, &bytes, options));
            items.push(Item { source, collection: None, result });
        }
    }
    items
}

/// Converts one file's bytes according to its extension.
pub fn convert(path: &Path, bytes: &[u8], options: Options) -> Result<Imported, Error> {
    let title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into());
    let ext = extension(path).unwrap_or_default();
    let mut imported = match ext.as_str() {
        "docx" => docx::import(&title, bytes)?,
        "odt" => odt::import(&title, bytes)?,
        "rtf" => rtf::import(&title, bytes)?,
        "md" | "markdown" => Imported { doc: text::markdown(&title, bytes), notes: Vec::new() },
        "html" | "htm" => Imported { doc: text::html(&title, bytes), notes: Vec::new() },
        "txt" | "text" => Imported { doc: text::plain(&title, bytes), notes: Vec::new() },
        other => return Err(Error::Invalid(format!("unsupported file type .{other}"))),
    };
    if !options.comments_as_notes {
        imported.notes.clear();
    }
    Ok(imported)
}

fn walk_dir(root: &Path, dir: &Path, root_name: Option<&str>, options: Options, items: &mut Vec<Item>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        let hidden = path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
        if hidden {
            continue;
        }
        if path.is_dir() {
            walk_dir(root, &path, root_name, options, items);
        } else if extension(&path).is_some_and(|e| EXTENSIONS.contains(&e.as_str())) {
            let folder = path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
            let collection = options.folders_as_collections.then(|| folder.or(root_name.map(String::from))).flatten();
            let source = path.strip_prefix(root.parent().unwrap_or(root)).unwrap_or(&path).display().to_string();
            let result = std::fs::read(&path).map_err(Error::from).and_then(|bytes| convert(&path, &bytes, options));
            items.push(Item { source, collection: collection.filter(|c| !is_export_folder(c)), result });
        }
    }
}

fn import_zip(path: &Path, options: Options, items: &mut Vec<Item>) {
    let archive = std::fs::read(path).map_err(Error::from).and_then(|bytes| {
        let zip = zip::ZipArchive::new(Cursor::new(bytes))?;
        Ok(zip)
    });
    let mut zip = match archive {
        Ok(zip) => zip,
        Err(err) => {
            items.push(Item { source: display_name(path), collection: None, result: Err(err) });
            return;
        }
    };
    for i in 0..zip.len() {
        let Ok(mut file) = zip.by_index(i) else { continue };
        if file.is_dir() {
            continue;
        }
        let Ok(name) = file.name().map(|n| PathBuf::from(n.as_ref())) else { continue };
        let hidden = name.components().any(|c| c.as_os_str().to_string_lossy().starts_with(['.', '_']));
        if hidden || !extension(&name).is_some_and(|e| EXTENSIONS.contains(&e.as_str())) {
            continue;
        }
        let mut bytes = Vec::new();
        let result = file.read_to_end(&mut bytes).map_err(Error::from).and_then(|_| convert(&name, &bytes, options));
        let folder = name.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
        let collection = options.folders_as_collections.then_some(folder).flatten().filter(|c| !is_export_folder(c));
        items.push(Item { source: name.display().to_string(), collection, result });
    }
}

fn is_export_folder(name: &str) -> bool {
    NOT_COLLECTIONS.contains(&name.to_ascii_lowercase().as_str())
}

fn extension(path: &Path) -> Option<String> {
    path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase())
}

fn display_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takeout_zip_maps_folders_to_collections() {
        let docx_bytes = docx::tests::docx(&[(
            "word/document.xml",
            r#"<w:document xmlns:w="w"><w:body><w:p><w:r><w:t>Dear Grandma</w:t></w:r></w:p></w:body></w:document>"#,
        )]);
        let mut out = Cursor::new(Vec::new());
        {
            use std::io::Write;
            let mut zip = zip::ZipWriter::new(&mut out);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("Takeout/Drive/Letters/Letter to Grandma.docx", opts).unwrap();
            zip.write_all(&docx_bytes).unwrap();
            zip.start_file("Takeout/Drive/Loose note.txt", opts).unwrap();
            zip.write_all(b"just text").unwrap();
            zip.start_file("Takeout/Drive/photo.jpg", opts).unwrap();
            zip.write_all(b"\xff\xd8").unwrap();
            zip.start_file("__MACOSX/Takeout/._junk.txt", opts).unwrap();
            zip.write_all(b"x").unwrap();
            zip.finish().unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("takeout.zip");
        std::fs::write(&path, out.into_inner()).unwrap();

        let items = import_paths(&[path], Options::default());
        assert_eq!(items.len(), 2, "unsupported and hidden entries are skipped");
        assert_eq!(items[0].collection.as_deref(), Some("Letters"));
        assert_eq!(items[0].result.as_ref().unwrap().doc.title, "Letter to Grandma");
        assert_eq!(items[0].result.as_ref().unwrap().doc.plain_text(), "Dear Grandma");
        assert_eq!(items[1].collection, None, "Drive is not a collection");
    }

    #[test]
    fn folders_import_recursively_and_report_failures() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Writing");
        std::fs::create_dir_all(root.join("Essays")).unwrap();
        std::fs::write(root.join("Essays/On Quiet.md"), "# On Quiet\n\nText").unwrap();
        std::fs::write(root.join("todo.txt"), "milk").unwrap();
        std::fs::write(root.join("broken.docx"), "not a zip").unwrap();
        std::fs::write(root.join(".hidden.txt"), "x").unwrap();

        let items = import_paths(&[root], Options::default());
        let mut summary: Vec<_> =
            items.iter().map(|i| (i.source.clone(), i.collection.clone(), i.result.is_ok())).collect();
        summary.sort();
        assert_eq!(
            summary,
            [
                ("Writing/Essays/On Quiet.md".into(), Some("Essays".into()), true),
                ("Writing/broken.docx".into(), Some("Writing".into()), false),
                ("Writing/todo.txt".into(), Some("Writing".into()), true),
            ]
        );

        let flat = import_paths(
            &[dir.path().join("Writing")],
            Options { folders_as_collections: false, comments_as_notes: true },
        );
        assert!(flat.iter().all(|i| i.collection.is_none()));
    }
}
