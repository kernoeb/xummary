//! One scrollable pane. The briefing streams in; you read it and close it.

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Margin};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};
use std::time::Duration;
use tokio::sync::mpsc;
use unicode_width::UnicodeWidthStr;

/// What the fetch-and-summarize task reports to the screen.
#[derive(Debug)]
pub enum Update {
    Status(String),
    Token(String),
    Done(Option<String>),
}

struct App {
    status: String,
    text: String,
    scroll: u16,
    /// Stay pinned to the bottom while tokens arrive, until the reader scrolls up.
    follow: bool,
    done: bool,
}

pub async fn run(mut updates: mpsc::UnboundedReceiver<Update>) -> Result<()> {
    let mut app = App {
        status: "starting".into(),
        text: String::new(),
        scroll: 0,
        follow: true,
        done: false,
    };

    // Raw mode first, so the reader thread does not swallow the first keystroke
    // into the shell's line buffer.
    let mut terminal = ratatui::init();
    let mut keys = spawn_input_thread();
    let mut updates_closed = false;

    let result = loop {
        if let Err(e) = terminal.draw(|f| draw(f, &mut app)) {
            break Err(e.into());
        }

        tokio::select! {
            biased;
            key = keys.recv() => {
                match key {
                    Some(k) if quits(k) => break Ok(()),
                    Some(k) => scroll(&mut app, k),
                    // The input thread died; without a keyboard there is no
                    // way out of the alternate screen.
                    None => break Ok(()),
                }
            }
            update = updates.recv(), if !updates_closed => {
                match update {
                    Some(u) => apply(&mut app, u),
                    None => updates_closed = true,
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(150)) => {}
        }
    };

    ratatui::restore();
    result
}

fn apply(app: &mut App, update: Update) {
    match update {
        Update::Status(s) => app.status = s,
        Update::Token(t) => app.text.push_str(&t),
        Update::Done(err) => {
            app.done = true;
            app.status = match err {
                Some(e) => format!("failed: {e}"),
                None => "done".into(),
            };
        }
    }
}

fn quits(k: KeyEvent) -> bool {
    matches!(k.code, KeyCode::Char('q') | KeyCode::Esc)
        || (k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL))
}

fn scroll(app: &mut App, k: KeyEvent) {
    match k.code {
        KeyCode::Char('j') | KeyCode::Down => app.scroll = app.scroll.saturating_add(1),
        KeyCode::Char('k') | KeyCode::Up => {
            app.scroll = app.scroll.saturating_sub(1);
            app.follow = false;
        }
        KeyCode::Char(' ') | KeyCode::PageDown => app.scroll = app.scroll.saturating_add(10),
        KeyCode::PageUp => {
            app.scroll = app.scroll.saturating_sub(10);
            app.follow = false;
        }
        KeyCode::Char('g') | KeyCode::Home => {
            app.scroll = 0;
            app.follow = false;
        }
        KeyCode::Char('G') | KeyCode::End => app.follow = true,
        _ => {}
    }
}

/// Keyboard reads block, so they get their own thread and the draw loop stays async.
fn spawn_input_thread() -> mpsc::UnboundedReceiver<KeyEvent> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || loop {
        match event::read() {
            Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => {
                if tx.send(k).is_err() {
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    });
    rx
}

fn draw(f: &mut ratatui::Frame, app: &mut App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(f.area());

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " digest ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                " {}{}",
                app.status,
                if app.done { "" } else { "…" }
            )),
        ])),
        header,
    );

    let inner = body.inner(Margin::new(1, 0));
    let lines = layout_text(&app.text, inner.width as usize);
    let max_scroll = (lines.len() as u16).saturating_sub(inner.height);
    app.scroll = if app.follow {
        max_scroll
    } else {
        app.scroll.min(max_scroll)
    };

    f.render_widget(Paragraph::new(lines).scroll((app.scroll, 0)), inner);

    if max_scroll > 0 {
        let mut state = ScrollbarState::new(max_scroll as usize).position(app.scroll as usize);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            body,
            &mut state,
        );
    }

    f.render_widget(
        Paragraph::new(Span::styled(
            " q quit · j/k scroll · g top · G follow",
            Style::default().fg(Color::DarkGray),
        )),
        footer,
    );
}

