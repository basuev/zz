#![cfg(unix)]

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);
const INPUT_SETTLE: Duration = Duration::from_millis(100);

struct Fixture {
    root: TempDir,
    input: PathBuf,
}

impl Fixture {
    fn new(seed: &str) -> Self {
        let root = tempfile::tempdir().expect("create test directory");
        let input = root.path().join("prompt.txt");
        fs::write(&input, seed).expect("write prompt seed");
        Self { root, input }
    }

    fn spawn(&self) -> PtyChild {
        PtyChild::spawn(&self.input, self.root.path())
    }

    fn content(&self) -> String {
        fs::read_to_string(&self.input).expect("read prompt")
    }
}

struct PtyChild {
    child: Option<Child>,
    master: File,
    output: Vec<u8>,
}

impl PtyChild {
    fn spawn(input: &Path, root: &Path) -> Self {
        let (master, slave) = open_pty(24, 80).expect("open PTY");
        let stdin = slave.try_clone().expect("clone PTY slave for stdin");
        let stdout = slave.try_clone().expect("clone PTY slave for stdout");

        let child = Command::new(env!("CARGO_BIN_EXE_zz"))
            .arg(input)
            .current_dir(root)
            .env("HOME", root)
            .env("TERM", "xterm-256color")
            .stdin(Stdio::from(stdin))
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(slave))
            .spawn()
            .expect("spawn zz");

