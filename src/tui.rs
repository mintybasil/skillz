//! TUI for skill import selection using ratatui.

use std::fs;
use std::io;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Terminal;

use crate::commands::FoundSkill;

/// Which pane is currently focused.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    List,
    Preview,
}

impl Focus {
    fn toggle(self) -> Self {
        match self {
            Focus::List => Focus::Preview,
            Focus::Preview => Focus::List,
        }
    }
}

/// A rendered markdown line with styled spans.
fn render_markdown(content: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut in_code_block = false;

    for raw_line in content.lines() {
        // Code block fence
        if raw_line.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
            lines.push(Line::from(vec![Span::styled(
                raw_line.to_string(),
                Style::default().fg(Color::DarkGray),
            )]));
            continue;
        }

        if in_code_block {
            // Inside code block: monochrome, no parsing
            lines.push(Line::from(vec![Span::styled(
                raw_line.to_string(),
                Style::default().fg(Color::Gray),
            )]));
            continue;
        }

        // Empty line
        if raw_line.trim().is_empty() {
            lines.push(Line::raw(""));
            continue;
        }

        // Headings
        if let Some(rest) = raw_line.trim_start().strip_prefix("# ") {
            lines.push(Line::from(vec![Span::styled(
                rest.to_string(),
                Style::default()
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                    .fg(Color::Cyan),
            )]));
            continue;
        }
        if let Some(rest) = raw_line.trim_start().strip_prefix("## ") {
            lines.push(Line::from(vec![Span::styled(
                rest.to_string(),
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(Color::Yellow),
            )]));
            continue;
        }
        if let Some(rest) = raw_line.trim_start().strip_prefix("### ") {
            lines.push(Line::from(vec![Span::styled(
                rest.to_string(),
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(Color::Magenta),
            )]));
            continue;
        }
        if let Some(rest) = raw_line.trim_start().strip_prefix("#### ") {
            lines.push(Line::from(vec![Span::styled(
                rest.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            )]));
            continue;
        }

        // Horizontal rule
        if raw_line.trim() == "---" || raw_line.trim() == "***" || raw_line.trim() == "___" {
            lines.push(Line::from(vec![Span::styled(
                "─".repeat(60),
                Style::default().fg(Color::DarkGray),
            )]));
            continue;
        }

        // List items (bullet or numbered)
        let trimmed = raw_line.trim_start();
        let indent = raw_line.len() - trimmed.len();
        let indent_str = " ".repeat(indent);

        if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            let rest = &trimmed[2..];
            let spans = parse_inline(rest);
            let mut line_spans = vec![Span::raw(format!("{indent_str}• "))];
            line_spans.extend(spans);
            lines.push(Line::from(line_spans));
            continue;
        }

        // Numbered list
        if let Some(pos) = trimmed.find(". ") {
            if pos > 0 && trimmed[..pos].chars().all(|c| c.is_ascii_digit()) {
                let rest = &trimmed[pos + 2..];
                let number = &trimmed[..pos + 1];
                let spans = parse_inline(rest);
                let mut line_spans = vec![Span::raw(format!("{indent_str}{number}. "))];
                line_spans.extend(spans);
                lines.push(Line::from(line_spans));
                continue;
            }
        }

        // Blockquote
        if let Some(rest) = trimmed.strip_prefix("> ") {
            let spans = parse_inline(rest);
            let mut line_spans = vec![Span::styled(
                "│ ".to_string(),
                Style::default().fg(Color::Blue),
            )];
            line_spans.extend(spans);
            lines.push(Line::from(line_spans));
            continue;
        }
        if trimmed == ">" {
            lines.push(Line::from(vec![Span::styled(
                "│".to_string(),
                Style::default().fg(Color::Blue),
            )]));
            continue;
        }

        // Regular text — parse inline formatting
        let spans = parse_inline(trimmed);
        let mut line_spans = vec![Span::raw(indent_str)];
        line_spans.extend(spans);
        lines.push(Line::from(line_spans));
    }

    lines
}

