use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use ignore::WalkBuilder;

const MAX_INDEXED_FILES: usize = 200_000;

pub fn spawn_workspace_index(workspace: PathBuf) -> Receiver<Vec<String>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let files = collect_workspace_files(&workspace);
        let _ = sender.send(files);
    });
    receiver
}

fn collect_workspace_files(workspace: &Path) -> Vec<String> {
    let mut builder = WalkBuilder::new(workspace);
    builder
        .hidden(false)
        .parents(true)
        .ignore(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|entry| entry.file_name() != ".git");

    let mut files = Vec::new();
    for result in builder.build() {
        let Ok(entry) = result else {
            continue;
        };
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() && !file_type.is_symlink() {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(workspace) else {
            continue;
        };
        let Some(relative) = relative.to_str() else {
            continue;
        };
        if relative.is_empty() || relative.chars().any(char::is_control) {
            continue;
        }
        files.push(relative.replace(std::path::MAIN_SEPARATOR, "/"));
        if files.len() >= MAX_INDEXED_FILES {
            break;
        }
    }
    files.sort_unstable();
    files
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn index_respects_ignore_files_but_includes_hidden_files() {
        let directory = tempdir().unwrap();
        fs::create_dir_all(directory.path().join("src")).unwrap();
        fs::create_dir_all(directory.path().join("ignored")).unwrap();
        fs::write(directory.path().join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(directory.path().join(".env.example"), "KEY=").unwrap();
        fs::write(directory.path().join("ignored/generated.rs"), "").unwrap();
        fs::write(directory.path().join(".gitignore"), "ignored/\n").unwrap();

        let files = collect_workspace_files(directory.path());

        assert!(files.contains(&"src/main.rs".to_owned()));
        assert!(files.contains(&".env.example".to_owned()));
        assert!(files.contains(&".gitignore".to_owned()));
        assert!(!files.iter().any(|path| path.starts_with("ignored/")));
    }
}