        let mut process = Self {
            child: Some(child),
            master,
            output: Vec::new(),
        };
        process.wait_until_ready();
        process
    }

    fn send(&mut self, bytes: &[u8]) {
        self.master.write_all(bytes).expect("write PTY input");
        self.master.flush().expect("flush PTY input");
    }

    fn enter_normal(&mut self) {
        self.send(b"\x1b");
        thread::sleep(INPUT_SETTLE);
    }

    fn finish(mut self) -> (ExitStatus, Vec<u8>) {
        let deadline = Instant::now() + EXIT_TIMEOUT;
        loop {
            let status = self
                .child
                .as_mut()
                .expect("child is present")
                .try_wait()
                .expect("poll zz process");
            if let Some(status) = status {
                self.child.take();
                self.drain_output(Duration::from_millis(100));
                return (status, std::mem::take(&mut self.output));
            }
            if Instant::now() >= deadline {
                let rendered = String::from_utf8_lossy(&self.output);
                panic!("zz did not exit; terminal output: {rendered:?}");
            }
            self.read_output(Duration::from_millis(20));
        }
    }

    fn kill(mut self) {
        let mut child = self.child.take().expect("child is present");
        child.kill().expect("kill zz");
        child.wait().expect("wait for killed zz");
    }

    fn wait_until_ready(&mut self) {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        let mut cursor_position_reported = false;
        loop {
            if !cursor_position_reported && contains(&self.output, b"\x1b[6n") {
                self.send(b"\x1b[1;1R");
                cursor_position_reported = true;
            }
            if cursor_position_reported
                && contains(&self.output, b"\x1b[?1049h")
                && contains(&self.output, b"\x1b[2J")
            {
                return;
            }
            if Instant::now() >= deadline {
                let rendered = String::from_utf8_lossy(&self.output);
                panic!("zz did not finish terminal setup; terminal output: {rendered:?}");
            }
            self.read_output(Duration::from_millis(50));
            if let Some(status) = self
                .child
                .as_mut()
                .expect("child is present")
                .try_wait()
                .expect("poll zz startup")
            {
                let rendered = String::from_utf8_lossy(&self.output);
                panic!("zz exited during startup with {status}; terminal output: {rendered:?}");
            }
        }
    }

    fn drain_output(&mut self, budget: Duration) {
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline {
            if !self.read_output(Duration::from_millis(10)) {
                break;
            }
        }
    }

    fn read_output(&mut self, timeout: Duration) -> bool {
        let mut descriptor = libc::pollfd {
            fd: self.master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        // SAFETY: descriptor points to one initialized pollfd for the duration of the call.
        let ready = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
        if ready <= 0 || descriptor.revents & libc::POLLIN == 0 {
            return false;
        }

        let mut buffer = [0_u8; 8192];
        match self.master.read(&mut buffer) {
            Ok(0) => false,
            Ok(read) => {
                self.output.extend_from_slice(&buffer[..read]);
                true
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => false,
            Err(error) => panic!("read PTY output: {error}"),
        }
    }
}

impl Drop for PtyChild {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
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

    // SAFETY: openpty initializes both file descriptors; the optional name and termios pointers
    // are null, and dimensions points to a valid winsize value.
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

    // SAFETY: successful openpty returned two owned file descriptors.
    let master = unsafe { File::from_raw_fd(master_fd) };
    // SAFETY: successful openpty returned two owned file descriptors.
    let slave = unsafe { File::from_raw_fd(slave_fd) };
    Ok((master, slave))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[test]
fn zz_accepts_and_atomically_replaces_the_input_file() {
    let fixture = Fixture::new("");
    let mut process = fixture.spawn();

    process.send(b"ihello from pty");
    process.enter_normal();
    process.send(b"ZZ");

    let (status, output) = process.finish();
    assert!(
        status.success(),
        "terminal output: {:?}",
        String::from_utf8_lossy(&output)
    );
    assert_eq!(fixture.content(), "hello from pty");
}

#[test]
fn zq_cancels_without_changing_the_input_file() {
    let fixture = Fixture::new("original");
    let mut process = fixture.spawn();

    process.send(b"A changed");
    process.enter_normal();
    process.send(b"ZQ");

    let (status, output) = process.finish();
    assert_eq!(
        status.code(),
        Some(1),
        "terminal output: {:?}",
        String::from_utf8_lossy(&output)
    );
    assert_eq!(fixture.content(), "original");
}

#[test]
fn bracketed_paste_keeps_multiline_unicode_literal() {
    let fixture = Fixture::new("");
    let mut process = fixture.spawn();
    let text = "first line\n@context/file.rs:10-20\nтаблица 🙂\tend";

    process.send(b"\x1b[200~");
    process.send(text.as_bytes());
    process.send(b"\x1b[201~");
    thread::sleep(INPUT_SETTLE);
    process.send(b"ZZ");

    let (status, output) = process.finish();
    assert!(
        status.success(),
        "terminal output: {:?}",
        String::from_utf8_lossy(&output)
    );
    assert_eq!(fixture.content(), text);
}

#[test]
fn at_picker_inserts_an_ignored_aware_workspace_file_reference() {
    let fixture = Fixture::new("");
    fs::create_dir_all(fixture.root.path().join("src")).expect("create source directory");
    fs::create_dir_all(fixture.root.path().join("src/generated"))
        .expect("create ignored directory");
    fs::write(fixture.root.path().join("src/main.rs"), "fn main() {}").expect("write source file");
    fs::write(fixture.root.path().join("src/generated/main.rs"), "")
        .expect("write ignored source file");
    fs::write(fixture.root.path().join(".gitignore"), "src/generated/\n")
        .expect("write ignore file");
    let mut process = fixture.spawn();

    process.send(b"i@src/mr");
    thread::sleep(Duration::from_millis(300));
    process.send(b"\r");
    process.enter_normal();
    process.send(b"ZZ");

    let (status, output) = process.finish();
    assert!(
        status.success(),
        "terminal output: {:?}",
        String::from_utf8_lossy(&output)
    );
    assert_eq!(fixture.content(), "@src/main.rs ");
}

#[test]
fn accepted_history_can_be_recalled_through_the_terminal() {
    let fixture = Fixture::new("");
    let mut first = fixture.spawn();
    first.send(b"ihistorical prompt");
    first.enter_normal();
    first.send(b"ZZ");
    let (status, output) = first.finish();
    assert!(
        status.success(),
        "terminal output: {:?}",
        String::from_utf8_lossy(&output)
    );

    fs::write(&fixture.input, "current draft").expect("reset prompt seed");
    let mut second = fixture.spawn();
    second.send(b"\x10\rZZ");
    let (status, output) = second.finish();
    assert!(
        status.success(),
        "terminal output: {:?}",
        String::from_utf8_lossy(&output)
    );
    assert_eq!(fixture.content(), "historical prompt");
}

#[test]
fn dot_repeats_the_last_change_through_the_terminal() {
    let fixture = Fixture::new("one two");
    let mut process = fixture.spawn();

    process.send(b"daw.ZZ");

    let (status, output) = process.finish();
    assert!(
        status.success(),
        "terminal output: {:?}",
        String::from_utf8_lossy(&output)
    );
    assert_eq!(fixture.content(), "");
}

#[test]
fn text_objects_work_through_the_terminal() {
    let fixture = Fixture::new("one two");
    let mut process = fixture.spawn();

    process.send(b"dawZZ");

    let (status, output) = process.finish();
    assert!(
        status.success(),
        "terminal output: {:?}",
        String::from_utf8_lossy(&output)
    );
    assert_eq!(fixture.content(), "two");
}

#[test]
fn character_find_and_repeat_work_through_the_terminal() {
    let fixture = Fixture::new("one:two:three");
    let mut process = fixture.spawn();

    process.send(b"f:x;xZZ");

    let (status, output) = process.finish();
    assert!(
        status.success(),
        "terminal output: {:?}",
        String::from_utf8_lossy(&output)
    );
    assert_eq!(fixture.content(), "onetwothree");
}

#[test]
fn slash_search_moves_to_the_next_match() {
    let fixture = Fixture::new("first target third target");
    let mut process = fixture.spawn();

    process.send(b"/target\rxZZ");

    let (status, output) = process.finish();
    assert!(
        status.success(),
        "terminal output: {:?}",
        String::from_utf8_lossy(&output)
    );
    assert_eq!(fixture.content(), "first arget third target");
}

#[test]
fn legacy_cyrillic_keys_control_normal_mode_but_insert_unicode_text() {
    let fixture = Fixture::new("");
    let mut process = fixture.spawn();

    process.send("штекст".as_bytes());
    process.enter_normal();
    process.send("ЯЯ".as_bytes());

    let (status, output) = process.finish();
    assert!(
        status.success(),
        "terminal output: {:?}",
        String::from_utf8_lossy(&output)
    );
    assert_eq!(fixture.content(), "текст");
}

#[test]
fn kitty_base_layout_keys_control_commands_without_changing_inserted_text() {
    let fixture = Fixture::new("");
    let mut process = fixture.spawn();

    // Physical i on a Russian layout enters Insert mode.
    process.send(b"\x1b[1096:1064:105u");
    // Physical a inserts the logical Russian character while Insert mode is active.
    process.send(b"\x1b[1092:1060:97u");
    process.send("раза".as_bytes());
    process.send(b"\x1b[27u");
    thread::sleep(INPUT_SETTLE);
    // Shift+Z twice accepts, independently of the active layout.
    process.send(b"\x1b[1103:1071:122;2u\x1b[1103:1071:122;2u");

    let (status, output) = process.finish();
    assert!(
        status.success(),
        "terminal output: {:?}",
        String::from_utf8_lossy(&output)
    );
    assert_eq!(fixture.content(), "фраза");
}

#[test]
fn autosaved_text_is_recovered_after_forced_termination() {
    let fixture = Fixture::new("");
    let mut interrupted = fixture.spawn();

    interrupted.send(b"irecovered draft");
    thread::sleep(Duration::from_millis(500));
    interrupted.kill();

    let mut recovered = fixture.spawn();
    recovered.send(b"ZZ");
    let (status, output) = recovered.finish();

    assert!(
        status.success(),
        "terminal output: {:?}",
        String::from_utf8_lossy(&output)
    );
    assert_eq!(fixture.content(), "recovered draft");
}
