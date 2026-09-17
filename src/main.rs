use std::io;
use std::path::PathBuf;
use std::time::Duration;

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

// ============ Логика конвертации ============

fn hex_to_rgb(hex_str: &str) -> Result<(u8, u8, u8), String> {
    let s = hex_str.trim().trim_start_matches('#');
    if s.len() != 3 && s.len() != 6 {
        return Err("HEX должен быть 3 или 6 символов".into());
    }
    let expanded;
    let s = if s.len() == 3 {
        expanded = format!(
            "{}{}{}{}{}{}",
            &s[0..1],
            &s[0..1],
            &s[1..2],
            &s[1..2],
            &s[2..3],
            &s[2..3]
        );
        expanded.as_str()
    } else {
        s
    };

    let r = u8::from_str_radix(&s[0..2], 16).map_err(|e| e.to_string())?;
    let g = u8::from_str_radix(&s[2..4], 16).map_err(|e| e.to_string())?;
    let b = u8::from_str_radix(&s[4..6], 16).map_err(|e| e.to_string())?;
    Ok((r, g, b))
}

/// rgb (0–255) to cmyk (0–100%) через icc-профили.
fn rgb_to_cmyk_icc(
    r: u8,
    g: u8,
    b: u8,
    srgb_path: &str,
    cmyk_path: &str,
) -> Result<(u8, u8, u8, u8), String> {
    let input = lcms2::Profile::new_file(srgb_path)
        .map_err(|e| format!("Не удалось загрузить sRGB ICC: {e}"))?;
    let output = lcms2::Profile::new_file(cmyk_path)
        .map_err(|e| format!("Не удалось загрузить CMYK ICC: {e}"))?;

    let transform = lcms2::Transform::new(
        &input,
        lcms2::PixelFormat::RGB_8,
        &output,
        lcms2::PixelFormat::CMYK_8,
        lcms2::Intent::RelativeColorimetric,
    )
    .map_err(|e| format!("Ошибка создания transform: {e}"))?;

    let mut src = [r, g, b];
    let mut dst = [0u8; 4];
    transform.transform_pixels(&mut src, &mut dst);

    let to_pc = |v: u8| ((v as f32 / 255.0) * 100.0).round() as u8;
    Ok((to_pc(dst[0]), to_pc(dst[1]), to_pc(dst[2]), to_pc(dst[3])))
}

// ============ Состояние приложения ============

#[derive(PartialEq, Eq, Clone, Copy)]
enum Field {
    Hex,
    SrgbPath,
    CmykPath,
}

struct App {
    hex_input: String,
    srgb_path: String,
    cmyk_path: String,
    rgb: Option<(u8, u8, u8)>,
    cmyk: Option<(u8, u8, u8, u8)>,
    error: Option<String>,
    status: String,
    focus: Field,
    preview_color: Option<Color>,
}

impl App {
    fn new() -> Self {
        Self {
            hex_input: String::new(),
            srgb_path: "sRGB_v4_ICC.icc".to_string(),
            cmyk_path: "CoatedFOGRA27.icc".to_string(),
            rgb: None,
            cmyk: None,
            error: None,
            status: "Введите HEX и нажмите Enter. Tab — переключение полей.".to_string(),
            focus: Field::Hex,
            preview_color: None,
        }
    }

    fn current_field_mut(&mut self) -> &mut String {
        match self.focus {
            Field::Hex => &mut self.hex_input,
            Field::SrgbPath => &mut self.srgb_path,
            Field::CmykPath => &mut self.cmyk_path,
        }
    }

    fn next_field(&mut self) {
        self.focus = match self.focus {
            Field::Hex => Field::SrgbPath,
            Field::SrgbPath => Field::CmykPath,
            Field::CmykPath => Field::Hex,
        };
    }

    fn prev_field(&mut self) {
        self.focus = match self.focus {
            Field::Hex => Field::CmykPath,
            Field::SrgbPath => Field::Hex,
            Field::CmykPath => Field::SrgbPath,
        };
    }

