use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::tui::theme::Theme;

mod wrap;
use wrap::{PushLines, cell, fit, span_width, word_width, words_from};

#[cfg(test)]
mod tests;

/// A run of text sharing one style. Words are built from sequences of these so a
/// single word may carry more than one style (e.g. `**bo**ld`).
#[derive(Clone)]
struct Piece {
    text: String,
    style: Style,
}

/// Parse `md` and render it to owned, word-wrapped [`Line`]s no wider than
/// `width` columns, themed with `theme`. `ascii` swaps unicode bullets/rules for
/// ASCII equivalents.
pub(super) fn render(md: &str, width: usize, theme: &Theme, ascii: bool) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_GFM);

    let mut r = Renderer::new(width, theme, ascii);
    for ev in Parser::new_ext(md, opts) {
        r.event(ev);
    }
    r.finish()
}

struct ListLevel {
    /// Next item number for an ordered list, or `None` for a bullet list.
    ordered: Option<u64>,
    /// Column width of this level's marker, used to indent wrapped/continuation
    /// lines so they align under the item text.
    indent: usize,
    /// The marker to print on the next item's first line, consumed when that
    /// line is flushed.
    pending: Option<String>,
}

/// One in-progress table: cells captured as plain text, header separated.
struct TableAcc {
    header: Vec<String>,
    rows: Vec<Vec<String>>,
    row: Vec<String>,
    cell: String,
    in_head: bool,
}

struct Renderer<'t> {
    width: usize,
    theme: &'t Theme,
    ascii: bool,
    out: Vec<Line<'static>>,
    /// Inline buffer for the current block, flushed at block boundaries.
    segs: Vec<Piece>,
    /// Inline style stack (emphasis/strong/code/link/heading), folded to get the
    /// current style.
    styles: Vec<Style>,
    lists: Vec<ListLevel>,
    quote: usize,
    /// Raw lines of the code block currently open, if any.
    code: Option<Vec<String>>,
    table: Option<TableAcc>,
    /// Destination URLs of open links/images, and where their visible text began
    /// in `segs`, so the URL can be appended when it differs from the text.
    links: Vec<(String, usize, bool)>,
}

