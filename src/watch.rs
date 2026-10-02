//! Follows a Claude Code session running in a terminal: reads its transcript as it grows, and
//! gathers the images and videos it made or looked at, for the panel beside the terminal.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{Context, Task};

use crate::live::LiveSession;
use crate::media::{self, MediaItem};
use crate::transcript::{TranscriptReader, transcript_path};

const POLL: Duration = Duration::from_secs(1);
const SCAN_EVERY: Duration = Duration::from_secs(10);
/// Thumbnails made per pass, so a folder of new frames doesn't hold up the rest.
const THUMBS_PER_PASS: usize = 8;

pub struct Watch {
    pub session: LiveSession,
    /// Newest first, with thumbnails as they're made.
    pub media: Vec<MediaItem>,
    /// Media files its tools wrote, edited or read.
    touched: Vec<PathBuf>,
    _poll: Task<()>,
    _scan: Task<()>,
}

impl Watch {
    pub fn new(session: LiveSession, cx: &mut Context<Self>) -> Self {
        let (cwd, id) = (session.cwd.clone(), session.session_id.clone());
        let poll = cx.spawn(async move |this, cx| {
            // Claude Code writes the transcript with the first message, so keep looking until then.
            let path = loop {
                let (cwd, id) = (cwd.clone(), id.clone());
                let found = cx
                    .background_executor()
                    .spawn(async move { transcript_path(&cwd, &id) })
                    .await;
                if let Some(path) = found {
                    break path;
                }
                if this.update(cx, |_, _| {}).is_err() {
                    return;
                }
                cx.background_executor().timer(POLL * 2).await;
            };
            let mut reader = TranscriptReader::new(path);
            loop {
                let (back, inputs) = cx
                    .background_executor()
                    .spawn(async move {
                        let inputs = reader.read_new();
                        (reader, inputs)
                    })
                    .await;
                reader = back;
                let alive = this.update(cx, |watch, _| {
                    if let Ok(inputs) = inputs {
                        watch.touched(&inputs);
                    }
                });
                if alive.is_err() {
                    break;
                }
                cx.background_executor().timer(POLL).await;
            }
        });
        let scan = cx.spawn(async move |this, cx| {
            loop {
                let Ok((cwd, since, touched, previous)) = this.update(cx, |watch, _| {
                    let since = watch
                        .session
                        .started_at
                        .map(|ms| UNIX_EPOCH + Duration::from_millis(ms))
                        .unwrap_or_else(|| SystemTime::now() - Duration::from_secs(6 * 3600));
                    (
                        watch.session.cwd.clone(),
                        since,
                        watch.touched.clone(),
                        watch.media.clone(),
                    )
                }) else {
                    break;
                };
                let media = cx
                    .background_executor()
                    .spawn(async move {
                        let scanned = media::scan(&cwd, since);
                        let mut items = media::merge(&touched, &scanned, &previous);
                        for item in items
                            .iter_mut()
                            .filter(|item| item.thumb.is_none())
                            .take(THUMBS_PER_PASS)
                        {
                            item.thumb = media::thumbnail(&item.path);
                        }
                        items
                    })
                    .await;
                let more = media.iter().any(|item| item.thumb.is_none());
                let alive = this.update(cx, |watch, cx| {
                    if watch.media != media {
                        watch.media = media;
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
                // Come back sooner while thumbnails are still being made.
                let wait = if more { Duration::from_secs(1) } else { SCAN_EVERY };
                cx.background_executor().timer(wait).await;
            }
        });
        Self {
            session,
            media: Vec::new(),
            touched: Vec::new(),
            _poll: poll,
            _scan: scan,
        }
    }

    /// Notes the media files that tools were called with; the next scan shows them.
    fn touched(&mut self, inputs: &[serde_json::Value]) {
        for path in inputs.iter().filter_map(media::paths_in) {
            if !self.touched.contains(&path) {
                self.touched.push(path);
            }
        }
    }

    /// The latest state of the same session, from the session files.
    pub fn update_session(&mut self, session: LiveSession, cx: &mut Context<Self>) {
        if self.session != session {
            self.session = session;
            cx.notify();
        }
    }
}