    fn convert(&mut self) {
        self.error = None;
        let rgb = match hex_to_rgb(&self.hex_input) {
            Ok(v) => v,
            Err(e) => {
                self.error = Some(format!("HEX: {e}"));
                self.rgb = None;
                self.cmyk = None;
                self.preview_color = None;
                return;
            }
        };
        self.rgb = Some(rgb);
        self.preview_color = Some(Color::Rgb(rgb.0, rgb.1, rgb.2));

        match rgb_to_cmyk_icc(rgb.0, rgb.1, rgb.2, &self.srgb_path, &self.cmyk_path) {
            Ok(cmyk) => {
                self.cmyk = Some(cmyk);
                self.status =
                    "OK. c — копировать CMYK, r — копировать RGB, h — копировать HEX".into();
            }
            Err(e) => {
                self.cmyk = None;
                self.error = Some(e);
                self.status = "Ошибка конверсии CMYK (проверь пути к ICC)".into();
            }
        }
    }
}

// ============ Ввод ============

fn handle_key(app: &mut App, key: KeyCode) -> bool {
    match key {
        KeyCode::Esc => return true,
        KeyCode::Tab => app.next_field(),
        KeyCode::BackTab => app.prev_field(),
        KeyCode::Enter => {
            if app.focus == Field::Hex {
                app.convert();
            } else {
                app.next_field();
            }
        }
        KeyCode::Backspace => {
            let f = app.current_field_mut();
            f.pop();
        }
        KeyCode::Char(c) => {
            let f = app.current_field_mut();
            f.push(c);
        }
        _ => {}
    }
    false
}
fn handle_command_key(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Char('c') if app.cmyk.is_some() && app.hex_input.is_empty() => {}
        _ => {}
    }
}

// ============ Отрисовка ============

fn ui(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3), // HEX input
            Constraint::Length(3), // sRGB path
            Constraint::Length(3), // CMYK path
            Constraint::Length(3), // RGB output
            Constraint::Length(3), // CMYK output
            Constraint::Length(3), // preview
            Constraint::Min(3),    // help/status
        ])
        .split(area);

    // ----- HEX -----
    let hex_style = if app.focus == Field::Hex {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let hex_para = Paragraph::new(app.hex_input.as_str()).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(hex_style)
            .title(" HEX (например: #FF5733 или FF5733) "),
    );
    frame.render_widget(hex_para, chunks[0]);

    // ----- sRGB path -----
    let srgb_style = if app.focus == Field::SrgbPath {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let srgb_para = Paragraph::new(app.srgb_path.as_str()).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(srgb_style)
            .title(" Путь к sRGB ICC "),
    );
    frame.render_widget(srgb_para, chunks[1]);

    // ----- CMYK path -----
    let cmyk_style = if app.focus == Field::CmykPath {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let cmyk_para = Paragraph::new(app.cmyk_path.as_str()).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(cmyk_style)
            .title(" Путь к CMYK ICC "),
    );
    frame.render_widget(cmyk_para, chunks[2]);

    // ----- RGB output -----
    let rgb_text = match app.rgb {
        Some((r, g, b)) => format!("RGB: {r}, {g}, {b}"),
        None => "RGB: —".to_string(),
    };
    let rgb_para =
        Paragraph::new(rgb_text).block(Block::default().borders(Borders::ALL).title(" RGB "));
    frame.render_widget(rgb_para, chunks[3]);

    // ----- CMYK output -----
    let cmyk_text = match app.cmyk {
        Some((c, m, y, k)) => format!("CMYK: {c}%, {m}%, {y}%, {k}%"),
        None => "CMYK: —".to_string(),
    };
    let cmyk_para =
        Paragraph::new(cmyk_text).block(Block::default().borders(Borders::ALL).title(" CMYK "));
    frame.render_widget(cmyk_para, chunks[4]);

    // ----- Preview -----
    let preview_para = if let Some(color) = app.preview_color {
        Paragraph::new("  ")
            .style(Style::default().bg(color))
            .block(Block::default().borders(Borders::ALL).title(" Превью "))
    } else {
        Paragraph::new("(нет цвета)")
            .block(Block::default().borders(Borders::ALL).title(" Превью "))
    };
    frame.render_widget(preview_para, chunks[5]);

    // ----- Status / Help / Error -----
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        "Enter — конвертировать | Tab — след. поле | Shift+Tab — пред. поле | Esc — выход",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(Span::styled(
        "Копирование: Ctrl+R — RGB | Ctrl+C — CMYK | Ctrl+H — HEX | + ALT — в строчку (rgb: 0, 0, 0)",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(app.status.as_str()));
    if let Some(err) = &app.error {
        lines.push(Line::from(Span::styled(
            format!("Ошибка: {err}"),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
    }
    let status_para = Paragraph::new(lines)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title(" Статус "));
    frame.render_widget(status_para, chunks[6]);
}

// ============ Копирование в буфер через arboard ============

fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("Не удалось открыть буфер обмена: {e}"))?;
    clipboard
        .set_text(text.to_string())
        .map_err(|e| format!("Не удалось скопировать: {e}"))
}
fn paste_from_clipboard() -> Result<String, String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("Не удалось открыть буфер обмена: {e}"))?;
    clipboard
        .get_text()
        .map_err(|e| format!("Не удалось прочитать буфер: {e}"))
}