impl<'t> Renderer<'t> {
    fn new(width: usize, theme: &'t Theme, ascii: bool) -> Self {
        Self {
            width,
            theme,
            ascii,
            out: Vec::new(),
            segs: Vec::new(),
            styles: Vec::new(),
            lists: Vec::new(),
            quote: 0,
            code: None,
            table: None,
            links: Vec::new(),
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        // Trim a trailing blank the block spacing may have left.
        while self
            .out
            .last()
            .is_some_and(|l| l.spans.iter().all(|s| s.content.trim().is_empty()))
        {
            self.out.pop();
        }
        self.out
    }

    /// The current inline style: the theme foreground patched by every style on
    /// the stack (emphasis, strong, code, ...).
    fn cur_style(&self) -> Style {
        let mut s = self.theme.fg_style();
        for patch in &self.styles {
            s = s.patch(*patch);
        }
        s
    }

    fn code_style(&self) -> Style {
        Style::new().fg(self.theme.fg).bg(self.theme.border)
    }

    fn push_text(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        self.segs.push(Piece {
            text: text.to_string(),
            style,
        });
    }

    /// Ensure a single blank line separates block-level elements.
    fn ensure_blank(&mut self) {
        if self.out.is_empty() {
            return;
        }
        let last_blank = self
            .out
            .last()
            .is_some_and(|l| l.spans.iter().all(|s| s.content.trim().is_empty()));
        if !last_blank {
            self.out.push(Line::from(String::new()));
        }
    }

    fn event(&mut self, ev: Event<'_>) {
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => {
                if let Some(tbl) = self.table.as_mut() {
                    tbl.cell.push_str(&t);
                } else if let Some(code) = self.code.as_mut() {
                    code.push_str_lines(&t);
                } else {
                    let style = self.cur_style();
                    self.push_text(&t, style);
                }
            }
            Event::Code(t) => {
                if let Some(tbl) = self.table.as_mut() {
                    tbl.cell.push_str(&t);
                } else {
                    let style = self.cur_style().patch(self.code_style());
                    self.push_text(&t, style);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if self.table.is_none() && self.code.is_none() {
                    let style = self.cur_style();
                    self.push_text(" ", style);
                }
            }
            Event::TaskListMarker(done) => {
                let mark = if done { "[x] " } else { "[ ] " };
                let style = self.cur_style();
                self.push_text(mark, style);
            }
            Event::FootnoteReference(label) => {
                self.push_text(&format!("[^{label}]"), self.theme.dim_style());
            }
            Event::Rule => {
                self.ensure_blank();
                let ch = if self.ascii { "-" } else { "─" };
                let rule = ch.repeat(self.width);
                self.out
                    .push(Line::from(Span::styled(rule, self.theme.dim_style())));
                self.out.push(Line::from(String::new()));
            }
            // Raw HTML rarely survives the HTML->Markdown conversion; show any
            // stray inline HTML as dim text rather than dropping it silently.
            Event::Html(t) | Event::InlineHtml(t) => {
                if self.table.is_none() && self.code.is_none() {
                    self.push_text(t.trim_end_matches('\n'), self.theme.dim_style());
                }
            }
            Event::InlineMath(t) | Event::DisplayMath(t) => {
                let style = self.cur_style().patch(self.code_style());
                self.push_text(&t, style);
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                if self.lists.is_empty() {
                    self.ensure_blank();
                }
            }
            Tag::Heading { level, .. } => {
                self.ensure_blank();
                let bump = matches!(level, HeadingLevel::H1 | HeadingLevel::H2);
                let mut s = self.theme.accent_style().add_modifier(Modifier::BOLD);
                if bump {
                    s = s.add_modifier(Modifier::UNDERLINED);
                }
                self.styles.push(s);
            }
            Tag::BlockQuote(_) => {
                self.ensure_blank();
                self.quote += 1;
            }
            Tag::CodeBlock(_) => {
                self.ensure_blank();
                self.code = Some(Vec::new());
            }
            Tag::List(first) => {
                let indent = match first {
                    Some(n) => n.to_string().len() + 2,
                    None => 2,
                };
                self.lists.push(ListLevel {
                    ordered: first,
                    indent,
                    pending: None,
                });
            }
            Tag::Item => {
                let marker = match self.lists.last_mut() {
                    Some(l) => match l.ordered {
                        Some(n) => {
                            l.ordered = Some(n + 1);
                            let m = format!("{n}. ");
                            l.indent = m.width();
                            m
                        }
                        None => {
                            if self.ascii {
                                "- ".to_string()
                            } else {
                                "• ".to_string()
                            }
                        }
                    },
                    None => "• ".to_string(),
                };
                if let Some(l) = self.lists.last_mut() {
                    l.pending = Some(marker);
                }
            }
            Tag::Emphasis => self
                .styles
                .push(Style::new().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.styles.push(Style::new().add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => self
                .styles
                .push(Style::new().add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { dest_url, .. } => {
                self.links
                    .push((dest_url.to_string(), self.segs.len(), false));
                self.styles
                    .push(self.theme.accent_style().add_modifier(Modifier::UNDERLINED));
            }
            Tag::Image { dest_url, .. } => {
                self.links
                    .push((dest_url.to_string(), self.segs.len(), true));
                self.push_text("[image: ", self.theme.dim_style());
                self.styles.push(self.theme.dim_style());
            }
            Tag::Table(_) => {
                self.ensure_blank();
                self.table = Some(TableAcc {
                    header: Vec::new(),
                    rows: Vec::new(),
                    row: Vec::new(),
                    cell: String::new(),
                    in_head: false,
                });
            }
            Tag::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.in_head = true;
                }
            }
            Tag::TableCell => {
                if let Some(t) = self.table.as_mut() {
                    t.cell.clear();
                }
            }
            // Rows are delimited by cell start/end; nothing to do on row start.
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.flush_block(),
            TagEnd::Heading(_) => {
                self.flush_block();
                self.styles.pop();
            }
            TagEnd::BlockQuote(_) => {
                self.quote = self.quote.saturating_sub(1);
            }
            TagEnd::CodeBlock => {
                if let Some(lines) = self.code.take() {
                    let style = self.theme.dim_style();
                    let prefix = self.block_prefix();
                    for raw in lines {
                        let mut spans = prefix.clone();
                        spans.push(Span::styled(format!("  {raw}"), style));
                        self.out.push(Line::from(spans));
                    }
                }
            }
            TagEnd::List(_) => {
                self.lists.pop();
                if self.lists.is_empty() {
                    self.out.push(Line::from(String::new()));
                }
            }
            TagEnd::Item => {
                // Tight lists put text directly under the item with no
                // Paragraph; flush whatever inline buffer remains.
                if !self.segs.is_empty() {
                    self.flush_block();
                }
                // A pending marker means the item was empty; emit it alone.
                if let Some(l) = self.lists.last_mut()
                    && l.pending.take().is_some()
                {
                    // nothing to render for a truly empty item
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.styles.pop();
            }
            TagEnd::Link => {
                self.styles.pop();
                self.append_link_url();
            }
            TagEnd::Image => {
                self.styles.pop();
                self.push_text("]", self.theme.dim_style());
                self.append_link_url();
            }
            TagEnd::Table => self.flush_table(),
            TagEnd::TableCell => {
                if let Some(t) = self.table.as_mut() {
                    let cell = t.cell.trim().to_string();
                    t.row.push(cell);
                }
            }
            TagEnd::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.header = std::mem::take(&mut t.row);
                    t.in_head = false;
                }
            }
            TagEnd::TableRow => {
                if let Some(t) = self.table.as_mut()
                    && !t.in_head
                {
                    let row = std::mem::take(&mut t.row);
                    t.rows.push(row);
                }
            }
            _ => {}
        }
    }

    /// Append ` (url)` (dim) after a link/image when the URL adds information
    /// beyond the visible text.
    fn append_link_url(&mut self) {
        let Some((url, text_start, is_image)) = self.links.pop() else {
            return;
        };
        if url.is_empty() {
            return;
        }
        let shown: String = self.segs[text_start..]
            .iter()
            .map(|p| p.text.as_str())
            .collect();
        let shown = shown.trim();
        let bare = url.strip_prefix("mailto:").unwrap_or(&url);
        // Skip the suffix when the text already is the URL (autolinks) or, for
        // images, always show it since the alt text rarely is the URL.
        if !is_image && (shown == url || shown == bare) {
            return;
        }
        self.push_text(&format!(" ({url})"), self.theme.dim_style());
    }

    /// The quote/list indentation spans shared by every line of the current
    /// block. The innermost list marker is not included here (see `flush_block`).
    fn block_prefix(&self) -> Vec<Span<'static>> {
        let mut spans: Vec<Span<'static>> = Vec::new();
        if self.quote > 0 {
            let bar = if self.ascii { "> " } else { "│ " };
            spans.push(Span::styled(bar.repeat(self.quote), self.theme.dim_style()));
        }
        let indent: usize = self.lists.iter().map(|l| l.indent).sum();
        if indent > 0 {
            spans.push(Span::raw(" ".repeat(indent)));
        }
        spans
    }

