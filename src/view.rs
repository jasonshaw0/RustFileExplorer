use std::{
    env,
    io::{self, Write},
    path::{Path, PathBuf},
};

use crossterm::{
    cursor::MoveTo,
    event::KeyCode,
    queue,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{self, Clear, ClearType},
};
use unicode_width::UnicodeWidthChar;

use crate::{
    Explorer,
    files::{Preview, visible_text},
};

const LIST_TOP: u16 = 4;
const PREVIEW_TOP: u16 = 3;
const FOOTER_ROWS: u16 = 3;
const GUTTER_WIDTH: usize = 8;

#[derive(Clone, Copy)]
enum Tone {
    Plain,
    Accent,
    Muted,
    Selected,
    Error,
}

pub(crate) struct View {
    width: usize,
    rows: u16,
}

impl View {
    pub(crate) fn new() -> io::Result<Self> {
        let (columns, rows) = terminal::size()?;
        Ok(Self {
            // Leave the final column free so a full row can't trigger terminal wrapping.
            width: usize::from(columns.saturating_sub(1)),
            rows,
        })
    }

    pub(crate) fn height(&self, preview: bool) -> usize {
        let start = if preview { PREVIEW_TOP } else { LIST_TOP };
        usize::from(self.rows.saturating_sub(start + FOOTER_ROWS))
    }

    pub(crate) fn draw(
        &self,
        out: &mut impl Write,
        explorer: &mut Explorer,
        preview: Option<&mut OpenPreview>,
    ) -> io::Result<()> {
        queue!(out, Clear(ClearType::All))?;
        if self.width < 23 || self.height(false) == 0 {
            if self.rows > 0 {
                self.write(out, 0, 0, "Resize terminal", Tone::Plain)?;
            }
        } else if let Some(preview) = preview {
            self.header(out, "Preview", &preview.path)?;
            self.draw_preview(out, preview)?;
        } else {
            self.header(out, "Files", &explorer.directory)?;
            self.draw_list(out, explorer)?;
        }
        out.flush()
    }

    fn header(&self, out: &mut impl Write, title: &str, path: &Path) -> io::Result<()> {
        self.write(out, 1, 0, title, Tone::Accent)?;
        self.write(out, 1, 1, &display_path(path, self.width - 2), Tone::Plain)?;
        self.divider(out, 2)
    }

    fn draw_list(&self, out: &mut impl Write, explorer: &mut Explorer) -> io::Result<()> {
        let table = Table::new(self.width - 2);
        self.write(
            out,
            1,
            3,
            &table.row("  ", "Name", "Type", "Size"),
            Tone::Muted,
        )?;

        let count = explorer.matches.len();
        let height = self.height(false);
        explorer.scroll.keep_visible(count, height);
        let top = explorer.scroll.top;
        for (offset, &index) in explorer.matches.iter().skip(top).take(height).enumerate() {
            let entry = &explorer.entries[index];
            let (marker, tone) = if top + offset == explorer.scroll.selected {
                ("> ", Tone::Selected)
            } else if entry.directory {
                ("  ", Tone::Accent)
            } else {
                ("  ", Tone::Plain)
            };
            let size = entry.size.map(size_label).unwrap_or_default();
            let row = table.row(marker, &entry.label, entry.kind, &size);
            self.write(out, 1, LIST_TOP + offset as u16, &row, tone)?;
        }
        if count == 0 {
            let message = if explorer.filter.is_empty() {
                "No visible files"
            } else {
                "No matching names"
            };
            self.write(out, 3, LIST_TOP, message, Tone::Muted)?;
        }

        let status = if explorer.editing_filter {
            format!("Filter  {}_", visible_text(&explorer.filter))
        } else if !explorer.status.is_empty() {
            visible_text(&explorer.status)
        } else if !explorer.filter.is_empty() {
            format!("{count} matches  /{}", visible_text(&explorer.filter))
        } else if count == 0 {
            "0 entries".to_owned()
        } else {
            format!("{} of {count}", explorer.scroll.selected + 1)
        };
        let (tone, detail) = if explorer.editing_filter {
            (Tone::Accent, "")
        } else if !explorer.status.is_empty() {
            (Tone::Error, "")
        } else if explorer.show_hidden {
            (Tone::Plain, "Dotfiles on")
        } else {
            (Tone::Plain, "")
        };
        let hints = if explorer.editing_filter {
            [
                "Enter Apply   Esc Clear   ↑↓ Move",
                "Enter Apply   Esc Clear",
                "Enter Apply",
            ]
        } else if !explorer.filter.is_empty() {
            [
                "↑↓ Move   Enter Open   Esc Clear   / Edit   q Quit",
                "Enter Open   Esc Clear   q Quit",
                "Esc Clear   q Quit",
            ]
        } else {
            [
                "↑↓ Move   Enter Open   ← Up   / Filter   . Hidden   r Refresh   q Quit",
                "↑↓ Move  ↵ Open  ← Up  / Find  . Hidden  r Reload  q Quit",
                "↵ Open / Find q Quit",
            ]
        };
        self.footer(out, &status, tone, detail, &hints)
    }

