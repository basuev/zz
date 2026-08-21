use std::collections::VecDeque;
use std::env;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use ignore::WalkBuilder;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

const MAX_RESULTS: usize = 100;
const MAX_CANDIDATES: usize = 2_000;
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
                let mut files = rank_candidates(walk_entries(&directory, &display, ""), "");
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

            if let Some(display) = exact_file(&workspace, &request.query) {
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
                let mut files = search_workspace(fd.as_deref(), &workspace, &request.query);
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

            let shallow =
                if !request.query.is_empty() && !query_is_scoped(&workspace, &request.query) {
                    fuzzy_shallow_entries(&workspace, &request.query)
                } else {
                    Vec::new()
                };
            if !shallow.is_empty()
                && result_sender
                    .send(SearchResult {
                        generation: request.generation,
                        files: shallow.clone(),
                        complete: false,
                    })
                    .is_err()
            {
                break;
            }

            let mut files = search_workspace(fd.as_deref(), &workspace, &request.query);
            files.extend(shallow);
            files = rank_candidates(files, search_needle(&workspace, &request.query));
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

fn search_workspace(fd: Option<&Path>, workspace: &Path, query: &str) -> Vec<String> {
    if query.is_empty() {
        return shallow_workspace_entries(workspace);
    }
    let candidates = if query_is_scoped(workspace, query) || is_home_directory(workspace) {
        search_with_walker(workspace, query)
    } else {
        fd.and_then(|fd| search_with_fd(fd, workspace, query))
            .unwrap_or_else(|| search_with_walker(workspace, query))
    };
    rank_candidates(candidates, search_needle(workspace, query))
}

fn shallow_workspace_entries(workspace: &Path) -> Vec<String> {
    let mut entries = shallow_workspace_entries_unbounded(workspace);
    entries.sort_unstable();
    entries.truncate(MAX_RESULTS);
    entries
}

fn fuzzy_shallow_entries(workspace: &Path, query: &str) -> Vec<String> {
    rank_candidates(shallow_workspace_entries_unbounded(workspace), query)
}

fn shallow_workspace_entries_unbounded(workspace: &Path) -> Vec<String> {
    let mut builder = WalkBuilder::new(workspace);
    builder
        .max_depth(Some(1))
        .hidden(false)
        .parents(true)
        .ignore(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .sort_by_file_path(compare_search_paths);
    builder
        .build()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let kind = entry.file_type()?;
            let relative = entry.path().strip_prefix(workspace).ok()?.to_str()?;
            if relative.is_empty() || relative.chars().any(char::is_control) {
                return None;
            }
            let mut path = relative.replace(std::path::MAIN_SEPARATOR, "/");
            if kind.is_dir() {
                path.push('/');
            } else if !kind.is_file() && !kind.is_symlink() {
                return None;
            }
            Some(path)
        })
        .collect()
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
    let max_results = MAX_CANDIDATES.to_string();
    let mut command = Command::new(fd);
    command.args([
        "--base-directory",
        search_root.to_str()?,
        "--max-results",
        &max_results,
        "--type",
        "f",
        "--type",
        "d",
        "--hidden",
        "--ignore-case",
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
    command.arg(subsequence_regex(&compact_name(needle)));

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
        .map(|path| {
            let path = path.replace('\\', "/");
            let suffix = if search_root.join(&path).is_dir() {
                "/"
            } else {
                ""
            };
            format!("{display_prefix}{path}{suffix}")
        })
        .collect();
    files.sort_unstable();
    files.dedup();
    Some(files)
}

fn subsequence_regex(query: &str) -> String {
    if query.is_empty() {
        return ".".to_owned();
    }
    let mut pattern = String::new();
    for ch in query.chars() {
        if ".+*?()|[]{}^$\\".contains(ch) {
            pattern.push('\\');
        }
        pattern.push(ch);
        pattern.push_str(".*");
    }
    pattern
}

fn normalize_query(query: &str) -> &str {
    query.strip_prefix("./").unwrap_or(query)
}

fn search_with_walker(workspace: &Path, query: &str) -> Vec<String> {
    let query = normalize_query(query);
    let (search_root, display_prefix, needle) = scoped_search(workspace, query);
    if is_home_directory(&search_root) {
        search_home_directories(&search_root, &display_prefix, needle)
    } else {
        walk_entries(&search_root, &display_prefix, needle)
    }
}

fn search_home_directories(home: &Path, display_prefix: &str, needle: &str) -> Vec<String> {
    const SOURCE_ROOTS: [&str; 9] = [
        "src",
        "source",
        "code",
        "projects",
        "project",
        "repos",
        "workspace",
        "work",
        "dev",
    ];
    const MAX_DEPTH: usize = 4;

    let started = Instant::now();
    let mut visited = 0;
    let mut queue = VecDeque::new();
    for root in SOURCE_ROOTS {
        let path = home.join(root);
        if path.is_dir() {
            queue.push_back((path, format!("{root}/"), 0));
        }
    }

    let mut directories = Vec::new();
    while let Some((directory, relative, depth)) = queue.pop_front() {
        if visited >= MAX_FALLBACK_ENTRIES || started.elapsed() >= SEARCH_BUDGET {
            break;
        }
        if depth > 0 && directory.join(".git").is_dir() {
            continue;
        }
        let Ok(entries) = directory.read_dir() else {
            continue;
        };
        let mut children = entries.filter_map(Result::ok).collect::<Vec<_>>();
        children.sort_unstable_by_key(|entry| entry.file_name());
        for entry in children {
            visited += 1;
            if visited >= MAX_FALLBACK_ENTRIES || started.elapsed() >= SEARCH_BUDGET {
                break;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if !kind.is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                continue;
            };
            if name.starts_with('.') || search_path_bucket(entry.path().as_path()) >= 3 {
                continue;
            }
            let child_relative = format!("{relative}{name}/");
            let display = format!("{display_prefix}{child_relative}");
            if fuzzy_subsequence(&display, needle) {
                directories.push(display);
            }
            if depth < MAX_DEPTH {
                queue.push_back((entry.path(), child_relative, depth + 1));
            }
        }
        if !directories.is_empty() {
            break;
        }
    }
    directories
}

fn walk_entries(search_root: &Path, display_prefix: &str, needle: &str) -> Vec<String> {
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
        .filter_entry(|entry| entry.file_name() != ".git")
        .sort_by_file_path(compare_search_paths);

    let started = Instant::now();
    let mut entries = Vec::new();
    for (visited, result) in builder.build().enumerate() {
        if visited >= MAX_FALLBACK_ENTRIES || started.elapsed() >= SEARCH_BUDGET {
            break;
        }
        let Ok(entry) = result else {
            continue;
        };
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() && !file_type.is_dir() && !file_type.is_symlink() {
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
        let mut relative = relative.replace(std::path::MAIN_SEPARATOR, "/");
        if file_type.is_dir() {
            relative.push('/');
        }
        let display = format!("{display_prefix}{relative}");
        if fuzzy_subsequence(&display, needle) {
            entries.push(display);
        }
    }
    entries
}

fn search_needle<'a>(workspace: &Path, query: &'a str) -> &'a str {
    scoped_search(workspace, normalize_query(query)).2
}

fn rank_candidates(mut candidates: Vec<String>, query: &str) -> Vec<String> {
    candidates.sort_unstable();
    candidates.dedup();
    if query.is_empty() {
        candidates.truncate(MAX_RESULTS);
        return candidates;
    }

    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let compact_query = compact_name(query);
    let compact_pattern = Pattern::parse(&compact_query, CaseMatching::Smart, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let mut buffer = Vec::new();
    let mut ranked: Vec<(String, u32)> = candidates
        .into_iter()
        .filter_map(|candidate| {
            let trimmed = candidate.trim_end_matches('/');
            let basename = trimmed.rsplit('/').next().unwrap_or(trimmed);
            let mut score = pattern
                .score(Utf32Str::new(&candidate, &mut buffer), &mut matcher)
                .unwrap_or_default();
            if let Some(basename_score) =
                pattern.score(Utf32Str::new(basename, &mut buffer), &mut matcher)
            {
                score = score.max(basename_score.saturating_add(1_000));
            }
            let compact = compact_name(basename);
            if let Some(compact_score) =
                compact_pattern.score(Utf32Str::new(&compact, &mut buffer), &mut matcher)
            {
                score = score.max(compact_score.saturating_add(1_200));
            }
            if score == 0 {
                return None;
            }
            if candidate.ends_with('/') {
                score = score.saturating_add(50);
            }
            Some((candidate, score))
        })
        .collect();
    ranked.sort_unstable_by(|(left_path, left_score), (right_path, right_score)| {
        right_score
            .cmp(left_score)
            .then_with(|| left_path.len().cmp(&right_path.len()))
            .then_with(|| left_path.cmp(right_path))
    });
    ranked.truncate(MAX_RESULTS);
    ranked.into_iter().map(|(path, _)| path).collect()
}

fn compact_name(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !is_fuzzy_separator(*ch))
        .collect()
}

fn is_fuzzy_separator(ch: char) -> bool {
    matches!(ch, '-' | '_' | ' ' | '/' | '.')
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

fn exact_file(workspace: &Path, query: &str) -> Option<String> {
    if query.is_empty() || query.ends_with('/') {
        return None;
    }
    let path = if query == "~" {
        return None;
    } else if let Some(relative) = query.strip_prefix("~/") {
        env::var_os("HOME").map(PathBuf::from)?.join(relative)
    } else {
        workspace.join(query)
    };
    path.is_file().then(|| query.to_owned())
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
    (root, display_root.to_owned(), scoped_query)
}

fn is_home_directory(path: &Path) -> bool {
    env::var_os("HOME").is_some_and(|home| path == Path::new(&home))
}

fn compare_search_paths(left: &Path, right: &Path) -> std::cmp::Ordering {
    search_path_bucket(left)
        .cmp(&search_path_bucket(right))
        .then_with(|| left.file_name().cmp(&right.file_name()))
}

fn search_path_bucket(path: &Path) -> u8 {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    match name.to_ascii_lowercase().as_str() {
        "src" | "source" | "code" | "projects" | "project" | "repos" | "workspace" | "work"
        | "dev" => 0,
        "library" | "node_modules" | "target" | ".cache" | ".nvim" | ".local" => 3,
        _ if name.starts_with('.') => 2,
        _ => 1,
    }
}

fn fuzzy_subsequence(candidate: &str, query: &str) -> bool {
    let mut candidate = candidate
        .chars()
        .filter(|ch| !is_fuzzy_separator(*ch))
        .flat_map(char::to_lowercase);
    for needle in query
        .chars()
        .filter(|ch| !is_fuzzy_separator(*ch))
        .flat_map(char::to_lowercase)
    {
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
    fn fuzzy_search_finds_and_ranks_directories_by_compact_subsequence() {
        let directory = tempdir().unwrap();
        fs::create_dir_all(directory.path().join("currency-sdk/src")).unwrap();
        fs::create_dir_all(directory.path().join("current-session-data")).unwrap();
        fs::create_dir_all(directory.path().join("currency-sdk-ignored")).unwrap();
        fs::write(directory.path().join("currency-sdk/src/lib.rs"), "").unwrap();
        fs::write(
            directory.path().join(".gitignore"),
            "currency-sdk-ignored/\n",
        )
        .unwrap();

        let ranked = rank_candidates(search_with_walker(directory.path(), "crncysdk"), "crncysdk");
        assert_eq!(ranked.first().map(String::as_str), Some("currency-sdk/"));
        assert!(ranked.contains(&"currency-sdk/src/lib.rs".to_owned()));

        let shallow = fuzzy_shallow_entries(directory.path(), "crncysdk");
        assert_eq!(shallow.first().map(String::as_str), Some("currency-sdk/"));
        assert!(!shallow.iter().any(|path| path.contains("ignored")));
    }

    #[test]
    fn ranking_is_not_biased_toward_the_first_candidates() {
        let mut candidates = (0..500)
            .map(|index| format!("aaa/noise-{index:04}.txt"))
            .collect::<Vec<_>>();
        candidates.push("projects/currency-sdk/".to_owned());

        assert_eq!(
            rank_candidates(candidates, "crncysdk")
                .first()
                .map(String::as_str),
            Some("projects/currency-sdk/")
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
    fn exact_file_keeps_the_typed_path() {
        let directory = tempdir().unwrap();
        fs::create_dir_all(directory.path().join("src")).unwrap();
        fs::write(directory.path().join("src/main.rs"), "").unwrap();

        assert_eq!(
            exact_file(directory.path(), "src/main.rs"),
            Some("src/main.rs".to_owned())
        );
        assert_eq!(exact_file(directory.path(), "src/missing.rs"), None);
    }

    #[test]
    fn regex_queries_escape_metacharacters() {
        assert_eq!(subsequence_regex("a+b"), "a.*\\+.*b.*");
        assert_eq!(subsequence_regex(""), ".");
    }
}
