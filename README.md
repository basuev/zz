# zz

A modal terminal editor for coding-agent prompts.

Write long, precise prompts in a focused terminal UI. Reuse local prompt history, recover drafts after a crash, and add exact file or line references with `@`. Run `zz` standalone or use it as `$VISUAL` in tools that support an external editor.

![Writing a prompt and adding an exact file range in zz](assets/demo.gif)

> The project is preparing its first public release. Build from source for now; prebuilt binaries and package-manager installs are next.

## Why zz

Inline prompt fields work for short requests. Larger coding tasks need editing, navigation, history, and a reliable way to point an agent at project context. General-purpose editors can do most of this, but they do not provide prompt-specific accept, cancel, recovery, history, and context workflows out of the box.

`zz` stays deliberately small:

- one soft-wrapped text buffer;
- modal editing with Vim, shell, and macOS shortcuts;
- transactional accept and cancel;
- local autosave and crash recovery;
- workspace and global prompt history;
- fuzzy project-file search with exact line ranges;
- no accounts, network client, or telemetry.

## Install from source

A Rust toolchain is currently required.

```sh
cargo install --git https://github.com/basuev/zz --locked
```

Or build a local checkout:

```sh
git clone https://github.com/basuev/zz.git
cd zz
cargo build --release
```

## Quick start

Run `zz` without arguments to write the accepted prompt to stdout:

```sh
zz > prompt.txt
```

Pass a file to use `zz` as an external editor:

```sh
zz path/to/prompt.txt
```

Set it as `$VISUAL` for a tool that supports external editors:

```sh
VISUAL=zz your-agent-command
```

In fish:

```fish
set -gx VISUAL zz
your-agent-command
```

Compatibility depends on the external-editor contract of the calling tool. Agent-specific workflows will be documented only after they are tested against released versions.

## The prompt workflow

- Press `i` to enter Insert mode.
- Write the prompt and use normal modal motions to edit it.
- Type `@` at a token boundary to search workspace files.
- Press `Enter` to preview a file or `Tab` to add the whole-file reference.
- In preview, move with `j`/`k`, press `v` to start a selection, then press `Enter` to add the selected line range.
- Press `ZZ` to accept the prompt.
- Press `ZQ` to cancel without changing the input file.

Examples of inserted references:

```text
@src/editor.rs
@src/editor.rs:120-180
@src/
```

Path prefixes such as `@src/editor` scope the search immediately. Search respects `.gitignore`, `.ignore`, global Git excludes, and hidden-file rules. No full workspace index is built.

## Editing reference

### Accept and cancel

| Keys | Action |
| --- | --- |
| `ZZ` | Accept |
| `ZQ` | Cancel |
| `:w`, `:wq`, `:x` | Accept |
| `:q`, `:q!` | Cancel |
| `Ctrl+C` | Clear the buffer |
| `Ctrl+C`, `Ctrl+C` | Cancel and exit |

### Navigation and changes

- `/pattern` and `?pattern` search forward and backward.
- `n` and `N` repeat the last search.
- `f`, `F`, `t`, and `T` move to characters on the current line.
- `;` and `,` repeat the last character motion.
- `iw`, `aw`, `i"`, `a"`, `i(`, and `a(` select text objects.
- `.` repeats the last change; a count repeats it multiple times.
- `Ctrl+A`, `Ctrl+E`, `Ctrl+U`, `Ctrl+W`, and `Ctrl+K` work in Insert mode.
- macOS Command and Option editing shortcuts work in supported terminals.

### History

Press `Ctrl+P` or run `:history` to open fuzzy prompt history. `Tab` switches between workspace and global history.

## Local data and privacy

Prompts are not sent over the network. Accepted prompt history and recovery drafts are stored in the operating system's application-data directory selected by the Rust `directories` crate. On Unix, `zz` restricts its data directories to mode `0700` and files to mode `0600`.

History contains the full accepted prompt. Draft files contain unfinished prompt text and workspace paths. Treat this local data as sensitive. See [docs/privacy.md](docs/privacy.md) for storage details and the current threat model.

## Platform status

| Platform | Status |
| --- | --- |
| macOS | Developed and tested locally |
| Linux | Expected to work; not yet continuously tested |
| Windows | Not yet claimed as supported |

Terminal behavior can vary. Please include the OS, terminal, shell, keyboard layout, and `zz` version in terminal-input bug reports.

## Performance

The repository includes steady-state PTY and editor performance budgets for startup, input, rendering, context search, and large pastes:

```sh
cargo bench --bench performance -- --check
```

The budgets are regression checks, not cross-project benchmark claims. Results depend on the machine, terminal, filesystem, and build profile.

## Contributing and security

- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)
- [Support](SUPPORT.md)
- [Changelog](CHANGELOG.md)

Use GitHub Issues for reproducible bugs and focused feature proposals. Please do not include private prompts, secrets, or proprietary source code in reports.
