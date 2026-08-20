# zz

A minimal modal prompt editor for coding agents.

`zz` runs as a standalone editor or through `$VISUAL`. It has one text buffer, soft wrapping, transactional accept and cancel, autosave, and crash recovery.

## Build

```sh
cargo build --release
```

## Use

Run `zz` without arguments to write the accepted prompt to stdout. Pass a file to use it as an external editor.

Set `VISUAL` to the release binary to use it from tools that support external editors.

- `ZZ` accepts the prompt.
- `ZQ` cancels without changing the input file.
- `:w`, `:wq`, and `:x` accept.
- `:q` and `:q!` cancel.
- `/pattern` and `?pattern` search forward and backward.
- `n` and `N` repeat the last search.
- `f`, `F`, `t`, and `T` move to characters on the current line.
- `;` and `,` repeat the last character motion.
- `iw` and `aw` select words.
- `i"`, `a"`, `i(`, and `a(` select quoted and parenthesized text.
