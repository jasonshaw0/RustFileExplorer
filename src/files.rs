use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{self, Read},
    path::Path,
    str,
};

const PREVIEW_LIMIT: usize = 64 * 1024;

pub struct Entry {
    // Keep the original name for filesystem calls. Display text can be lossy.
    pub name: OsString,
    pub search_name: String,
    pub label: String,
    pub directory: bool,
    pub kind: &'static str,
    pub size: Option<u64>,
}

pub fn list(path: &Path, show_hidden: bool) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in fs::read_dir(path)? {
        let item = item?;
        let name = item.file_name();
        let filename = name.to_string_lossy();
        if !show_hidden && filename.starts_with('.') {
            continue;
        }
        let kind = item.file_type().ok();
        let link = kind.is_some_and(|kind| kind.is_symlink());
        let directory = if link {
            item.path().is_dir()
        } else {
            kind.is_some_and(|kind| kind.is_dir())
        };
        let suffix = match (directory, link) {
            (true, true) => "/@",
            (true, false) => "/",
            (false, true) => "@",
            (false, false) => "",
        };
        let regular = kind.is_some_and(|kind| kind.is_file());
        let label_kind = if link {
            "Link"
        } else if directory {
            "Folder"
        } else if regular {
            "File"
        } else {
            "Other"
        };
        let size = if regular {
            item.metadata().ok().map(|metadata| metadata.len())
        } else {
            None
        };
        entries.push(Entry {
            search_name: filename.to_lowercase(),
            label: format!("{}{suffix}", visible_text(&filename)),
            directory,
            kind: label_kind,
            size,
            name,
        });
    }
    entries.sort_unstable_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.search_name.cmp(&b.search_name))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(entries)
}

pub struct Preview {
    pub text: String,
    pub truncated: bool,
}

pub fn preview(path: &Path) -> io::Result<Preview> {
    if !path.metadata()?.is_file() {
        return Err(io::Error::other("Only regular files can be previewed."));
    }

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A file can turn into a named pipe between the metadata check and open.
        // Without this flag, opening that pipe could wait for a writer.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("Only regular files can be previewed."));
    }

    let mut bytes = Vec::new();
    // Read one extra byte to tell a file at the limit from a longer one.
    file.take((PREVIEW_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)?;
    let truncated = bytes.len() > PREVIEW_LIMIT;
    bytes.truncate(PREVIEW_LIMIT);
    let text = match str::from_utf8(&bytes) {
        Ok(text) => Some(text),
        Err(error) if truncated && error.error_len().is_none() => {
            // The preview limit can land in the middle of a UTF-8 character.
            str::from_utf8(&bytes[..error.valid_up_to()]).ok()
        }
        Err(_) => None,
    };
    let text = text
        .filter(|text| !text.contains('\0'))
        .ok_or_else(|| io::Error::other("Preview needs a UTF-8 text file."))?;
    Ok(Preview {
        text: text.to_owned(),
        truncated,
    })
}

// Filenames and file contents can contain bytes the terminal treats as commands.
pub fn visible_text(text: &str) -> String {
    let mut visible = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_control() {
            visible.extend(ch.escape_default());
        } else {
            visible.push(ch);
        }
    }
    visible
}
