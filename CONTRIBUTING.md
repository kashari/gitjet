# Contributing to gitjet

Thank you for helping improve gitjet. Bug reports, documentation fixes and pull requests are all
welcome. Please follow the [code of conduct](CODE_OF_CONDUCT.md) in all project spaces.

- Found a bug? [Open an issue](https://github.com/klajdishark/gitjet/issues/new?template=bug_report.md)
  with your OS, terminal, `gitjet --version` and `git --version`.
- Have an idea? [Suggest a feature](https://github.com/klajdishark/gitjet/issues/new?template=feature_request.md).
  Describe the task you want to do and how you would expect it to work.
- Found a security problem? Do not open a public issue; see [SECURITY.md](SECURITY.md).

For anything bigger than a small fix, please open an issue first so we can agree on the approach
before you spend time on it.

## Development setup

You need:

- **Rust** 1.80 or newer, installed with [rustup](https://rustup.rs). The stable toolchain is
  enough. `rustfmt` and `clippy` come with the default rustup profile; otherwise run
  `rustup component add rustfmt clippy`.
- **Git** 2.31 or newer, on your `PATH`. gitjet and its tests run the real `git` executable.

The tests create their own temporary repositories with a fixed identity. CI also sets a global
identity and default branch before running the tests; doing the same locally avoids surprises:

```bash
git config --global user.name "Your Name"
git config --global user.email "you@example.com"
git config --global init.defaultBranch main
```

## Build, run and test

```bash
cargo build                                 # debug build
cargo run -- -C /path/to/some/repo          # run against a repository
cargo test                                  # all tests
cargo clippy --all-targets -- -D warnings   # lints, warnings are errors
cargo fmt                                   # format
```

CI runs the same checks on Windows, macOS and Linux:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo run --locked -- --version
```

Run gitjet against a scratch repository while developing, not against work you care about. An
easy way to get one:

```bash
cargo run -- init /tmp/gitjet-playground
```

## Project layout

```text
src/main.rs      CLI and startup
src/app.rs       event loop, tabs, popups, background tasks
src/action.rs    the Action enum returned by input handlers
src/config.rs    config file model and defaults
src/git/         everything that talks to Git
src/ui/          UI framework, widgets, diff viewer
src/ui/views/    one module per tab, plus the push dialog and the rebase editor
docs/            user and contributor documentation
packaging/       Scoop manifest and Homebrew formula templates
```

[docs/architecture.md](docs/architecture.md) explains the design in detail: the `View` and
`Popup` traits, the `Action` enum, background tasks, how every Git call goes through `Repo`, and
the cross-platform rules. Please read it before a larger change.

## Coding conventions

These are the conventions the existing code follows. Matching them keeps reviews short.

**Structure**

- Git logic goes in `src/git/`, UI in `src/ui/`. A view never builds Git command lines; it calls a
  function in `src/git/`.
- Each file starts with a `//!` comment that says what the module is for. Public functions have a
  `///` comment.
- Keep pure logic (layout, models, parsing) free of terminal and Git access, in its own module, so
  it can be unit-tested. Examples: `ui/diff_view/model.rs`, `ui/views/changes/tree.rs`,
  `git/conflict/merge.rs`.
- Features talk to each other through the `pub fn` entry points of their modules
  (`files::open_blame`, `push::open`, `conflicts::open_resolver`, ...), not through each other's
  internals.

**Behavior**

- Input handlers return an `Action`; they do not change app state directly.
- Anything slow (network, big logs, blame, diffs) runs in `Action::spawn` / `Action::spawn_then`
  or a loader thread. The UI thread never waits on the network.
- Run Git only through `Repo` (`repo.run`, `repo.command()`, ...). Background Git must never be
  able to prompt. When an operation may need a password or passphrase, add an "(in terminal)"
  variant that uses `Action::Suspend`.
- Ask before destructive operations with `ConfirmPopup`, and use `.danger()` for the ones that
  lose data (hard reset, delete, force push).
- Every key a view handles should appear in its `help()`, and actions on a selection should also
  be in its context menu (`.` / `m`). Where possible, build the key handling and the menu from the
  same list so they cannot drift apart.
- Menu items and palette entries that open a dialog end with `…` (for example "New branch…");
  items that act immediately do not.
- Errors use `anyhow` with context, and are shown with `{e:#}` so the whole chain is visible.
  Messages are short and say what to do next ("resolve them in the Conflicts tab (6)").

**Style**

- Colors come from `ctx.theme`, never hard-coded, so the light theme keeps working.
- Measure text with `unicode-width` (`util::truncate`, `util::fit`), never with byte lengths.
- Follow the [cross-platform rules](docs/architecture.md#cross-platform-rules): `util::shell`
  for shell commands, `util::sh_quote` for values passed to Git's shell, `/` in repository paths.
- Format with `cargo fmt`; `cargo clippy --all-targets -- -D warnings` must pass.

**Tests**

- Test Git behavior against real temporary repositories with `crate::git::testutil`
  (`temp_repo`, `write`, `commit_files`).
- Test views and popups by rendering them into ratatui's `TestBackend` and by sending key events
  to `handle_key`. See `src/ui/views/branches/tests.rs` for examples.
- A bug fix should come with a test that fails without the fix.

## How to add a palette action

1. Write the operation in the feature's module, returning an `Action`. If it opens a dialog, end
   the name with `…`.
2. Add a `PaletteItem` to that module's `pub fn palette()`:

   ```rust
   PaletteItem {
       name: "Do the thing…",
       run: |ctx| do_the_thing(ctx),
   },
   ```

   `run` is a plain `fn(&Ctx) -> Action`, so the closure cannot capture variables. Read what you
   need from `ctx`.
3. If the feature module is new, add its `palette()` to `ui::views::palette()` in
   `src/ui/views/mod.rs`.
4. If the action also belongs on a tab, add a key to the view's `handle_key`, its `help()`, and
   its context menu.
5. Document it on the feature's page in `docs/` and in
   [docs/keyboard-reference.md](docs/keyboard-reference.md).

## How to add a tab

1. Create `src/ui/views/<name>.rs` (and a `src/ui/views/<name>/` folder for submodules) with a
   struct that implements `View`: `title`, `refresh`, `draw`, `handle_key`, and usually `help`,
   `handle_mouse`, `captures_input` and `badge`.
2. In `src/ui/views/mod.rs`: add `pub mod <name>;`, a variant to `Tab`, the variant to `Tab::ALL`
   (and its length), and the view to `build()` **at the same position**. The app relies on
   `Tab::ALL` and `build()` having the same order.
3. Add the module's `palette()` to `ui::views::palette()`.
4. The number keys only reach tabs 1 to 9. A tenth tab is reachable with `[` / `]` and the
   palette's "Go to tab" entries.
5. Add a documentation page in `docs/`, link it from `docs/README.md`, and add its keys to
   `docs/keyboard-reference.md`.

## Pull requests

Before you open a pull request:

- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo clippy --all-targets -- -D warnings` passes.
- [ ] `cargo test` passes.
- [ ] New behavior has tests; bug fixes have a regression test.
- [ ] New or changed keys are in the view's `help()`, its context menu where it applies, and in
      the docs (the feature page and `docs/keyboard-reference.md`).
- [ ] Nothing assumes one OS: shell commands, paths and quoting follow the cross-platform rules.
- [ ] `CHANGELOG.md` has an entry under `## [Unreleased]` for user-visible changes.

Keep pull requests focused on one change. Describe what changed and why, and how you tested it
(which OS and terminal). Screenshots help for UI changes.

## Release process

For maintainers.

1. Update `version` in `Cargo.toml` and run `cargo build` so `Cargo.lock` matches.
2. In `CHANGELOG.md`, rename `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD` and add a new empty
   `## [Unreleased]` section.
3. Commit, then tag and push:

   ```bash
   git tag vX.Y.Z
   git push origin main vX.Y.Z
   ```

4. The tag starts `.github/workflows/release.yml`. It builds release binaries for six targets:

   | Target | Archive |
   |---|---|
   | `x86_64-pc-windows-msvc` | `gitjet-x86_64-pc-windows-msvc.zip` |
   | `aarch64-pc-windows-msvc` | `gitjet-aarch64-pc-windows-msvc.zip` |
   | `aarch64-apple-darwin` | `gitjet-aarch64-apple-darwin.tar.gz` |
   | `x86_64-apple-darwin` | `gitjet-x86_64-apple-darwin.tar.gz` |
   | `x86_64-unknown-linux-musl` | `gitjet-x86_64-unknown-linux-musl.tar.gz` |
   | `aarch64-unknown-linux-musl` | `gitjet-aarch64-unknown-linux-musl.tar.gz` |

   Each archive contains the binary, `README.md` and `LICENSE`, and gets a `.sha256` file. The
   workflow publishes a GitHub Release with all archives, checksums, `install.sh` and
   `install.ps1`, and generated release notes. Running the workflow by hand
   (`workflow_dispatch`) only builds the archives; it publishes nothing.
5. Update the package manifests with the new version and the SHA-256 values from the release's
   `.sha256` files, then publish them to the Scoop bucket and the Homebrew tap:
   - `packaging/scoop/gitjet.json`: `version`, both `url`s and both `hash`es.
   - `packaging/homebrew/gitjet.rb`: `version` and the four `sha256` values.

## License

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE),
the same license as the project.
