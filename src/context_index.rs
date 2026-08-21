use std::env;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use ignore::WalkBuilder;

const MAX_RESULTS: usize = 100;
const MAX_FALLBACK_ENTRIES: usize = 20_000;
const SEARCH_BUDGET: Duration = Duration::from_millis(300);

#[derive(Debug)]
pub struct SearchRequest {
    pub generation: u64,
    pub query: String,
}

#[derive(Debug)]
pub struct SearchResult {
    pub generation: u64,
    pub files: Vec<String>,
    pub complete: bool,
}

pub fn spawn_workspace_search(
    workspace: PathBuf,
) -> (Sender<SearchRequest>, Receiver<SearchResult>) {
    let (request_sender, request_receiver) = mpsc::channel::<SearchRequest>();
    let (result_sender, result_receiver) = mpsc::channel();
    thread::spawn(move || {
        let fd = find_executable(["fd", "fdfind"]);
        while let Ok(mut request) = request_receiver.recv() {
            while let Ok(newer) = request_receiver.try_recv() {
                request = newer;
            }
            if let Some((directory, display)) = exact_directory(&workspace, &request.query) {
                if result_sender
                    .send(SearchResult {
                        generation: request.generation,
                        files: vec![display.clone()],
                        complete: false,
                    })
                    .is_err()
                {
                    break;
                }
                let mut files = walk_files(&directory, &display, "");
                files.retain(|path| path != &display);
                files.insert(0, display);
                files.truncate(MAX_RESULTS);
                if result_sender
                    .send(SearchResult {
                        generation: request.generation,
                        files,
                        complete: true,
                    })
                    .is_err()
                {
                    break;
                }
                continue;
            }

            let files = if request.query.is_empty() {
                shallow_workspace_files(&workspace)
            } else if query_is_scoped(&workspace, &request.query) {
                search_with_walker(&workspace, &request.query)
            } else {
                fd.as_deref()
                    .and_then(|fd| search_with_fd(fd, &workspace, &request.query))
                    .unwrap_or_else(|| search_with_walker(&workspace, &request.query))
            };
            if result_sender
                .send(SearchResult {
                    generation: request.generation,
                    files,
                    complete: true,
                })
                .is_err()
            {
                break;
            }
        }
    });
    (request_sender, result_receiver)
}

fn shallow_workspace_files(workspace: &Path) -> Vec<String> {
    let Ok(entries) = workspace.read_dir() else {
        return Vec::new();
    };
    let mut files: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_type()
                .is_ok_and(|kind| kind.is_file() || kind.is_symlink())
        })
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|path| !path.chars().any(char::is_control))
        .collect();
    files.sort_unstable();
    files.truncate(MAX_RESULTS);
    files
}