    fn draw_preview(&self, out: &mut impl Write, preview: &mut OpenPreview) -> io::Result<()> {
        let height = self.height(true);
        preview.layout(self.width - GUTTER_WIDTH - 2, height);
        let page = &preview.rows[preview.top..(preview.top + height).min(preview.rows.len())];
        for (offset, row) in page.iter().enumerate() {
            let y = PREVIEW_TOP + offset as u16;
            let gutter = if row.continuation {
                "    · │".to_owned()
            } else {
                format!("{:>5} │", row.line)
            };
            self.write(out, 1, y, &gutter, Tone::Muted)?;
            self.write(out, (GUTTER_WIDTH + 1) as u16, y, &row.text, Tone::Plain)?;
        }
        let status = match (page.first(), page.last()) {
            (Some(first), Some(last)) if first.line != last.line => {
                format!(
                    "Lines {}–{} of {}",
                    first.line, last.line, preview.line_count
                )
            }
            (Some(first), _) => format!("Line {} of {}", first.line, preview.line_count),
            _ => "Empty file".to_owned(),
        };
        let detail = if preview.content.truncated {
            "First 64 KiB"
        } else if preview.wrap {
            "Wrap on"
        } else {
            "Wrap off"
        };
        let hints = [
            "↑↓ Scroll   PgUp/PgDn Page   w Wrap   Esc Back",
            "↑↓ Scroll   w Wrap   Esc Back",
            "w Wrap   Esc Back",
        ];
        self.footer(out, &status, Tone::Plain, detail, &hints)
    }

    fn footer(
        &self,
        out: &mut impl Write,
        status: &str,
        tone: Tone,
        detail: &str,
        hints: &[&str],
    ) -> io::Result<()> {
        self.divider(out, self.rows - FOOTER_ROWS)?;
        self.write(out, 1, self.rows - 2, status, tone)?;
        let detail_width = cells(detail);
        if detail_width > 0 && cells(status) + detail_width + 4 <= self.width {
            let x = self.width - detail_width - 1;
            self.write(out, x as u16, self.rows - 2, detail, Tone::Muted)?;
        }

        let hint = hints
            .iter()
            .copied()
            .find(|hint| cells(hint) + 2 <= self.width);
        self.write(out, 1, self.rows - 1, hint.unwrap_or_default(), Tone::Muted)
    }

    fn divider(&self, out: &mut impl Write, y: u16) -> io::Result<()> {
        self.write(out, 1, y, &"─".repeat(self.width - 2), Tone::Muted)
    }

    fn write(
        &self,
        out: &mut impl Write,
        x: u16,
        y: u16,
        text: &str,
        tone: Tone,
    ) -> io::Result<()> {
        let available = self.width.saturating_sub(usize::from(x));
        queue!(out, MoveTo(x, y))?;
        match tone {
            Tone::Plain => {}
            Tone::Accent => queue!(
                out,
                SetForegroundColor(Color::Cyan),
                SetAttribute(Attribute::Bold)
            )?,
            Tone::Muted => queue!(out, SetForegroundColor(Color::DarkGrey))?,
            Tone::Selected => queue!(out, SetAttribute(Attribute::Reverse))?,
            Tone::Error => queue!(out, SetForegroundColor(Color::Yellow))?,
        }
        queue!(
            out,
            Print(prefix(text, available)),
            SetAttribute(Attribute::Reset),
            ResetColor
        )
    }
}

struct Table {
    name_width: usize,
    columns: bool,
}

impl Table {
    fn new(width: usize) -> Self {
        let columns = width >= 44;
        let metadata_width = if columns { 20 } else { 0 };
        Self {
            name_width: width - 2 - metadata_width,
            columns,
        }
    }

    fn row(&self, marker: &str, name: &str, kind: &str, size: &str) -> String {
        let name = shorten(name, self.name_width);
        let padding = self.name_width - cells(&name);
        let mut row = format!("{marker}{name}{:padding$}", "");
        if self.columns {
            row.push_str(&format!("  {kind:<7}  {size:>9}"));
        }
        row
    }
}

fn size_label(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    for unit in ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"] {
        value /= 1024.0;
        if value < 1024.0 {
            return format!("{value:.1} {unit}");
        }
    }
    unreachable!("u64 sizes fit in EiB")
}

fn display_path(path: &Path, width: usize) -> String {
    let home = env::var_os("HOME").filter(|home| !home.is_empty());
    let path = match home.as_ref().and_then(|home| path.strip_prefix(home).ok()) {
        Some(relative) if relative.as_os_str().is_empty() => "~".to_owned(),
        Some(relative) => format!("~{}{}", std::path::MAIN_SEPARATOR, relative.display()),
        None => path.display().to_string(),
    };
    let path = visible_text(&path);
    if cells(&path) <= width {
        return path;
    }
    // Keep the part of the path that identifies the current folder or file.
    let mut remaining = width.saturating_sub(1);
    for (offset, ch) in path.char_indices().rev() {
        let columns = char_cells(ch);
        if columns > remaining {
            return format!("…{}", &path[offset + ch.len_utf8()..]);
        }
        remaining -= columns;
    }
    path
}

