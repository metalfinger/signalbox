//! Claude Code's own transcript of a session (`~/.claude/projects/<folder>/<id>.jsonl`), read as
//! it grows for what the side panel needs: the input of every tool Claude calls, which names
//! the images and videos it makes or looks at. Lines are appended as the session runs, so a
//! reader keeps its place and reads only what is new.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Where Claude Code keeps the transcript of `session_id`, started in `cwd`.
pub fn transcript_path(cwd: &Path, session_id: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let projects = home.join(".claude").join("projects");
    let file = format!("{session_id}.jsonl");
    let folder: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let direct = projects.join(folder).join(&file);
    if direct.is_file() {
        return Some(direct);
    }
    std::fs::read_dir(&projects)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path().join(&file))
        .find(|path| path.is_file())
}

pub struct TranscriptReader {
    path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
}

impl TranscriptReader {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset: 0,
            partial: Vec::new(),
        }
    }

    /// The inputs of the tools called in what was appended since the last call. A file that was
    /// replaced or shortened is read again from the start.
    pub fn read_new(&mut self) -> std::io::Result<Vec<Value>> {
        let mut file = File::open(&self.path)?;
        let len = file.metadata()?.len();
        if len < self.offset {
            self.offset = 0;
            self.partial.clear();
        }
        let mut inputs = Vec::new();
        if len > self.offset {
            file.seek(SeekFrom::Start(self.offset))?;
            let mut bytes = Vec::with_capacity((len - self.offset) as usize);
            file.take(len - self.offset).read_to_end(&mut bytes)?;
            self.offset += bytes.len() as u64;
            self.partial.extend_from_slice(&bytes);
            if let Some(end) = self.partial.iter().rposition(|&b| b == b'\n') {
                let complete: Vec<u8> = self.partial.drain(..=end).collect();
                for line in String::from_utf8_lossy(&complete).lines() {
                    inputs.extend(tool_inputs(line));
                }
            }
        }
        Ok(inputs)
    }
}

/// The inputs of the tools one transcript line calls: Claude's own, not a subagent's.
fn tool_inputs(line: &str) -> Vec<Value> {
    let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
        return Vec::new();
    };
    if value["type"] != "assistant" || value["isSidechain"] == true {
        return Vec::new();
    }
    value["message"]["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "tool_use")
        .map(|block| block["input"].clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use serde_json::json;

    use super::*;

    fn line(value: Value) -> String {
        format!("{value}\n")
    }

    fn reads(path: &str) -> Value {
        json!({"type": "tool_use", "id": "t", "name": "Read", "input": {"file_path": path}})
    }

    #[test]
    fn reads_tool_inputs_as_they_are_written() {
        let path = std::env::temp_dir().join(format!("signalbox-transcript-{}.jsonl", std::process::id()));
        let mut file = File::create(&path).unwrap();
        file.write_all(line(json!({"type": "user", "message": {"content": "draw a cat"}})).as_bytes())
            .unwrap();
        let subagent = json!({"type": "assistant", "isSidechain": true, "message": {"content": [reads("/sub.png")]}});
        file.write_all(line(subagent).as_bytes()).unwrap();
        let assistant = line(json!({"type": "assistant",
            "message": {"content": [{"type": "text", "text": "Here it is"}, reads("/cat.png")]}}));
        let (head, tail) = assistant.split_at(20);
        file.write_all(head.as_bytes()).unwrap();
        file.flush().unwrap();

        let mut reader = TranscriptReader::new(path.clone());
        assert!(
            reader.read_new().unwrap().is_empty(),
            "a subagent's tools and a half-written line don't count"
        );
        file.write_all(tail.as_bytes()).unwrap();
        file.flush().unwrap();
        assert_eq!(reader.read_new().unwrap(), vec![json!({"file_path": "/cat.png"})]);
        assert!(reader.read_new().unwrap().is_empty());

        // Rewritten shorter, it's read from the start.
        std::fs::write(&path, line(json!({"type": "assistant", "message": {"content": [reads("/b.png")]}}))).unwrap();
        assert_eq!(reader.read_new().unwrap(), vec![json!({"file_path": "/b.png"})]);
        std::fs::remove_file(&path).ok();
    }
}
