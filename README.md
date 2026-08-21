# zz

A minimal modal prompt editor for coding agents.

`zz` runs as a standalone editor or through `$VISUAL`. It has one text buffer, soft wrapping, transactional accept and cancel, autosave, and crash recovery.

## Build

```sh
cargo build --release
```

## Use

Run `zz` without arguments to write the accepted prompt to stdout. Pass a file to use it as an external editor. Existing prompts open at the end by default; integrations can pass an exact UTF-8 byte position through `ZZ_CURSOR_BYTE` or `--cursor-byte`.

Set `VISUAL` to the release binary to use it from tools that support external editors.

Pi can preserve an exact composer position by loading `integrations/pi-zz-cursor.ts` as a global extension. The extension passes `ZZ_CURSOR_BYTE` when it launches `zz`; set `ZZ_BIN` if the binary is not available as `zz` on `PATH`.

- `ZZ` accepts the prompt.
- `ZQ` cancels without changing the input file.
- The first `Ctrl+C` clears the buffer; a second consecutive `Ctrl+C` cancels and exits.
- Insert mode supports macOS editing keys: `Cmd+Backspace` clears the prompt, `Option+Backspace`/`Option+Delete` delete words, and `Cmd`/`Option` with arrow keys navigate by buffer, line, or word.
- Shell-style `Ctrl+A`, `Ctrl+E`, `Ctrl+U`, `Ctrl+W`, and `Ctrl+K` also work in Insert mode.
- `:w`, `:wq`, and `:x` accept.
- `:q` and `:q!` cancel.
- `/pattern` and `?pattern` search forward and backward.
- `n` and `N` repeat the last search.
- `f`, `F`, `t`, and `T` move to characters on the current line.
- `;` and `,` repeat the last character motion.
- `iw` and `aw` select words.
- `i"`, `a"`, `i(`, and `a(` select quoted and parenthesized text.
- `.` repeats the last change; a count repeats it multiple times.
- `Ctrl+P` or `:history` opens fuzzy prompt history for the current workspace.
- `Tab` toggles between workspace and global history in the picker.
- Type `@` at a token boundary in Insert mode to search workspace files.
- `Enter` on a file opens its line preview; `Tab` attaches the whole file immediately.
- In the preview, use `j`/`k`, start a selection with `v`, and press `Enter` to attach the selected line range. `Esc` returns to files; `a` or `Tab` attaches the whole file.
- An exact directory match is offered first and can be attached without choosing a file.
- You can also type `:10` or `:10-40` to attach one line or an inclusive line range directly.
- Path prefixes such as `@src/editor` scope the search immediately; no full workspace index is built.
- Fuzzy matching treats `-`, `_`, spaces, dots, and slashes as optional separators, so `crncysdk` matches `currency-sdk/`.
- The context picker respects `.gitignore`, `.ignore`, global Git excludes, and hidden files.

## Performance

Run the steady-state PTY and editor performance budgets with:

```sh
cargo bench --bench performance -- --check
```