/// Wraps the briefing to `width` and styles headings and bullets. Wrapping here
/// rather than letting `Paragraph` do it is what lets scrolling know the real
/// height.
fn layout_text(text: &str, width: usize) -> Vec<Line<'static>> {
    let width = width.max(10);
    // Three states, told apart by weight and colour rather than a badge glyph:
    // a story you have never seen, one that moved since you read it, and one
    // carried over unchanged.
    let fresh = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let moved = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let carried = Style::default().fg(Color::Cyan);
    let mut out: Vec<Line<'static>> = Vec::new();

    for raw in text.split('\n') {
        let line = raw.replace("**", "");
        let line = line.trim_end();
        if line.is_empty() {
            out.push(Line::raw(""));
            continue;
        }

        let (content, style, indent) = if let Some(h) = line.strip_prefix("## ") {
            if !out.is_empty() {
                out.push(Line::raw(""));
            }
            let (title, mark) = split_mark(h);
            (title, style_for(mark, fresh, moved, carried), 0)
        } else if let Some(h) = line.strip_prefix("# ") {
            let (title, mark) = split_mark(h);
            (title, style_for(mark, fresh, moved, carried), 0)
        } else if let Some(b) = line.strip_prefix("- ") {
            (format!("• {b}"), Style::default(), 2)
        } else {
            (line.to_string(), Style::default(), 0)
        };

        for (i, piece) in wrap(&content, width).into_iter().enumerate() {
            let text = if i == 0 {
                piece
            } else {
                format!("{}{piece}", " ".repeat(indent))
            };
            out.push(Line::from(Span::styled(text, style)));
        }
    }
    out
}

/// What a heading says about how much of its story you have already read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mark {
    /// Already in the briefing you read, unchanged since.
    Carried,
    /// Not in the briefing you read.
    New,
    /// In the briefing you read, but the story has moved.
    Updated,
}

/// Splits the mark off a heading. The model writes it in English whatever the
/// briefing's language, so this is a plain suffix match.
fn split_mark(heading: &str) -> (String, Mark) {
    let heading = heading.trim_end();
    for (suffix, mark) in [
        (crate::llm::NEW_MARK.trim(), Mark::New),
        (crate::llm::UPDATED_MARK.trim(), Mark::Updated),
    ] {
        if let Some(title) = heading.strip_suffix(suffix) {
            return (title.trim_end().to_string(), mark);
        }
    }
    (heading.to_string(), Mark::Carried)
}

fn style_for(mark: Mark, fresh: Style, moved: Style, carried: Style) -> Style {
    match mark {
        Mark::New => fresh,
        Mark::Updated => moved,
        Mark::Carried => carried,
    }
}

/// Greedy word wrap on display width. A word wider than the pane is cut.
fn wrap(s: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for word in s.split_whitespace() {
        let w = word.width();
        if current_width > 0 && current_width + 1 + w > width {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }
        if w > width {
            for ch in word.chars() {
                let cw = ch.to_string().width();
                if current_width + cw > width {
                    lines.push(std::mem::take(&mut current));
                    current_width = 0;
                }
                current.push(ch);
                current_width += cw;
            }
            continue;
        }
        if current_width > 0 {
            current.push(' ');
            current_width += 1;
        }
        current.push_str(word);
        current_width += w;
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_breaks_on_words() {
        assert_eq!(wrap("one two three", 7), vec!["one two", "three"]);
    }

    #[test]
    fn wrap_cuts_a_word_wider_than_the_pane() {
        assert_eq!(wrap("aaaaaa", 3), vec!["aaa", "aaa"]);
    }

    #[test]
    fn wrap_counts_display_width_not_bytes() {
        assert_eq!(wrap("日本語 ok", 6), vec!["日本語", "ok"]);
    }

    #[test]
    fn a_marked_heading_loses_its_mark() {
        assert_eq!(split_mark("Astra [new]"), ("Astra".to_string(), Mark::New));
        assert_eq!(
            split_mark("Astra [updated]"),
            ("Astra".to_string(), Mark::Updated)
        );
        assert_eq!(split_mark("Astra"), ("Astra".to_string(), Mark::Carried));
    }

    #[test]
    fn a_heading_gets_a_blank_line_before_it() {
        let lines = layout_text("text\n## Topic", 40);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].to_string(), "");
        assert_eq!(lines[2].to_string(), "Topic");
    }

    #[test]
    fn a_leading_heading_gets_no_blank_line() {
        let lines = layout_text("## Topic", 40);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn bullets_become_dots() {
        assert_eq!(layout_text("- a point", 40)[0].to_string(), "• a point");
    }

    #[test]
    fn stray_bold_markers_are_dropped() {
        assert_eq!(layout_text("**loud**", 40)[0].to_string(), "loud");
    }
}
