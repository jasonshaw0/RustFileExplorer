mod files;

use std::{
    env,
    ffi::OsStr,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{Attribute, Print, SetAttribute},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use unicode_width::UnicodeWidthChar;

use files::{Entry, Preview, visible_text};

const USAGE: &str = "Usage: rustexplorer [directory]";

#[derive(Default)]
struct Scroll {
    selected: usize,
    top: usize,
}

impl Scroll {
    fn keep_visible(&mut self, len: usize, height: usize) {
        self.selected = self.selected.min(len.saturating_sub(1));
        // Scroll only when the selection leaves the window. Pull the window
        // up if filtering or resizing leaves blank rows below the list.
        self.top = self
            .top
            .min(self.selected)
            .max((self.selected + 1).saturating_sub(height))
            .min(len.saturating_sub(height));
    }
}

struct Explorer {
    directory: PathBuf,
    entries: Vec<Entry>,
    matches: Vec<usize>,
    scroll: Scroll,
    show_hidden: bool,
    filter: String,
    editing_filter: bool,
    status: String,
}

impl Explorer {
    fn new(directory: PathBuf) -> io::Result<Self> {
        let entries = files::list(&directory, false)?;
        let matches = (0..entries.len()).collect();
        Ok(Self {
            directory,
            entries,
            matches,
            scroll: Scroll::default(),
            show_hidden: false,
            filter: String::new(),
            editing_filter: false,
            status: String::new(),
        })
    }

    fn selected(&self) -> Option<&Entry> {
        self.matches
            .get(self.scroll.selected)
            .map(|&index| &self.entries[index])
    }

    fn update_matches(&mut self, select: Option<&OsStr>) {
        let query = self.filter.to_lowercase();
        self.matches = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.search_name.contains(&query))
            .map(|(index, _)| index)
            .collect();
        self.scroll = Scroll::default();
        if let Some(name) = select {
            self.scroll.selected = self
                .matches
                .iter()
                .position(|&index| self.entries[index].name == name)
                .unwrap_or(0);
        }
    }

    fn load(&mut self, path: PathBuf, show_hidden: bool, select: Option<&OsStr>) {
        match files::list(&path, show_hidden) {
            Ok(entries) => {
                if path != self.directory {
                    self.filter.clear();
                }
                self.entries = entries;
                self.directory = path;
                self.show_hidden = show_hidden;
                self.update_matches(select);
            }
            Err(error) => self.status = format!("Cannot read {} ({error})", path.display()),
        }
    }

    fn go_up(&mut self) {
        if let Some(parent) = self.directory.parent() {
            // Highlight the folder we just left so it's easy to keep browsing.
            let child = self.directory.file_name().map(OsStr::to_owned);
            self.load(parent.to_owned(), self.show_hidden, child.as_deref());
        }
    }

    fn open_selected(&mut self) -> Option<OpenPreview> {
        let entry = self.selected()?;
        let path = self.directory.join(&entry.name);
        if entry.directory {
            self.load(path, self.show_hidden, None);
            return None;
        }
        match files::preview(&path) {
            Ok(preview) => Some(OpenPreview {
                path,
                preview,
                top: 0,
            }),
            Err(error) => {
                self.status = error.to_string();
                None
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent, height: usize) -> Option<OpenPreview> {
        self.status.clear();
        if self.editing_filter {
            match key.code {
                KeyCode::Esc => {
                    self.editing_filter = false;
                    self.filter.clear();
                    self.update_matches(None);
                }
                KeyCode::Enter => self.editing_filter = false,
                KeyCode::Up => self.scroll.selected = self.scroll.selected.saturating_sub(1),
                KeyCode::Down => self.scroll.selected += 1,
                KeyCode::Backspace => {
                    self.filter.pop();
                    self.update_matches(None);
                }
                KeyCode::Char(ch)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.filter.push(ch);
                    self.update_matches(None);
                }
                _ => {}
            }
            return None;
        }

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll.selected = self.scroll.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => self.scroll.selected += 1,
            KeyCode::PageUp => self.scroll.selected = self.scroll.selected.saturating_sub(height),
            KeyCode::PageDown => self.scroll.selected += height,
            KeyCode::Home | KeyCode::Char('g') => self.scroll.selected = 0,
            KeyCode::End | KeyCode::Char('G') => {
                self.scroll.selected = self.matches.len().saturating_sub(1);
            }
            KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => self.go_up(),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => return self.open_selected(),
            KeyCode::Char('/') => self.editing_filter = true,
            KeyCode::Esc if !self.filter.is_empty() => {
                let selected = self.selected().map(|entry| entry.name.clone());
                self.filter.clear();
                self.update_matches(selected.as_deref());
            }
            KeyCode::Char('.' | 'r') => {
                let selected = self.selected().map(|entry| entry.name.clone());
                let show_hidden = self.show_hidden ^ (key.code == KeyCode::Char('.'));
                self.load(self.directory.clone(), show_hidden, selected.as_deref());
            }
            _ => {}
        }
        None
    }
}