/// Parse inline markdown formatting: **bold**, *italic*, `code`, [link](url).
fn parse_inline(text: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        // Bold: **text** or __text__
        if let Some(end) = find_marker(remaining, "**") {
            let before = &remaining[..end];
            if !before.is_empty() {
                spans.push(Span::raw(before.to_string()));
            }
            let after_marker = &remaining[end + 2..];
            if let Some(close) = find_marker(after_marker, "**") {
                let content = &after_marker[..close];
                spans.push(Span::styled(
                    content.to_string(),
                    Style::default().add_modifier(Modifier::BOLD),
                ));
                remaining = &after_marker[close + 2..];
                continue;
            }
            // No closing — treat literally
            spans.push(Span::raw(format!("**{}", &after_marker)));
            break;
        }

        // Code: `text`
        if remaining.starts_with('`') {
            let after = &remaining[1..];
            if let Some(close) = after.find('`') {
                let content = &after[..close];
                spans.push(Span::styled(
                    content.to_string(),
                    Style::default().fg(Color::Green),
                ));
                remaining = &after[close + 1..];
                continue;
            }
        }

        // Link: [text](url)
        if remaining.starts_with('[') {
            if let Some(close_bracket) = remaining.find(']') {
                let link_text = &remaining[1..close_bracket];
                let after = &remaining[close_bracket + 1..];
                if after.starts_with('(') {
                    if let Some(close_paren) = after.find(')') {
                        spans.push(Span::styled(
                            link_text.to_string(),
                            Style::default()
                                .fg(Color::Blue)
                                .add_modifier(Modifier::UNDERLINED),
                        ));
                        remaining = &after[close_paren + 1..];
                        continue;
                    }
                }
            }
        }

        // Italic: *text* or _text_
        if (remaining.starts_with('*') || remaining.starts_with('_'))
            && remaining.len() > 1
            && !remaining.starts_with("**")
        {
            let marker = &remaining[..1];
            let after = &remaining[1..];
            if let Some(close) = after.find(marker) {
                let content = &after[..close];
                spans.push(Span::styled(
                    content.to_string(),
                    Style::default().add_modifier(Modifier::ITALIC),
                ));
                remaining = &after[close + 1..];
                continue;
            }
        }

        // No more markers — push rest as plain text
        // Find the next potential marker start
        let next_marker = remaining
            .find("**")
            .or_else(|| remaining.find('`'))
            .or_else(|| remaining.find('['))
            .or_else(|| remaining.find('*'))
            .or_else(|| remaining.find('_'))
            .or_else(|| remaining.find("__"));

        match next_marker {
            Some(0) => {
                // We're at a marker but didn't match any pattern above — consume one char and continue
                spans.push(Span::raw(remaining[..1].to_string()));
                remaining = &remaining[1..];
            }
            Some(pos) => {
                spans.push(Span::raw(remaining[..pos].to_string()));
                remaining = &remaining[pos..];
            }
            None => {
                spans.push(Span::raw(remaining.to_string()));
                break;
            }
        }
    }

    spans
}

/// Find a marker that's not at position 0 (i.e., the closing one).
fn find_marker(text: &str, marker: &str) -> Option<usize> {
    text.find(marker)
        .filter(|&pos| pos > 0 || marker.len() > 1)
        .map(|pos| if pos == 0 { 0 } else { pos })
}

