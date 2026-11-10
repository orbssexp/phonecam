/// Access units waiting for the decoder beyond which the backlog is dropped (about 100 ms of 30 fps).
const MAX_BACKLOG: usize = 3;

/// Per-window (10 s) pipeline measurements for the `[media]` line.
#[derive(Default)]
struct StageWindow {
    frames: u64,
    queue_ms: f64,
    queue_max_ms: f64,
    decode_ms: f64,
    download_ms: f64,
    convert_ms: f64,
    rotate_ms: f64,
    scale_ms: f64,
    total_ms: f64,
    skipped: u64,
}

/// Drops the access units that would only play old video. With a keyframe in the queue everything
/// before the newest one goes (decoding resumes there); with none, every access unit goes and
/// `await_key` is set so deltas are skipped until a keyframe arrives. Other messages are kept in
/// order. Returns how many access units were dropped.
fn trim_backlog(q: &mut VecDeque<MediaMsg>, await_key: &mut bool) -> usize {
    let last_key = q.iter().rposition(|m| matches!(m, MediaMsg::Au(a) if a.keyframe));
    let mut dropped = 0;
    let kept: VecDeque<MediaMsg> = q
        .drain(..)
        .enumerate()
        .filter(|(i, m)| {
            let stale = matches!(m, MediaMsg::Au(_)) && last_key.is_none_or(|k| *i < k);
            if stale {
                dropped += 1;
            }
            !stale
        })
        .map(|(_, m)| m)
        .collect();
    *q = kept;
    if last_key.is_none() {
        if dropped > 0 {
            *await_key = true;
        }
    } else {
        *await_key = false;
    }
    dropped
}

