//! What each Claude session's status line last reported, from `~/.signalbox/status/<session id>.json`
//! (written by a status line like `contrib/statusline.py` on every update): context use, limits,
//! lines changed.

use std::path::PathBuf;

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limit {
    pub used_pct: u32,
    /// Unix seconds.
    pub resets_at: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionStatus {
    pub context_pct: u32,
    pub context_size: u64,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub duration_ms: u64,
    pub five_hour: Option<Limit>,
    pub seven_day: Option<Limit>,
    /// Unix seconds.
    pub updated: u64,
}

pub fn dir() -> PathBuf {
    crate::sys::data_dir().join("status")
}

pub fn read(session_id: &str) -> Option<SessionStatus> {
    let text = std::fs::read_to_string(dir().join(format!("{session_id}.json"))).ok()?;
    parse(&serde_json::from_str(&text).ok()?)
}

fn number(value: &Value) -> Option<f64> {
    value.as_f64()
}

fn limit(value: &Value) -> Option<Limit> {
    Some(Limit {
        used_pct: number(&value["used_percentage"])?.round().max(0.) as u32,
        resets_at: value["resets_at"].as_u64(),
    })
}

pub fn parse(value: &Value) -> Option<SessionStatus> {
    let context = &value["context"];
    let cost = &value["cost"];
    Some(SessionStatus {
        context_pct: context["used_pct"].as_u64().unwrap_or(0) as u32,
        context_size: context["size"].as_u64().unwrap_or(0),
        lines_added: cost["lines_added"].as_u64().unwrap_or(0),
        lines_removed: cost["lines_removed"].as_u64().unwrap_or(0),
        duration_ms: cost["duration_ms"].as_u64().unwrap_or(0),
        five_hour: limit(&value["limits"]["five_hour"]),
        seven_day: limit(&value["limits"]["seven_day"]),
        updated: value["updated"].as_u64()?,
    })
}

/// "4h43m", "5d1h", "12m".
pub fn until(seconds: u64) -> String {
    let (days, hours, minutes) = (seconds / 86_400, (seconds % 86_400) / 3_600, (seconds % 3_600) / 60);
    match (days, hours) {
        (0, 0) => format!("{minutes}m"),
        (0, _) => format!("{hours}h{minutes:02}m"),
        _ => format!("{days}d{hours}h"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_what_the_status_line_writes() {
        let status = parse(&json!({
            "session_id": "s", "model": "Opus 5.5",
            "context": {"used_pct": 44, "size": 1000000},
            "cost": {"duration_ms": 600000, "lines_added": 202, "lines_removed": 41},
            "limits": {"five_hour": {"used_percentage": 7.4, "resets_at": 100}, "seven_day": null},
            "updated": 50
        }))
        .unwrap();
        assert_eq!(status.context_pct, 44);
        assert_eq!(
            status.five_hour,
            Some(Limit {
                used_pct: 7,
                resets_at: Some(100)
            })
        );
        assert_eq!(status.seven_day, None);
        assert_eq!(status.lines_added, 202);
    }

    #[test]
    fn short_numbers() {
        assert_eq!(until(17_000), "4h43m");
        assert_eq!(until(450_000), "5d5h");
        assert_eq!(until(720), "12m");
    }
}
