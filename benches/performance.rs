#![cfg(unix)]

use std::fs::{self, File};
use std::hint::black_box;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use tempfile::TempDir;
use zz::editor::Editor;
use zz::render::ViewState;

const STARTUP_RUNS: usize = 9;
const STARTUP_BUDGET: Duration = Duration::from_millis(100);
const CONTEXT_BUDGET: Duration = Duration::from_millis(350);
const HOME_FUZZY_BUDGET: Duration = Duration::from_millis(350);
const INPUT_BUDGET_NS_PER_KEY: f64 = 20_000.0;
const RENDER_BUDGET: Duration = Duration::from_millis(12);
const PASTE_1_MIB_BUDGET: Duration = Duration::from_millis(100);
const PASTE_10_MIB_BUDGET: Duration = Duration::from_millis(600);
const PASTE_64_MIB_BUDGET: Duration = Duration::from_secs(4);

fn main() {
    if cfg!(debug_assertions) {
        return;
    }
    let check = std::env::args().any(|argument| argument == "--check");
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_zz"));
    let fixture = Fixture::new();

    // Warm filesystem, dyld, SQLite, and code-signature caches before measuring steady state.
    measure_startup(&binary, &fixture).expect("warm startup");
    let startup = median_duration(
        (0..STARTUP_RUNS)
            .map(|_| measure_startup(&binary, &fixture).expect("measure startup"))
            .collect(),
    );
    let input_ns = measure_input_latency();
    let render = measure_render_latency();
    let context = median_duration(
        (0..5)
            .map(|_| measure_context_search(&binary, &fixture).expect("measure context search"))
            .collect(),
    );
    let home_fuzzy = median_duration(
        (0..5)
            .map(|_| measure_home_fuzzy_search(&binary, &fixture).expect("measure home fuzzy"))
            .collect(),
    );
    let paste_1 = measure_paste(1 << 20, 5);
    let paste_10 = measure_paste(10 << 20, 3);
    let paste_64 = measure_paste(64 << 20, 1);

    println!("zz performance (steady state)");
    println!("  startup median       {:>9.2} ms", millis(startup));
    println!("  input                 {:>9.0} ns/key", input_ns);
    println!("  render                {:>9.2} ms/frame", millis(render));
    println!("  scoped context        {:>9.2} ms", millis(context));
    println!("  home fuzzy directory  {:>9.2} ms", millis(home_fuzzy));
    print_paste("paste 1 MiB", 1, paste_1);
    print_paste("paste 10 MiB", 10, paste_10);
    print_paste("paste 64 MiB", 64, paste_64);

    if check {
        let failures = [
            budget("startup", startup, STARTUP_BUDGET),
            budget_ns("input", input_ns, INPUT_BUDGET_NS_PER_KEY),
            budget("render", render, RENDER_BUDGET),
            budget("context", context, CONTEXT_BUDGET),
            budget("home fuzzy", home_fuzzy, HOME_FUZZY_BUDGET),
            budget("paste 1 MiB", paste_1, PASTE_1_MIB_BUDGET),
            budget("paste 10 MiB", paste_10, PASTE_10_MIB_BUDGET),
            budget("paste 64 MiB", paste_64, PASTE_64_MIB_BUDGET),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        if !failures.is_empty() {
            eprintln!("performance budget failures:");
            for failure in failures {
                eprintln!("  {failure}");
            }
            std::process::exit(1);
        }
        println!("  budgets               PASS");
    }
}

fn measure_input_latency() -> f64 {
    let mut editor = Editor::new("");
    editor.handle_key(key('i'));
    let iterations = 50_000;
    let start = Instant::now();
    for _ in 0..iterations {
        editor.handle_key(key('a'));
    }
    let elapsed = start.elapsed();
    black_box(editor.buffer.len_chars());
    elapsed.as_nanos() as f64 / iterations as f64
}

fn measure_render_latency() -> Duration {
    let text = (0..10_000)
        .map(|line| format!("line {line:05} src/module_{line:05}.rs https://example.com/{line}\n"))
        .collect::<String>();
    let mut editor = Editor::new(&text);
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).expect("create test terminal");
    let mut view = ViewState::default();
    terminal
        .draw(|frame| view.render(frame, &editor))
        .expect("warm render");

    let iterations = 200;
    let start = Instant::now();
    for _ in 0..iterations {
        editor.handle_key(key('j'));
        terminal
            .draw(|frame| view.render(frame, &editor))
            .expect("render frame");
    }
    start.elapsed() / iterations
}

fn measure_paste(size: usize, runs: usize) -> Duration {
    let text = "x".repeat(size);
    median_duration(
        (0..runs)
            .map(|_| {
                let mut editor = Editor::new("");
                editor.handle_key(key('i'));
                let start = Instant::now();
                editor.handle_paste(black_box(&text));
                let elapsed = start.elapsed();
                black_box(editor.buffer.len_chars());
                elapsed
            })
            .collect(),
    )
}

fn measure_startup(binary: &Path, fixture: &Fixture) -> io::Result<Duration> {
    let started = Instant::now();
    let mut process = PtyProcess::spawn(binary, &fixture.input, fixture.root.path())?;
    process.wait_for_ready(Duration::from_secs(3))?;
    let elapsed = started.elapsed();
    process.kill()?;
    Ok(elapsed)
}