    /// Flush the accumulated inline buffer as word-wrapped, prefixed lines.
    fn flush_block(&mut self) {
        let segs = std::mem::take(&mut self.segs);
        // The innermost list level may owe a marker on this block's first line.
        let marker = self.lists.last_mut().and_then(|l| l.pending.take());

        let quote_spans: Vec<Span<'static>> = if self.quote > 0 {
            let bar = if self.ascii { "> " } else { "│ " };
            vec![Span::styled(bar.repeat(self.quote), self.theme.dim_style())]
        } else {
            Vec::new()
        };
        // Indent contributed by the outer list levels (all but the innermost).
        let outer_indent: usize = self
            .lists
            .iter()
            .rev()
            .skip(1)
            .map(|l| l.indent)
            .sum::<usize>();
        let inner_indent = self.lists.last().map(|l| l.indent).unwrap_or(0);

        // First line: outer indent, then the marker (or spaces if none pending).
        let mut first_prefix = quote_spans.clone();
        if outer_indent > 0 {
            first_prefix.push(Span::raw(" ".repeat(outer_indent)));
        }
        match &marker {
            Some(m) => first_prefix.push(Span::styled(m.clone(), self.theme.dim_style())),
            None if inner_indent > 0 => first_prefix.push(Span::raw(" ".repeat(inner_indent))),
            None => {}
        }
        // Continuation lines: full indent, no marker.
        let mut cont_prefix = quote_spans;
        let total_indent = outer_indent + inner_indent;
        if total_indent > 0 {
            cont_prefix.push(Span::raw(" ".repeat(total_indent)));
        }

