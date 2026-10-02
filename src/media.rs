//! The images and videos a Claude session made or looked at: the paths its tools touched, and
//! the media that appeared in its folder while it ran. Shown as small thumbnails, made once with
//! Quick Look and cached, so a folder of 4K frames doesn't fill memory.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const IMAGES: [&str; 8] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "heic"];
const VIDEOS: [&str; 4] = ["mp4", "mov", "m4v", "webm"];
/// Folders never worth walking: dependencies, builds, caches, and anything that would make
/// cloud storage download files.
const SKIP: [&str; 14] = [
    ".git",
    "node_modules",
    "target",
    ".next",
    ".turbo",
    ".cache",
    "venv",
    ".venv",
    "__pycache__",
    "Library",
    ".Trash",
    "DerivedData",
    "CloudStorage",
    "Mobile Documents",
];
const MAX_DEPTH: usize = 6;
const MAX_ENTRIES: usize = 30_000;
pub const MAX_ITEMS: usize = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Image,
    Video,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaItem {
    pub path: PathBuf,
    pub kind: Kind,
    pub modified: SystemTime,
    /// A small picture of it, once made.
    pub thumb: Option<PathBuf>,
}

pub fn kind(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if IMAGES.contains(&ext.as_str()) {
        Some(Kind::Image)
    } else if VIDEOS.contains(&ext.as_str()) {
        Some(Kind::Video)
    } else {
        None
    }
}

/// Media files under `dir` changed since `since`, newest first. Doesn't walk the home folder
/// itself, cloud storage, dependency or build folders, and stops after a bounded walk.
pub fn scan(dir: &Path, since: SystemTime) -> Vec<(PathBuf, SystemTime)> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if home.as_deref() == Some(dir) || dir.parent().is_none() {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut entries = 0;
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((folder, depth)) = stack.pop() {
        let Ok(listing) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in listing.filter_map(|entry| entry.ok()) {
            entries += 1;
            if entries > MAX_ENTRIES {
                break;
            }
            let Ok(file_type) = entry.file_type() else { continue };
            let path = entry.path();
            if file_type.is_dir() {
                let name = entry.file_name();
                if depth + 1 < MAX_DEPTH && !SKIP.iter().any(|skip| name == *skip) {
                    stack.push((path, depth + 1));
                }
            } else if file_type.is_file()
                && kind(&path).is_some()
                && let Ok(modified) = entry.metadata().and_then(|meta| meta.modified())
                && modified >= since
            {
                found.push((path, modified));
            }
        }
    }
    found.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));
    found.truncate(MAX_ITEMS);
    found
}

/// The files a tool call named that are images or videos (Write, Edit and Read take `file_path`).
pub fn paths_in(input: &serde_json::Value) -> Option<PathBuf> {
    let path = PathBuf::from(input["file_path"].as_str()?);
    kind(&path).map(|_| path)
}

fn thumbs_dir() -> PathBuf {
    crate::sys::data_dir().join("thumbs")
}

/// A cached 256-pixel picture of `path`, made with Quick Look the first time (it handles
/// both images and videos). Changes to the file make a new one.
pub fn thumbnail(path: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(path).ok()?;
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    meta.len().hash(&mut hasher);
    meta.modified().ok()?.hash(&mut hasher);
    let dir = thumbs_dir();
    let thumb = dir.join(format!("{:016x}.png", hasher.finish()));
    if thumb.is_file() {
        return Some(thumb);
    }
    let work = dir.join(format!("work-{}", std::process::id()));
    std::fs::create_dir_all(&work).ok()?;
    let made = crate::sys::command("/usr/bin/qlmanage")
        .args(["-t", "-s", "256", "-o"])
        .arg(&work)
        .arg(path)
        .output()
        .ok()?;
    let produced = work.join(format!("{}.png", path.file_name()?.to_string_lossy()));
    let result =
        (made.status.success() && produced.is_file() && std::fs::rename(&produced, &thumb).is_ok()).then_some(thumb);
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// Puts the touched paths and the scanned ones together: existing files only, each once,
/// newest first, keeping thumbnails already made.
pub fn merge(touched: &[PathBuf], scanned: &[(PathBuf, SystemTime)], previous: &[MediaItem]) -> Vec<MediaItem> {
    let mut items: Vec<MediaItem> = Vec::new();
    let mut add = |path: &Path, modified: SystemTime| {
        if items.iter().any(|item| item.path == path) {
            return;
        }
        let Some(kind) = kind(path) else { return };
        let thumb = previous
            .iter()
            .find(|item| item.path == path && item.modified == modified)
            .and_then(|item| item.thumb.clone());
        items.push(MediaItem {
            path: path.to_path_buf(),
            kind,
            modified,
            thumb,
        });
    };
    for (path, modified) in scanned {
        add(path, *modified);
    }
    for path in touched {
        if let Ok(modified) = std::fs::metadata(path).and_then(|meta| meta.modified()) {
            add(path, modified);
        }
    }
    items.sort_by_key(|item| std::cmp::Reverse(item.modified));
    items.truncate(MAX_ITEMS);
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn kinds_by_extension() {
        assert_eq!(kind(Path::new("a/frame-01.PNG")), Some(Kind::Image));
        assert_eq!(kind(Path::new("clip.mov")), Some(Kind::Video));
        assert_eq!(kind(Path::new("notes.md")), None);
        let input = serde_json::json!({"file_path": "/x/sprite.webp"});
        assert_eq!(paths_in(&input), Some(PathBuf::from("/x/sprite.webp")));
        assert_eq!(paths_in(&serde_json::json!({"file_path": "/x/a.rs"})), None);
    }

    #[test]
    fn scan_finds_new_media_and_skips_dependency_folders() {
        let dir = std::env::temp_dir().join(format!("signalbox-media-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("out/frames")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(dir.join(".strawberry/media")).unwrap();
        let before = SystemTime::now() - Duration::from_secs(5);
        std::fs::write(dir.join("out/frames/f1.png"), b"x").unwrap();
        std::fs::write(dir.join(".strawberry/media/m.jpg"), b"x").unwrap();
        std::fs::write(dir.join("node_modules/pkg/logo.png"), b"x").unwrap();
        std::fs::write(dir.join("readme.md"), b"x").unwrap();
        let found: Vec<String> = scan(&dir, before)
            .into_iter()
            .map(|(path, _)| path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(found.len(), 2);
        assert!(found.contains(&"f1.png".to_string()) && found.contains(&"m.jpg".to_string()));
        assert!(scan(&dir, SystemTime::now() + Duration::from_secs(60)).is_empty());

        let merged = merge(
            &[dir.join("out/frames/f1.png"), dir.join("gone.png")],
            &scan(&dir, before),
            &[],
        );
        assert_eq!(merged.len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }
}