struct OpenPreview {
    path: PathBuf,
    preview: Preview,
    top: usize,
}

impl OpenPreview {
    fn scroll(&mut self, key: KeyCode, height: usize) {
        self.top = match key {
            KeyCode::Up | KeyCode::Char('k') => self.top.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.top.saturating_add(1),
            KeyCode::PageUp => self.top.saturating_sub(height),
            KeyCode::PageDown => self.top.saturating_add(height),
            KeyCode::Home | KeyCode::Char('g') => 0,
            // Drawing clamps this to the last page once the text is wrapped.
            KeyCode::End | KeyCode::Char('G') => usize::MAX,
            _ => self.top,
        };
    }
}

struct View {
    width: usize,
    rows: u16,
}

impl View {
    fn new() -> io::Result<Self> {
        let (columns, rows) = terminal::size()?;
        Ok(Self {
            // Leave the last column free so printing a full row won't wrap.
            width: usize::from(columns.saturating_sub(1)),
            rows,
        })
    }

    fn height(&self) -> usize {
        usize::from(self.rows.saturating_sub(5))
    }

    fn line(&self, out: &mut impl Write, y: u16, text: &str, style: Attribute) -> io::Result<()> {
        let mut columns = 0;
        let end = text
            .char_indices()
            .find(|&(_, ch)| {
                columns += ch.width().unwrap_or(0);
                columns > self.width
            })
            .map_or(text.len(), |(offset, _)| offset);
        queue!(
            out,
            MoveTo(0, y),
            SetAttribute(style),
            Print(&text[..end]),
            SetAttribute(Attribute::Reset)
        )
    }

    fn draw(
        &self,
        out: &mut impl Write,
        explorer: &mut Explorer,
        preview: Option<&mut OpenPreview>,
    ) -> io::Result<()> {
        queue!(out, Clear(ClearType::All))?;
        if self.height() == 0 || self.width < 12 {
            if self.rows > 0 {
                self.line(out, 0, "Resize terminal (q quits)", Attribute::Reset)?;
            }
        } else if let Some(preview) = preview {
            self.draw_preview(out, preview)?;
        } else {
            self.draw_directory(out, explorer)?;
        }
        out.flush()
    }

    fn draw_directory(&self, out: &mut impl Write, explorer: &mut Explorer) -> io::Result<()> {
        let path = display_path(&explorer.directory, self.width.saturating_sub(6));
        self.line(out, 0, &format!("Files  {path}"), Attribute::Bold)?;

        let height = self.height();
        explorer.scroll.keep_visible(explorer.matches.len(), height);
        let top = explorer.scroll.top;
        for (y, &index) in explorer.matches.iter().skip(top).take(height).enumerate() {
            let entry = &explorer.entries[index];
            let selected = top + y == explorer.scroll.selected;
            let marker = if selected { "> " } else { "  " };
            let style = if selected {
                Attribute::Reverse
            } else if entry.directory {
                Attribute::Bold
            } else {
                Attribute::Reset
            };
            self.line(
                out,
                y as u16 + 2,
                &format!("{marker}{}", entry.label),
                style,
            )?;
        }
        if explorer.matches.is_empty() {
            let message = if explorer.filter.is_empty() {
                "  Empty directory"
            } else {
                "  No matches"
            };
            self.line(out, 2, message, Attribute::Reset)?;
        }

        let status = if explorer.editing_filter {
            format!("/{}_", visible_text(&explorer.filter))
        } else if !explorer.status.is_empty() {
            visible_text(&explorer.status)
        } else if !explorer.filter.is_empty() {
            format!(
                "/{}  ·  {} matches  ·  Esc clear",
                visible_text(&explorer.filter),
                explorer.matches.len()
            )
        } else {
            let hidden = if explorer.show_hidden {
                "shown"
            } else {
                "hidden"
            };
            format!("{} entries  ·  dotfiles {hidden}", explorer.entries.len())
        };
        self.line(out, self.rows - 3, &status, Attribute::Reset)?;
        let (first, second) = if explorer.editing_filter {
            ("Type to filter  ·  ↑↓ select", "Enter apply  ·  Esc clear")
        } else {
            (
                "↑↓ / jk move  ·  Enter open  ·  ← / h parent",
                "/ filter  ·  . hidden  ·  r reload  ·  q quit",
            )
        };
        self.line(out, self.rows - 2, first, Attribute::Reset)?;
        self.line(out, self.rows - 1, second, Attribute::Reset)
    }