        if segs.is_empty() {
            return;
        }

        let prefix_w = span_width(&first_prefix);
        let avail = self.width.saturating_sub(prefix_w).max(1);
        let words = words_from(&segs);

        let mut cur = first_prefix;
        let mut cur_w = 0usize;
        let mut fresh = true; // no words on the current line yet
        let mut emitted = false;

        for word in words {
            let ww = word_width(&word);
            if ww > avail {
                // Hard-break an overlong word (e.g. a long URL) across lines.
                for piece in word {
                    for ch in piece.text.chars() {
                        let cw = ch.width().unwrap_or(0);
                        if cur_w + cw > avail && !fresh {
                            self.out.push(Line::from(std::mem::take(&mut cur)));
                            cur = cont_prefix.clone();
                            cur_w = 0;
                            emitted = true;
                        }
                        cur.push(Span::styled(ch.to_string(), piece.style));
                        cur_w += cw;
                        fresh = false;
                    }
                }
                continue;
            }
            let extra = if fresh { ww } else { ww + 1 };
            if cur_w + extra > avail && !fresh {
                self.out.push(Line::from(std::mem::take(&mut cur)));
                cur = cont_prefix.clone();
                cur_w = 0;
                fresh = true;
                emitted = true;
            }
            if !fresh {
                cur.push(Span::raw(" "));
                cur_w += 1;
            }
            for piece in word {
                cur_w += piece.text.width();
                cur.push(Span::styled(piece.text, piece.style));
            }
            fresh = false;
        }
        if !fresh || !emitted {
            self.out.push(Line::from(cur));
        }
    }

    fn flush_table(&mut self) {
        let Some(t) = self.table.take() else {
            return;
        };
        let cols = t
            .header
            .len()
            .max(t.rows.iter().map(|r| r.len()).max().unwrap_or(0));
        if cols == 0 {
            return;
        }

        // Natural column widths, then shrink to fit the pane width.
        let mut widths = vec![0usize; cols];
        for (i, wref) in widths.iter_mut().enumerate() {
            let mut w = cell(&t.header, i).width();
            for r in &t.rows {
                w = w.max(cell(r, i).width());
            }
            *wref = w.max(1);
        }
        // " │ " separators between columns plus a 1-col pad on each side.
        let sep_w = 3 * cols.saturating_sub(1) + 2;
        let budget = self.width.saturating_sub(sep_w).max(cols);
        while widths.iter().sum::<usize>() > budget {
            let Some((i, _)) = widths.iter().enumerate().max_by_key(|(_, w)| **w) else {
                break;
            };
            if widths[i] <= 1 {
                break;
            }
            widths[i] -= 1;
        }

        let bar = if self.ascii { "|" } else { "│" };
        let dash = if self.ascii { "-" } else { "─" };
        let sep_style = self.theme.dim_style();

        let render_row = |widths: &[usize], row: &[String], header: bool| -> Line<'static> {
            let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
            for (i, w) in widths.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::styled(format!(" {bar} "), sep_style));
                }
                let text = fit(cell(row, i), *w);
                let style = if header {
                    Style::new().add_modifier(Modifier::BOLD)
                } else {
                    Style::new()
                };
                spans.push(Span::styled(text, style));
            }
            spans.push(Span::raw(" "));
            Line::from(spans)
        };

        if !t.header.is_empty() {
            self.out.push(render_row(&widths, &t.header, true));
            // Header underline.
            let mut sep: Vec<Span<'static>> = vec![Span::styled(dash.to_string(), sep_style)];
            for (i, w) in widths.iter().enumerate() {
                if i > 0 {
                    sep.push(Span::styled(format!("{dash}{dash}{dash}"), sep_style));
                }
                sep.push(Span::styled(dash.repeat(*w), sep_style));
            }
            sep.push(Span::styled(dash.to_string(), sep_style));
            self.out.push(Line::from(sep));
        }
        for r in &t.rows {
            self.out.push(render_row(&widths, r, false));
        }
        self.out.push(Line::from(String::new()));
    }
}
