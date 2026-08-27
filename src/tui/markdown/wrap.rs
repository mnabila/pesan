use super::*;

/// The `i`th cell of a table row, or `""` when the row is short.
pub(super) fn cell(row: &[String], i: usize) -> &str {
    row.get(i).map(String::as_str).unwrap_or("")
}

/// Pad or truncate `s` to exactly `width` display columns.
pub(super) fn fit(s: &str, width: usize) -> String {
    let w = s.width();
    if w == width {
        s.to_string()
    } else if w < width {
        format!("{s}{}", " ".repeat(width - w))
    } else {
        let mut out = String::new();
        let mut used = 0usize;
        let cap = width.saturating_sub(1);
        for ch in s.chars() {
            let cw = ch.width().unwrap_or(0);
            if used + cw > cap {
                break;
            }
            out.push(ch);
            used += cw;
        }
        out.push('…');
        out
    }
}

/// Split a run of styled pieces into words (space-delimited), preserving each
/// word's per-piece styling so a word may span multiple styles.
pub(super) fn words_from(segs: &[Piece]) -> Vec<Vec<Piece>> {
    let mut words: Vec<Vec<Piece>> = Vec::new();
    let mut cur: Vec<Piece> = Vec::new();
    for seg in segs {
        let mut first = true;
        for part in seg.text.split(' ') {
            if !first && !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            first = false;
            if !part.is_empty() {
                cur.push(Piece {
                    text: part.to_string(),
                    style: seg.style,
                });
            }
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

pub(super) fn word_width(word: &[Piece]) -> usize {
    word.iter().map(|p| p.text.width()).sum()
}

pub(super) fn span_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

/// Split incoming code-block text (which may contain several lines) into the
/// running line vector, so each source line becomes its own rendered row.
pub(super) trait PushLines {
    fn push_str_lines(&mut self, text: &str);
}

impl PushLines for Vec<String> {
    fn push_str_lines(&mut self, text: &str) {
        let mut parts = text.split('\n');
        if let Some(first) = parts.next() {
            if let Some(last) = self.last_mut() {
                last.push_str(first);
            } else {
                self.push(first.to_string());
            }
        }
        for p in parts {
            self.push(p.to_string());
        }
    }
}
