# Contributing

Contributions should keep `zz` focused on editing prompts for coding agents. Prefer small changes with a clear user problem over broad editor features.

## Before opening a change

- Search existing issues and pull requests.
- Open an issue before a large behavior or storage-format change.
- Do not include private prompts, credentials, or proprietary source code in fixtures.
- Keep platform claims limited to behavior that can be reproduced in tests.

## Local checks

Run the formatting, lint, and test checks:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

On Unix, run the release performance budgets when changing input, rendering, storage, context search, or terminal handling:

```sh
cargo bench --bench performance -- --check
```

## Pull requests

A pull request should:

- explain the user-visible problem and the chosen behavior;
- include tests for behavior changes;
- update README or help text when commands or keys change;
- avoid unrelated refactoring;
- state which operating systems and terminals were tested.

Use a Conventional Commit subject such as `fix: preserve input on cancel` or `feat(history): add deletion`.