fn measure_context_search(binary: &Path, fixture: &Fixture) -> io::Result<Duration> {
    let mut process = PtyProcess::spawn(binary, &fixture.input, fixture.root.path())?;
    process.wait_for_ready(Duration::from_secs(3))?;
    let started = Instant::now();
    process.send(b"i@src/mr")?;
    process.wait_for_output(b"src/main.rs", Duration::from_secs(2))?;
    let elapsed = started.elapsed();
    process.kill()?;
    Ok(elapsed)
}

fn measure_home_fuzzy_search(binary: &Path, fixture: &Fixture) -> io::Result<Duration> {
    let mut process = PtyProcess::spawn(binary, &fixture.input, fixture.root.path())?;
    process.wait_for_ready(Duration::from_secs(3))?;
    let started = Instant::now();
    process.send(b"i@crncysdk")?;
    process.wait_for_output(b"src/bro/currency-sdk/", Duration::from_secs(2))?;
    let elapsed = started.elapsed();
    process.kill()?;
    Ok(elapsed)
}

fn print_paste(label: &str, mib: usize, elapsed: Duration) {
    let throughput = mib as f64 / elapsed.as_secs_f64();
    println!(
        "  {label:<20} {:>9.2} ms  {:>8.1} MiB/s",
        millis(elapsed),
        throughput
    );
}

fn budget(name: &str, actual: Duration, limit: Duration) -> Option<String> {
    (actual > limit).then(|| {
        format!(
            "{name}: {:.2} ms exceeds {:.2} ms",
            millis(actual),
            millis(limit)
        )
    })
}

fn budget_ns(name: &str, actual: f64, limit: f64) -> Option<String> {
    (actual > limit).then(|| format!("{name}: {actual:.0} ns exceeds {limit:.0} ns"))
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn median_duration(mut values: Vec<Duration>) -> Duration {
    values.sort_unstable();
    values[values.len() / 2]
}

fn key(ch: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)
}

struct Fixture {
    root: TempDir,
    input: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("create benchmark workspace");
        let input = root.path().join("prompt.txt");
        fs::write(&input, "benchmark seed\n").expect("write benchmark seed");
        fs::create_dir_all(root.path().join("src/data")).expect("create source tree");
        fs::create_dir_all(root.path().join("src/bro/currency-sdk"))
            .expect("create fuzzy directory");
        fs::write(root.path().join("src/main.rs"), "fn main() {}\n").expect("write target file");
        for index in 0..2_000 {
            fs::write(
                root.path().join(format!("src/data/file_{index:04}.txt")),
                "generated\n",
            )
            .expect("write generated benchmark file");
        }
        Self { root, input }
    }
}

struct PtyProcess {
    child: Child,
    master: File,
    output: Vec<u8>,
    cursor_reported: bool,
}

impl PtyProcess {
    fn spawn(binary: &Path, input: &Path, root: &Path) -> io::Result<Self> {
        let (master, slave) = open_pty(24, 120)?;
        let stdin = slave.try_clone()?;
        let stdout = slave.try_clone()?;
        let child = Command::new(binary)
            .arg(input)
            .current_dir(root)
            .env("HOME", root)
            .env("TERM", "xterm-256color")
            .env_remove("ZZ_CURSOR_BYTE")
            .stdin(Stdio::from(stdin))
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(slave))
            .spawn()?;
        Ok(Self {
            child,
            master,
            output: Vec::new(),
            cursor_reported: false,
        })
    }

    fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.master.write_all(bytes)?;
        self.master.flush()
    }

    fn wait_for_ready(&mut self, timeout: Duration) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            self.read_output(Duration::from_millis(20))?;
            if !self.cursor_reported && contains(&self.output, b"\x1b[6n") {
                self.send(b"\x1b[1;1R")?;
                self.cursor_reported = true;
            }
            if self.cursor_reported
                && contains(&self.output, b"\x1b[?1049h")
                && contains(&self.output, b"\x1b[2J")
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "zz startup timed out",
                ));
            }
        }
    }

    fn wait_for_output(&mut self, needle: &[u8], timeout: Duration) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            self.read_output(Duration::from_millis(20))?;
            if contains(&self.output, needle) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("output did not contain {}", String::from_utf8_lossy(needle)),
                ));
            }
        }
    }

    fn read_output(&mut self, timeout: Duration) -> io::Result<()> {
        let mut descriptor = libc::pollfd {
            fd: self.master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        // SAFETY: descriptor points to one initialized pollfd for this call.
        let ready = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
        if ready <= 0 || descriptor.revents & libc::POLLIN == 0 {
            return Ok(());
        }
        let mut buffer = [0_u8; 16_384];
        match self.master.read(&mut buffer) {
            Ok(0) => Ok(()),
            Ok(read) => {
                self.output.extend_from_slice(&buffer[..read]);
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn kill(&mut self) -> io::Result<()> {
        if self.child.try_wait()?.is_none() {
            self.child.kill()?;
        }
        self.child.wait()?;
        Ok(())
    }
}

impl Drop for PtyProcess {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

fn open_pty(rows: u16, columns: u16) -> io::Result<(File, File)> {
    let mut master_fd: RawFd = -1;
    let mut slave_fd: RawFd = -1;
    let mut dimensions = libc::winsize {
        ws_row: rows,
        ws_col: columns,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: openpty initializes both descriptors and reads a valid winsize.
    let result = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut dimensions,
        )
    };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful openpty returned owned descriptors.
    let master = unsafe { File::from_raw_fd(master_fd) };
    // SAFETY: successful openpty returned owned descriptors.
    let slave = unsafe { File::from_raw_fd(slave_fd) };
    Ok((master, slave))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
