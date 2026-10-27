[← Back to the project README](../README.md)

# gitjet documentation

gitjet is a keyboard-driven terminal Git client for Windows, macOS and Linux, with the Git
features of a full-featured IDE: changelists, a shelf, line-by-line staging, a 3-way merge
resolver, interactive rebase, annotate, local history and more. It runs your installed `git` for
every operation, so your hooks, credentials and Git config behave exactly as on the command line.

New here? Start with [Getting started](getting-started.md). Looking for a specific task? The
[feature overview](feature-overview.md) tells you where to find it.

![The Changes tab](screenshots/changes.png)

## Contents

### Basics

- [Getting started](getting-started.md): install, first launch, the screen, how to operate
  gitjet, command-line options.
- [Feature overview](feature-overview.md): common Git tasks and the tab and key for each.
- [Keyboard reference](keyboard-reference.md): every key for every tab and dialog.
- [Configuration](configuration.md): every setting, commit checks, hosting entries, where data is
  stored.
- [Troubleshooting](troubleshooting.md): Git not found, passwords and SSH, terminals, colors,
  unsigned binaries, line endings, resetting gitjet.

### Using gitjet

| Tab | Page | What it covers |
|---|---|---|
| `1` Changes | [Changes and commit](changes-and-commit.md) | Changelists, staging (also single lines), the commit dialog, rollback |
| | [Diff viewer](diff-viewer.md) | Unified and side-by-side diffs, comparisons with branches and revisions |
| `2` Log | [Log](log.md) | Commit graph, filters, cherry-pick, revert, reset, undo commit |
| `3` Branches | [Branches and remotes](branches-and-remotes.md) | Checkout, merge, rebase, compare, fetch, pull, Update Project, push |
| | [Interactive rebase](interactive-rebase.md) | Reorder, reword, squash, fixup, edit and drop commits |
| `4` Stash & Shelf | [Stash, shelf and patches](stash-shelf-patches.md) | Stash, shelve and unshelve, create and apply patches |
| `5` Files | [Files, blame and history](files-blame-history.md) | Annotate, file history, history for selected lines, file at a revision |
| `6` Conflicts | [Conflicts and merge resolver](conflicts.md) | Conflicted files, 3-way merge, continue, skip and abort |
| `7` Pull Requests | [Pull requests](pull-requests.md) | Pull requests on your hosting provider |
| `8` Local History | [Local history](local-history.md) | Automatic snapshots of your files, independent of commits |
| `9` Repository | [Repository](repository.md) | Tags, remotes, worktrees, submodules, ignore rules, Git config, switching repositories |

### Contributing

- [Architecture](architecture.md): module map, the UI framework, the event loop, how Git is run,
  testing, cross-platform rules.
- [Contributing guide](../CONTRIBUTING.md): development setup, conventions, pull requests,
  releases.
- [Changelog](../CHANGELOG.md)
- [Code of conduct](../CODE_OF_CONDUCT.md)
- [Security policy](../SECURITY.md)