fn find_executable<const N: usize>(names: [&str; N]) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for directory in env::split_paths(&path) {
        for name in names {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn search_with_fd(fd: &Path, workspace: &Path, query: &str) -> Option<Vec<String>> {
    let query = normalize_query(query);
    let (search_root, display_prefix, needle) = scoped_search(workspace, query);
    let max_results = MAX_RESULTS.to_string();
    let mut command = Command::new(fd);
    command.args([
        "--base-directory",
        search_root.to_str()?,
        "--max-results",
        &max_results,
        "--type",
        "f",
        "--hidden",
        "--full-path",
        "--exclude",
        ".git",
        "--color",
        "never",
    ]);
    let root_gitignore = workspace.join(".gitignore");
    if search_root == workspace && root_gitignore.is_file() {
        command.arg("--ignore-file").arg(root_gitignore);
    }
    let pattern = if search_root != workspace || workspace.join(".git").exists() {
        subsequence_regex(needle)
    } else {
        literal_regex(needle)
    };
    command.arg(pattern);

    let mut output = tempfile::tempfile().ok()?;
    command
        .stdout(Stdio::from(output.try_clone().ok()?))
        .stderr(Stdio::null());
    let mut child = command.spawn().ok()?;
    let deadline = Instant::now() + SEARCH_BUDGET;
    let completed = loop {
        match child.try_wait().ok()? {
            Some(status) => break status.success(),
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    output.seek(SeekFrom::Start(0)).ok()?;
    let mut stdout = String::new();
    output.read_to_string(&mut stdout).ok()?;
    if !completed && stdout.is_empty() {
        return Some(Vec::new());
    }
    let mut files: Vec<String> = stdout
        .lines()
        .filter(|path| !path.is_empty() && !path.chars().any(char::is_control))
        .map(|path| format!("{display_prefix}{}", path.replace('\\', "/")))
        .collect();
    files.sort_unstable();
    files.dedup();
    Some(files)
}

fn literal_regex(query: &str) -> String {
    escaped_regex(query, false)
}

fn subsequence_regex(query: &str) -> String {
    escaped_regex(query, true)
}

fn escaped_regex(query: &str, spread: bool) -> String {
    if query.is_empty() {
        return ".".to_owned();
    }
    let mut pattern = String::new();
    for ch in query.chars() {
        if ".+*?()|[]{}^$\\".contains(ch) {
            pattern.push('\\');
        }
        pattern.push(ch);
        if spread {
            pattern.push_str(".*");
        }
    }
    pattern
}

fn normalize_query(query: &str) -> &str {
    query.strip_prefix("./").unwrap_or(query)
}

fn search_with_walker(workspace: &Path, query: &str) -> Vec<String> {
    let query = normalize_query(query);
    let (search_root, display_prefix, needle) = scoped_search(workspace, query);
    walk_files(&search_root, &display_prefix, needle)
}

fn walk_files(search_root: &Path, display_prefix: &str, needle: &str) -> Vec<String> {
    let mut builder = WalkBuilder::new(search_root);
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
    for (visited, result) in builder.build().enumerate() {
        if visited >= MAX_FALLBACK_ENTRIES || files.len() >= MAX_RESULTS {
            break;
        }
        let Ok(entry) = result else {
            continue;
        };
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() && !file_type.is_symlink() {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(search_root) else {
            continue;
        };
        let Some(relative) = relative.to_str() else {
            continue;
        };
        if relative.is_empty() || relative.chars().any(char::is_control) {
            continue;
        }
        let relative = relative.replace(std::path::MAIN_SEPARATOR, "/");
        let display = format!("{display_prefix}{relative}");
        if fuzzy_subsequence(&display, needle) {
            files.push(display);
        }
    }
    files.sort_unstable();
    files
}

fn exact_directory(workspace: &Path, query: &str) -> Option<(PathBuf, String)> {
    if query.is_empty() {
        return None;
    }
    let home = env::var_os("HOME").map(PathBuf::from);
    let without_trailing = query.trim_end_matches('/');
    let directory = if query == "/" {
        PathBuf::from("/")
    } else if query == "~" || query == "~/" {
        home?
    } else if let Some(relative) = without_trailing.strip_prefix("~/") {
        home?.join(relative)
    } else {
        workspace.join(without_trailing)
    };
    if !directory.is_dir() {
        return None;
    }
    let display = if query.ends_with('/') {
        query.to_owned()
    } else if query == "." {
        "./".to_owned()
    } else {
        format!("{query}/")
    };
    Some((directory, display))
}

fn query_is_scoped(workspace: &Path, query: &str) -> bool {
    scoped_search(workspace, normalize_query(query)).0 != workspace
}

fn scoped_search<'a>(workspace: &Path, query: &'a str) -> (PathBuf, String, &'a str) {
    let home = env::var_os("HOME").map(PathBuf::from);
    let (root, scoped_query, display_root) = match (query.strip_prefix("~/"), home) {
        (Some(query), Some(home)) => (home, query, "~/"),
        _ => (workspace.to_path_buf(), query, ""),
    };
    for (separator, _) in scoped_query.match_indices('/').rev() {
        let prefix = &scoped_query[..=separator];
        let candidate = root.join(prefix);
        if candidate.is_dir() {
            return (
                candidate,
                format!("{display_root}{prefix}"),
                &scoped_query[separator + 1..],
            );
        }
    }
    (workspace.to_path_buf(), String::new(), query)
}

fn fuzzy_subsequence(candidate: &str, query: &str) -> bool {
    let mut candidate = candidate.chars().flat_map(char::to_lowercase);
    for needle in query.chars().flat_map(char::to_lowercase) {
        if !candidate.by_ref().any(|candidate| candidate == needle) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn fallback_search_respects_ignore_files_but_includes_hidden_files() {
        let directory = tempdir().unwrap();
        fs::create_dir_all(directory.path().join("src")).unwrap();
        fs::create_dir_all(directory.path().join("src/generated")).unwrap();
        fs::write(directory.path().join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(directory.path().join(".env.example"), "KEY=").unwrap();
        fs::write(directory.path().join("src/generated/main.rs"), "").unwrap();
        fs::write(directory.path().join(".gitignore"), "src/generated/\n").unwrap();

        let all = search_with_walker(directory.path(), "");
        assert!(all.contains(&"src/main.rs".to_owned()));
        assert!(all.contains(&".env.example".to_owned()));
        assert!(all.contains(&".gitignore".to_owned()));
        assert!(!all.iter().any(|path| path.starts_with("src/generated/")));

        assert_eq!(
            search_with_walker(directory.path(), "src/mr"),
            vec!["src/main.rs"]
        );
    }

    #[test]
    fn exact_directory_keeps_the_typed_path_as_the_first_candidate() {
        let directory = tempdir().unwrap();
        fs::create_dir_all(directory.path().join("src/nested")).unwrap();

        assert_eq!(
            exact_directory(directory.path(), "src"),
            Some((directory.path().join("src"), "src/".to_owned()))
        );
        assert_eq!(
            exact_directory(directory.path(), "src/"),
            Some((directory.path().join("src"), "src/".to_owned()))
        );
    }

    #[test]
    fn regex_queries_escape_metacharacters() {
        assert_eq!(literal_regex("src/a+b"), "src/a\\+b");
        assert_eq!(subsequence_regex("a+b"), "a.*\\+.*b.*");
        assert_eq!(literal_regex(""), ".");
    }
}
