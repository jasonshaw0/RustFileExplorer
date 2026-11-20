# Ubuntu file explorer in Rust

A simple, read-only file explorer for Ubuntu 26.04.1, written in Rust. Browse directories, see file types and sizes, filter by name, and preview text with line numbers.

You'll need Rust and Cargo to build it.

```sh
cargo install --path . --locked
cargo run --locked
cargo run --locked -- /path/to/directory
```


| Keys | Action |
| --- | --- |
| ↑ / ↓ or `k` / `j` | Move through entries or scroll a preview |
| Page Up / Page Down | Move a screen at a time |
| Home / End or `g` / `G` | Jump to the beginning or end |
| Enter, →, or `l` | Enter the selected folder or preview the file |
| ←, Backspace, or `h` | Go to the parent folder or leave the preview |
| `/` | Filter by name as you type. Enter applies the filter, and Esc clears it |
| `.` | Toggle dotfiles |
| `r` | Refresh the current directory |
| `w` | Toggle line wrapping in the preview |
| `q` | Leave the preview or quit from the file list |
| Ctrl+C | Quit |
