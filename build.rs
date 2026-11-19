//! The work-view bundle (decision 114) goes into the binary: `ui/work-view/build.sh` builds it into
//! `assets/work-view/`, and this writes the table `src/view.rs` serves it from, each file by its
//! path under `/work-view/`, its media type and its bytes, as the folder holds them. A file of a
//! type the table has no media type for stops the build, so that a new kind of file is looked at.
use std::path::{Path, PathBuf};

fn main() {
    let root = Path::new(&std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"))
        .join("assets/work-view");
    println!("cargo:rerun-if-changed=assets/work-view");
    let root_kind = std::fs::symlink_metadata(&root)
        .expect("assets/work-view")
        .file_type();
    assert!(root_kind.is_dir(), "assets/work-view: a folder, not a link");
    let mut files = Vec::new();
    walk(&root, &mut files);
    files.sort();
    let mut table = String::from("&[\n");
    for path in &files {
        let rel = path.strip_prefix(&root).expect("under the root");
        let name = rel
            .components()
            .map(|c| c.as_os_str().to_str().expect("a UTF-8 name"))
            .collect::<Vec<_>>()
            .join("/");
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let media = media(ext)
            .unwrap_or_else(|| panic!("assets/work-view/{name}: no media type for .{ext} files"));
        table += &format!(
            "    ({name:?}, {media:?}, include_bytes!({:?})),\n",
            path.display().to_string()
        );
    }
    table += "]\n";
    let out =
        PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets it")).join("work_view_files.rs");
    std::fs::write(out, table).expect("the table is written");
}

/// Every file under `dir`, in no order. Each entry is taken by its own type, a link not followed:
/// a link, or anything but a folder or a file, stops the build, so that the binary holds the
/// bundle's own bytes and no file a link points at.
fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let entry = entry.expect("a folder entry");
        let path = entry.path();
        let kind = entry.file_type().expect("a folder entry's type");
        if kind.is_dir() {
            walk(&path, files);
        } else if kind.is_file() {
            files.push(path);
        } else {
            panic!("{}: neither a file nor a folder (a link?)", path.display());
        }
    }
}

/// The media type a browser takes each kind of file of the bundle as.
fn media(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "png" => "image/png",
        "woff2" => "font/woff2",
        "md" => "text/plain; charset=utf-8",
        _ => return None,
    })
}
