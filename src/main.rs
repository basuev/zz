mod buffer;
mod context_index;
mod editor;
mod render;
mod storage;

use std::env;
use std::fs;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use context_index::{SearchRequest, SearchResult, spawn_workspace_search};
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use editor::{Editor, Outcome};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use render::{ViewState, cursor_style};
use storage::{DraftStore, HistoryStore, replace_input_file};

const AUTOSAVE_DELAY: Duration = Duration::from_millis(200);
const EVENT_POLL: Duration = Duration::from_millis(50);
const CONTEXT_SEARCH_DEBOUNCE: Duration = Duration::from_millis(35);

fn main() -> ExitCode {
    match try_main() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("zz: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn try_main() -> Result<ExitCode> {
    let target = parse_args()?;
    let input = match target {
        InputTarget::Standalone => None,
        InputTarget::File(path) => {
            let path = path
                .canonicalize()
                .with_context(|| format!("could not resolve input file {}", path.display()))?;
            if !path.is_file() {
                bail!("input is not a regular file: {}", path.display());
            }
            Some(path)
        }
    };

    let seed = match &input {
        Some(path) => fs::read_to_string(path)
            .with_context(|| format!("input file is not valid UTF-8: {}", path.display()))?,
        None => String::new(),
    };
    let workspace = env::current_dir().context("could not determine the current workspace")?;
    let drafts = DraftStore::new(&workspace, &seed)?;
    let history = HistoryStore::new()?;

    let mut editor = Editor::new(&seed);
    editor.enable_context_search();
    editor.set_history(
        history.workspace_history(&workspace, 500)?,
        history.global_history(1_000)?,
    );
    if let Some(recovered) = drafts.recover()? {
        editor.replace_text(&recovered.text, recovered.cursor);
    }

    let outcome = run_editor(&mut editor, &drafts, &workspace)?;
    match outcome {
        Outcome::Accept => {
            let text = editor.buffer.as_string();
            if let Some(input) = input {
                replace_input_file(&input, &text)?;
            } else {
                print!("{text}");
            }
            drafts.clear()?;
            if let Err(error) = history.save(&workspace, &text) {
                eprintln!("zz: could not save history: {error:#}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Outcome::Cancel => {
            drafts.save(&editor.buffer.as_string(), editor.buffer.cursor())?;
            Ok(ExitCode::from(1))
        }
    }
}

#[derive(Debug)]
enum InputTarget {
    Standalone,
    File(PathBuf),
}

fn parse_args() -> Result<InputTarget> {
    let mut args = env::args_os();
    let program = args.next().unwrap_or_default();
    let Some(first) = args.next() else {
        return Ok(InputTarget::Standalone);
    };
    if first == "--help" || first == "-h" {
        println!(
            "Usage: {} [prompt-file]\n\nWithout a file, the accepted prompt is written to stdout.\nZZ accepts the prompt. ZQ cancels without modifying the input file.\nCtrl+P opens prompt history. Type @ in Insert mode to attach a workspace file.",
            Path::new(&program).display()
        );
        std::process::exit(0);
    }
    if args.next().is_some() {
        bail!("at most one prompt file is supported");
    }
    Ok(InputTarget::File(PathBuf::from(first)))
}

fn run_editor(editor: &mut Editor, drafts: &DraftStore, workspace: &Path) -> Result<Outcome> {
    let mut terminal = ManagedTerminal::new()?;
    let mut view = ViewState::default();
    let mut render_required = true;
    let mut observed_revision = editor.buffer.revision();
    let mut saved_revision = observed_revision;
    let mut changed_at = Instant::now();
    let mut context_search = None;
    let mut pending_context_search: Option<(SearchRequest, Instant)> = None;

    loop {
        if let Some((generation, query)) = editor.take_context_search_request() {
            pending_context_search = Some((SearchRequest { generation, query }, Instant::now()));
        }
        let search_ready = pending_context_search
            .as_ref()
            .is_some_and(|(request, queued_at)| {
                request.query.is_empty() || queued_at.elapsed() >= CONTEXT_SEARCH_DEBOUNCE
            });
        if search_ready {
            if context_search.is_none() {
                context_search = Some(spawn_workspace_search(workspace.to_path_buf()));
            }
            let (request, _) = pending_context_search
                .take()
                .expect("pending context search is present");
            if context_search
                .as_ref()
                .is_some_and(|(requests, _)| requests.send(request).is_err())
            {
                context_search = None;
                editor.fail_context_search();
                render_required = true;
            }
        }

        let mut search_disconnected = false;
        if let Some((_, results)) = &context_search {
            loop {
                match results.try_recv() {
                    Ok(SearchResult { generation, files }) => {
                        if editor.apply_context_search_result(generation, files) {
                            render_required = true;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        search_disconnected = true;
                        break;
                    }
                }
            }
        }
        if search_disconnected {
            context_search = None;
            editor.fail_context_search();
            render_required = true;
        }
        if render_required {
            execute!(terminal.terminal.backend_mut(), cursor_style(editor.mode()))?;
            terminal.terminal.draw(|frame| view.render(frame, editor))?;
            render_required = false;
        }

        if let Some(outcome) = editor.outcome() {
            return Ok(outcome);
        }

        if event::poll(EVENT_POLL)? {
            match event::read()? {
                Event::Key(key) => editor.handle_key(key),
                Event::Paste(text) => editor.handle_paste(&text),
                Event::Resize(_, _) => {}
                Event::FocusGained | Event::FocusLost | Event::Mouse(_) => continue,
            }
            render_required = true;
        }

        let revision = editor.buffer.revision();
        if revision != observed_revision {
            observed_revision = revision;
            changed_at = Instant::now();
        }
        if revision != saved_revision && changed_at.elapsed() >= AUTOSAVE_DELAY {
            drafts.save(&editor.buffer.as_string(), editor.buffer.cursor())?;
            saved_revision = revision;
        }
    }
}

struct ManagedTerminal {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl ManagedTerminal {
    fn new() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(
            stdout,
            EnterAlternateScreen,
            EnableBracketedPaste,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
                    | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES,
            )
        ) {
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;
        Ok(Self { terminal })
    }
}

impl Drop for ManagedTerminal {
    fn drop(&mut self) {
        let _ = self.terminal.show_cursor();
        let _ = execute!(
            self.terminal.backend_mut(),
            SetCursorStyle::DefaultUserShape,
            PopKeyboardEnhancementFlags,
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
    }
}