fn char_cells(ch: char) -> usize {
    ch.width().unwrap_or(0)
}

fn cells(text: &str) -> usize {
    text.chars().map(char_cells).sum()
}

fn prefix(text: &str, width: usize) -> &str {
    let mut used = 0;
    for (offset, ch) in text.char_indices() {
        used += char_cells(ch);
        if used > width {
            return &text[..offset];
        }
    }
    text
}

fn shorten(text: &str, width: usize) -> String {
    if cells(text) > width {
        format!("{}…", prefix(text, width.saturating_sub(1)))
    } else {
        text.to_owned()
    }
}

struct PreviewRow {
    line: usize,
    continuation: bool,
    text: String,
}

pub(crate) struct OpenPreview {
    path: PathBuf,
    content: Preview,
    line_count: usize,
    top: usize,
    wrap: bool,
    layout: Option<(usize, bool)>,
    rows: Vec<PreviewRow>,
}

impl OpenPreview {
    pub(crate) fn new(path: PathBuf, content: Preview) -> Self {
        Self {
            path,
            line_count: content.text.lines().count(),
            content,
            top: 0,
            wrap: true,
            layout: None,
            rows: Vec::new(),
        }
    }

    pub(crate) fn handle_key(&mut self, key: KeyCode, height: usize) {
        self.top = match key {
            KeyCode::Up | KeyCode::Char('k') => self.top.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.top.saturating_add(1),
            KeyCode::PageUp => self.top.saturating_sub(height),
            KeyCode::PageDown => self.top.saturating_add(height),
            KeyCode::Home | KeyCode::Char('g') => 0,
            KeyCode::End | KeyCode::Char('G') => usize::MAX,
            KeyCode::Char('w') => {
                self.wrap = !self.wrap;
                self.top
            }
            _ => self.top,
        };
    }

    fn layout(&mut self, width: usize, height: usize) {
        if self.layout != Some((width, self.wrap)) {
            // Rewrapping changes screen row numbers. Keep the same file line in view.
            let anchor = self.rows.get(self.top).map(|row| row.line);
            self.rows = preview_rows(&self.content.text, width, self.wrap);
            if let Some(line) = anchor {
                self.top = self.rows.partition_point(|row| row.line < line);
            }
            self.layout = Some((width, self.wrap));
        }
        self.top = self.top.min(self.rows.len().saturating_sub(height));
    }
}

fn preview_rows(text: &str, width: usize, wrap: bool) -> Vec<PreviewRow> {
    debug_assert!(width >= 2);
    let mut rows = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let clean = visible_text(&raw.replace('\t', "    "));
        let line = index + 1;
        if !wrap {
            rows.push(PreviewRow {
                line,
                continuation: false,
                text: shorten(&clean, width),
            });
            continue;
        }

        let mut remaining = clean.as_str();
        let mut continuation = false;
        loop {
            let (chunk, next) = wrap_line(remaining, width);
            rows.push(PreviewRow {
                line,
                continuation,
                text: chunk.to_owned(),
            });
            if next.is_empty() {
                break;
            }
            remaining = next;
            continuation = true;
        }
    }
    rows
}

fn wrap_line(text: &str, width: usize) -> (&str, &str) {
    let chunk = prefix(text, width);
    let rest = &text[chunk.len()..];
    if rest.is_empty() {
        return (chunk, rest);
    }
    if let Some(next) = rest.strip_prefix(' ') {
        return (chunk, next);
    }

    // Prefer a nearby word break. A long token can still take the whole row.
    if let Some(space) = chunk.rfind(' ')
        && cells(&chunk[..space]) >= width / 2
        && !chunk[..space].trim().is_empty()
    {
        return (&chunk[..space], &text[space + 1..]);
    }
    (chunk, rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_keeps_source_line_numbers_across_word_wraps() {
        let rows = preview_rows("open field station\n\n測試測試測", 8, true);
        let rendered: Vec<_> = rows
            .iter()
            .map(|row| (row.line, row.continuation, row.text.as_str()))
            .collect();
        assert_eq!(
            rendered,
            [
                (1, false, "open"),
                (1, true, "field"),
                (1, true, "station"),
                (2, false, ""),
                (3, false, "測試測試"),
                (3, true, "測"),
            ]
        );
    }

    #[test]
    fn table_keeps_columns_aligned_with_wide_names() {
        let table = Table::new(48);
        let header = table.row("  ", "Name", "Type", "Size");
        let row = table.row("> ", &"測".repeat(14), "File", "10 B");
        let col = |line: &str, word| cells(line.split_once(word).unwrap().0);
        assert_eq!(col(&header, "Type"), col(&row, "File"));
        assert_eq!(cells(&row), 48);
        assert!(row.contains('…'));
    }
}