    fn draw_preview(&self, out: &mut impl Write, open: &mut OpenPreview) -> io::Result<()> {
        let path = display_path(&open.path, self.width.saturating_sub(9));
        self.line(out, 0, &format!("Preview  {path}"), Attribute::Bold)?;

        let lines = preview_lines(&open.preview.text, self.width);
        let height = self.height();
        open.top = open.top.min(lines.len().saturating_sub(height));
        for (y, line) in lines.iter().skip(open.top).take(height).enumerate() {
            self.line(out, y as u16 + 2, line, Attribute::Reset)?;
        }
        let end = (open.top + height).min(lines.len());
        let status = if lines.is_empty() {
            "Empty file".to_owned()
        } else {
            let limit = if open.preview.truncated {
                "  ·  first 64 KiB"
            } else {
                ""
            };
            format!("{}–{end} of {} rows{limit}", open.top + 1, lines.len())
        };
        self.line(out, self.rows - 3, &status, Attribute::Reset)?;
        self.line(
            out,
            self.rows - 2,
            "↑↓ / jk scroll  ·  PgUp/PgDn page",
            Attribute::Reset,
        )?;
        self.line(out, self.rows - 1, "Esc / ← / q back", Attribute::Reset)
    }
}

fn display_path(path: &Path, width: usize) -> String {
    let path = visible_text(&path.to_string_lossy());
    let mut remaining = width;
    for (offset, ch) in path.char_indices().rev() {
        let columns = ch.width().unwrap_or(0);
        if columns > remaining || (offset != 0 && columns == remaining) {
            return format!("…{}", &path[offset + ch.len_utf8()..]);
        }
        remaining -= columns;
    }
    path
}

fn preview_lines(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for raw in text.lines() {
        let clean = visible_text(&raw.replace('\t', "    "));
        let mut start = 0;
        let mut columns = 0;
        for (offset, ch) in clean.char_indices() {
            let cell_width = ch.width().unwrap_or(0);
            if columns + cell_width > width {
                lines.push(clean[start..offset].to_owned());
                start = offset;
                columns = 0;
            }
            columns += cell_width;
        }
        lines.push(clean[start..].to_owned());
    }
    lines
}

struct Terminal;

impl Terminal {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        // Set up cleanup before writing to the terminal in case the write fails.
        let terminal = Self;
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        Ok(terminal)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let mut out = io::stdout();
        // Clear the frame on the Linux console, which has no alternate buffer.
        if env::var("TERM").as_deref() == Ok("linux") {
            let _ = execute!(out, Clear(ClearType::All), MoveTo(0, 0));
        }
        let _ = execute!(
            out,
            SetAttribute(Attribute::Reset),
            Show,
            LeaveAlternateScreen
        );
        let _ = terminal::disable_raw_mode();
    }
}

fn run(mut explorer: Explorer) -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other("An interactive terminal is required."));
    }
    let _terminal = Terminal::enter()?;
    let mut out = io::stdout().lock();
    let mut preview = None::<OpenPreview>;
    loop {
        let view = View::new()?;
        view.draw(&mut out, &mut explorer, preview.as_mut())?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(());
        }
        if let Some(open) = &mut preview {
            match key.code {
                KeyCode::Esc | KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h' | 'q') => {
                    preview = None;
                }
                code => open.scroll(code, view.height()),
            }
        } else if key.code == KeyCode::Char('q') && !explorer.editing_filter {
            return Ok(());
        } else {
            preview = explorer.handle_key(key, view.height());
        }
    }
}

fn start() -> io::Result<()> {
    let mut args = env::args_os().skip(1);
    let path = args.next();
    if args.next().is_some() {
        return Err(io::Error::other(USAGE));
    }
    if path
        .as_deref()
        .is_some_and(|arg| arg == "-h" || arg == "--help")
    {
        println!("{USAGE}\nBrowse from the current directory or the directory you supply.");
        return Ok(());
    }
    let path = path.map_or_else(env::current_dir, |path| PathBuf::from(path).canonicalize())?;
    run(Explorer::new(path)?)
}

fn main() -> ExitCode {
    match start() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("rustexplorer: {error}");
            ExitCode::FAILURE
        }
    }
}
