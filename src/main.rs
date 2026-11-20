mod files;
mod view;

use std::{
    env,
    ffi::OsStr,
    io::{self, BufWriter, IsTerminal},
    path::PathBuf,
    process::ExitCode,
};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    style::{Attribute, SetAttribute},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};

use files::Entry;
use view::{OpenPreview, View};

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
            Ok(preview) => Some(OpenPreview::new(path, preview)),
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
    let mut out = BufWriter::with_capacity(32 * 1024, io::stdout().lock());
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
                code => open.handle_key(code, view.height(true)),
            }
        } else if key.code == KeyCode::Char('q') && !explorer.editing_filter {
            return Ok(());
        } else {
            preview = explorer.handle_key(key, view.height(false));
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
