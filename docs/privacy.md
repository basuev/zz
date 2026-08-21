# Local data and privacy

`zz` is local software. The current code has no network client, account system, update checker, analytics, or telemetry.

## Stored data

`zz` stores two kinds of data in the operating system's application-data directory resolved by the Rust `directories` crate for the `dev.zz.zz` application identifier:

- `history.sqlite3` contains accepted prompt text, a workspace path, a content hash, and an acceptance timestamp;
- `drafts/*.json` contains an unfinished prompt, cursor position, workspace path, seed hash, and update timestamp.

On macOS this normally resolves below the user's Application Support directory. Linux and Windows paths follow their platform conventions. The exact location can vary with the environment.

On Unix, `zz` sets its data directories to mode `0700` and data files to mode `0600`. These permissions reduce access from other local users but do not protect against malware, a compromised user account, backups, or a privileged process.

## Context search

Context search reads workspace directory entries and may invoke `fd` when it is installed. Otherwise it uses an in-process filesystem walker. It does not upload file names or contents. Adding context inserts a path reference such as `@src/editor.rs:10-40` into the prompt; the calling agent decides how to interpret that reference.

## Sensitive prompts

Prompt history and recovery drafts may contain source code, credentials, customer data, or internal instructions. Avoid placing secrets in prompts. Review local backups and device-encryption settings according to your threat model.

A future feature that sends prompt contents or usage data over the network must be explicit, documented, and disabled by default. It is not part of the current project.