// ============ Главный цикл ============

fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let res = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(e) = res {
        eprintln!("Ошибка: {e}");
    }
    Ok(())
}

fn run_app<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> io::Result<()>
where
    std::io::Error: From<<B as Backend>::Error>,
{
    loop {
        terminal.draw(|f| ui(f, app))?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if key.modifiers.contains(event::KeyModifiers::CONTROL) {
                    let with_alt = key.modifiers.contains(event::KeyModifiers::ALT);
                    match key.code {
                        KeyCode::Char('v')
                        | KeyCode::Char('V')
                        | KeyCode::Char('м')
                        | KeyCode::Char('М') => {
                            match paste_from_clipboard() {
                                Ok(text) => {
                                    // Берём только первую строку и убираем пробелы/переводы строк
                                    let cleaned =
                                        text.lines().next().unwrap_or("").trim().to_string();

                                    // Санити-чек: оставляем только # и hex-символы
                                    let filtered: String = cleaned
                                        .chars()
                                        .filter(|c| c.is_ascii_hexdigit() || *c == '#')
                                        .collect();

                                    if filtered.is_empty() {
                                        app.error = Some("В буфере нет HEX-цвета".into());
                                    } else {
                                        // Вставляем в поле, которое сейчас в фокусе
                                        let field = app.current_field_mut();
                                        field.push_str(&filtered);
                                        app.status = format!("Вставлено: {filtered}");
                                        app.error = None;
                                    }
                                }
                                Err(e) => {
                                    app.error = Some(e);
                                }
                            }
                        }
                        KeyCode::Char('r')
                        | KeyCode::Char('R')
                        | KeyCode::Char('к')
                        | KeyCode::Char('К') => {
                            if let Some((r, g, b)) = app.rgb {
                                let text = if with_alt {
                                    format!("RGB: {r}, {g}, {b}")
                                } else {
                                    format!("R {r}\nG {g}\nB {b}")
                                };
                                match copy_to_clipboard(&text) {
                                    Ok(()) => {
                                        app.status = format!("Скопировано: {text}");
                                    }
                                    Err(e) => {
                                        app.error = Some(e);
                                    }
                                }
                            }
                        }
                        KeyCode::Char('c')
                        | KeyCode::Char('C')
                        | KeyCode::Char('с')
                        | KeyCode::Char('С') => {
                            if let Some((c, m, y, k)) = app.cmyk {
                                let text = if with_alt {
                                    format!("CMYK: {c}, {m}, {y}, {k}")
                                } else {
                                    format!("C {c}\nM {m}\nY {y}\nK {k}")
                                };
                                match copy_to_clipboard(&text) {
                                    Ok(()) => {
                                        app.status = format!("Скопировано: {text}");
                                    }
                                    Err(e) => {
                                        app.error = Some(e);
                                    }
                                }
                            }
                        }
                        KeyCode::Char('h')
                        | KeyCode::Char('H')
                        | KeyCode::Char('р')
                        | KeyCode::Char('Р') => {
                            let text = app.hex_input.trim().to_string();
                            match copy_to_clipboard(&text) {
                                Ok(()) => {
                                    app.status = "HEX скопирован в буфер".into();
                                }
                                Err(e) => {
                                    app.error = Some(e);
                                }
                            }
                        }
                        _ => {}
                    }
                    continue;
                }

                if handle_key(app, key.code) {
                    return Ok(());
                }
            }
        }
    }
}