/// Run the TUI skill selection. Returns the selected skills.
pub fn run_toggle_select(skills: &[FoundSkill]) -> Result<Vec<FoundSkill>> {
    // Setup terminal
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    enable_raw_mode()?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut selected: Vec<bool> = vec![false; skills.len()];
    let mut list_state = ListState::default();
    list_state.select(Some(0));
    let mut preview_scroll: u16 = 0;
    let mut focus = Focus::List;

    // Pre-read all skill md contents so we don't re-read on every frame
    let preview_contents: Vec<String> = skills
        .iter()
        .map(|s| fs::read_to_string(&s.skill_md_path).unwrap_or_default())
        .collect();

    let result: Result<Option<Vec<FoundSkill>>> = (|| {
        loop {
            let cursor = list_state.selected().unwrap_or(0);
            let preview_content = preview_contents
                .get(cursor)
                .map(|s| s.as_str())
                .unwrap_or("");

            terminal.draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
                    .split(f.area());

                // Left pane: skill list
                let items: Vec<ListItem> = skills
                    .iter()
                    .enumerate()
                    .map(|(i, skill)| {
                        let check = if selected[i] { "[x]" } else { "[ ]" };
                        let check_style = if selected[i] {
                            Style::default().fg(Color::Green)
                        } else {
                            Style::default()
                        };
                        let line = Line::from(vec![
                            Span::styled(format!("{check} "), check_style),
                            Span::styled(
                                skill.path.clone(),
                                if i == cursor {
                                    Style::default().add_modifier(Modifier::BOLD)
                                } else {
                                    Style::default()
                                },
                            ),
                            if skill.name.is_empty() {
                                Span::raw("")
                            } else {
                                Span::raw(format!("  ({})", skill.name))
                            },
                        ]);
                        ListItem::new(line)
                    })
                    .collect();

                let list_title = if focus == Focus::List {
                    "Skills [FOCUSED] (Space: toggle, a: all, Tab: switch, Enter: confirm, q: cancel)"
                } else {
                    "Skills (Tab: switch focus)"
                };

                let list = List::new(items)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(list_title)
                            .border_style(if focus == Focus::List {
                                Style::default().fg(Color::Cyan)
                            } else {
                                Style::default()
                            }),
                    )
                    .highlight_style(
                        Style::default()
                            .bg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD),
                    )
                    .highlight_symbol("> ");

                f.render_stateful_widget(list, chunks[0], &mut list_state.clone());

                // Right pane: rendered markdown preview
                let selected_count = selected.iter().filter(|&&s| s).count();
                let title = format!(
                    "Preview {} {}",
                    if selected_count > 0 {
                        format!("({} selected)", selected_count)
                    } else {
                        String::new()
                    },
                    if focus == Focus::Preview {
                        "[FOCUSED] (j/k: scroll, Tab: switch)"
                    } else {
                        ""
                    }
                );

                let rendered_lines = render_markdown(preview_content);
                let preview = Paragraph::new(rendered_lines)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(title)
                            .border_style(if focus == Focus::Preview {
                                Style::default().fg(Color::Cyan)
                            } else {
                                Style::default()
                            }),
                    )
                    .wrap(Wrap { trim: false })
                    .scroll((preview_scroll, 0));
                f.render_widget(preview, chunks[1]);
            })?;

            // Handle input
            if !event::poll(std::time::Duration::from_millis(100))? {
                continue;
            }

            let event = event::read()?;
            let Event::Key(KeyEvent {
                code,
                kind,
                modifiers,
                ..
            }) = event
            else {
                continue;
            };

            if kind != KeyEventKind::Press && kind != KeyEventKind::Repeat {
                continue;
            }

            // Global: Ctrl+C exits immediately
            if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
                return Ok(None);
            }

            // Global: Tab switches focus
            if code == KeyCode::Tab {
                focus = focus.toggle();
                continue;
            }

            // Global: Enter confirms, q cancels
            match code {
                KeyCode::Enter => {
                    let result: Vec<FoundSkill> = skills
                        .iter()
                        .zip(&selected)
                        .filter(|(_, &sel)| sel)
                        .map(|(s, _)| s.clone())
                        .collect();
                    return Ok(Some(result));
                }
                KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(None),
                _ => {}
            }

            // Pane-specific input
            match focus {
                Focus::List => match code {
                    KeyCode::Char('a') | KeyCode::Char('A') => {
                        let all_selected = selected.iter().all(|&s| s);
                        if all_selected {
                            selected.fill(false);
                        } else {
                            selected.fill(true);
                        }
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        let cursor = list_state.selected().unwrap_or(0);
                        if cursor + 1 < skills.len() {
                            list_state.select(Some(cursor + 1));
                            preview_scroll = 0;
                        }
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        let cursor = list_state.selected().unwrap_or(0);
                        if cursor > 0 {
                            list_state.select(Some(cursor - 1));
                            preview_scroll = 0;
                        }
                    }
                    KeyCode::Char(' ') => {
                        let cursor = list_state.selected().unwrap_or(0);
                        if cursor < selected.len() {
                            selected[cursor] = !selected[cursor];
                            let next = cursor + 1;
                            if next < skills.len() {
                                list_state.select(Some(next));
                            }
                        }
                    }
                    _ => {}
                },
                Focus::Preview => match code {
                    KeyCode::Char('j') | KeyCode::Down | KeyCode::PageDown => {
                        preview_scroll = preview_scroll.saturating_add(1);
                    }
                    KeyCode::Char('k') | KeyCode::Up | KeyCode::PageUp => {
                        preview_scroll = preview_scroll.saturating_sub(1);
                    }
                    _ => {}
                },
            }
        }
    })();

    // Restore terminal
    disable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, LeaveAlternateScreen)?;

    match result? {
        Some(selected) => Ok(selected),
        None => Ok(Vec::new()),
    }
}
