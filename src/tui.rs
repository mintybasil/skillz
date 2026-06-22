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

    // Result is Ok(Some(selected)) on confirm, Ok(None) on cancel, Err on failure
    let result: Result<Option<Vec<FoundSkill>>> = (|| {
        loop {
            let cursor = list_state.selected().unwrap_or(0);

            // Read preview content for the currently highlighted skill
            let preview_content = skills
                .get(cursor)
                .and_then(|s| fs::read_to_string(&s.skill_md_path).ok())
                .unwrap_or_default();

            terminal.draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
                    .split(f.area());

                // Left pane: skill list with toggle checkboxes
                let items: Vec<ListItem> = skills
                    .iter()
                    .enumerate()
                    .map(|(i, skill)| {
                        let check = if selected[i] { "[x]" } else { "[ ]" };
                        let style = if selected[i] {
                            Style::default().fg(Color::Green)
                        } else {
                            Style::default()
                        };
                        let line = Line::from(vec![
                            Span::styled(format!("{check} "), style),
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

                let list = List::new(items)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title("Skills (Space: toggle, a: all, Enter: confirm, q: cancel)"),
                    )
                    .highlight_style(
                        Style::default()
                            .bg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD),
                    )
                    .highlight_symbol("> ");

                f.render_stateful_widget(list, chunks[0], &mut list_state.clone());

                // Right pane: preview of SKILL.md
                let selected_count = selected.iter().filter(|&&s| s).count();
                let title = format!(
                    "SKILL.md preview {}",
                    if selected_count > 0 {
                        format!("({} selected)", selected_count)
                    } else {
                        String::new()
                    }
                );

                let preview = Paragraph::new(preview_content)
                    .block(Block::default().borders(Borders::ALL).title(title))
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

            // Ctrl+C exits immediately
            if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
                return Ok(None);
            }

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
                KeyCode::PageDown => {
                    preview_scroll = preview_scroll.saturating_add(10);
                }
                KeyCode::PageUp => {
                    preview_scroll = preview_scroll.saturating_sub(10);
                }
                _ => {}
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
