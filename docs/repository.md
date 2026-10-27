[← Back to docs index](README.md)

# Repository

The **Repository** tab (key `9`) collects the repository-level settings: **tags**, **remotes**,
**worktrees**, **submodules**, **ignore rules** and a few **Git config** values. It also hosts the
**repository switcher** and **Clone** / **Init**.

## Layout

- **Left**: the section list (Tags, Remotes, Worktrees, Submodules, Ignore rules, Config), the
  current repository and branch, and a reminder that `O` switches repository.
- **Right**: the selected section's list with a details pane for the selected row.

`Tab` moves focus between the section list and the section's list. Every operation of a section is
in its context menu (`.` or `m`), and each menu item's key also works directly, both in the list
and in the section menu. `Enter` runs the section's main action (or edits, where there is no main
action).

## How to

### Create and push a tag

1. Open **Tags**, press `n`.
2. Type the tag name (for example `v1.2.0`), then the revision (default `HEAD`; a branch, tag or
   hash), then a message. An empty message creates a lightweight tag; a message creates an
   annotated tag.
3. Press `p` to push the selected tag, or `P` to push all tags.

When there is more than one remote, gitjet asks which one to use and lists the current branch's
remote (or `origin`) first. With a single remote it is used directly.

Other tag actions: `Enter` shows the tagged commit, `w` compares the tag with the work tree, `c`
checks the tag out (detached HEAD, after confirmation), `D` deletes the local tag, `X` deletes the
tag on a remote. Tags are listed newest first.

### Manage remotes

1. Open **Remotes**. Press `n` to add one: type the name (`origin` is suggested when there is
   none), then the URL (https, ssh or a local path).
2. Select a remote. `e` (or `Enter`) edits its URL, `u` sets a separate push URL, `R` renames it,
   `D` removes it (its remote-tracking branches and settings go with it).
3. `f` fetches the selected remote, `F` fetches all remotes, `P` prunes its stale remote-tracking
   branches (`git remote prune`).

Fetches add `--prune` when `update.prune` is true (the default); see
[configuration](configuration.md). Fetching runs in the background without a terminal, so it cannot
answer password or passphrase prompts; see [troubleshooting](troubleshooting.md).

### Work with worktrees

1. Open **Worktrees**. The worktree you are in is marked.
2. Press `n` to add one. Type its directory (the default is a sibling folder named
   `<repo>-worktree`; relative paths are taken next to this repository, `~/` is your home
   directory). Then choose what to check out:
   - **New branch…** — type the branch name (default: the directory name) and its start point
     (default `HEAD`);
   - **Existing branch…** — pick a local branch; branches checked out in another worktree are
     disabled;
   - **Detached HEAD at revision…** — type a revision.
3. Select a worktree and press `Enter` to open it in gitjet (the whole app switches to it).
4. `D` removes a worktree after confirmation: its directory is deleted, the branch is kept. If it
   has uncommitted changes or untracked files nothing is removed. `X` force-removes it and discards
   those changes.
5. `l` locks a worktree (with an optional reason; locked worktrees are never pruned) or unlocks it.
   A locked worktree must be unlocked before it can be removed.
6. `P` prunes stale worktree entries whose directory is gone.

The main worktree and the one you are in cannot be removed.

### Work with submodules

- `Enter` opens an initialized submodule in gitjet.
- `i` runs `git submodule init`; `u` runs `git submodule update --init --recursive`; `s` syncs the
  URLs (`git submodule sync --recursive`).
- `n` adds a submodule: type the repository URL, then the directory inside this repository
  (defaults to the repository name).
- `d` deinitializes the selected submodule after confirmation (`git submodule deinit -f`); local
  changes inside it are lost.

The details pane shows the path, URL, checked-out commit, state (*not initialized*, *up to date*,
*different commit checked out*, *conflict*) and branch.

### Edit ignore rules

The **Ignore rules** section lists the patterns of the root `.gitignore` followed by those of
`.git/info/exclude` (comments and blank lines are skipped). The details pane says whether a rule is
shared (`.gitignore`, committed) or local to this clone (`info/exclude`).

- `n` adds a pattern to `.gitignore`, `x` adds one to `.git/info/exclude`. Prefix a pattern with
  `!` to re-include files.
- `D` removes the selected pattern (after confirmation).
- `c` checks a path (it need not exist): gitjet shows whether it is ignored or re-included, by which
  rule, in which file and on which line. If no rule matches, it says so.
- `I` lists all ignored files in the work tree (up to 2000).
- `Enter` opens the rule's file in your editor at that line; `e` edits `.gitignore`, `E` edits
  `.git/info/exclude`. A missing file is created first.

To ignore a file straight from the Changes tab, see [changes and commit](changes-and-commit.md).

### Change Git config values

The **Config** section shows these keys with their repository, global and effective values, plus a
one-line explanation:

`user.name`, `user.email`, `core.autocrlf`, `pull.rebase`, `commit.gpgsign`, `init.defaultBranch`,
`core.editor`.

- `e` (or `Enter`) edits the repository value, `g` edits the global value. An empty value removes
  the setting at that level.
- `x` unsets the repository value.
- `o` opens the repository's Git config file in your editor.

### Switch to another repository

Press `O` in this tab (or use **Switch repository…** in the `:` palette). The switcher is a
filterable list of:

| Kind | What it lists |
|---|---|
| Parent | The superproject, when the current repository is a submodule |
| Worktree | The repository's worktrees (bare and prunable ones are skipped) |
| Submodule | Initialized submodules |
| Nested | Git repositories found up to three directory levels below the work tree (`node_modules`, `target`, `vendor`, `build`, `dist`, `out`, `.idea`, `.gradle`, `.venv`, `__pycache__` and `bower_components` are skipped) |
| Recent | Repositories you opened before, most recent first (up to 20) |

The current repository and duplicates are left out. Pick an entry and press `Enter`; gitjet
reopens all tabs on that repository. The list ends with **Clone repository…** and
**Init repository…**.

Recent repositories are remembered in `recent.json` in gitjet's config directory (next to
`config.toml`, see [configuration](configuration.md)), not inside the repository. A repository is
added every time gitjet opens it; entries whose directory no longer exists are dropped.

### Clone or create a repository from inside gitjet

- **Clone repository…**: type the URL (https, ssh or a local path), then the target directory. The
  default is a sibling of the current repository named after the cloned repository; the directory
  must not exist or must be empty. The clone runs in the background and gitjet opens the new
  repository when it finishes.
- **Init repository…**: type a directory (created if missing; the field starts with the parent
  directory of the current repository). gitjet runs `git init` there and opens it.

Both are also available from the command line as `gitjet clone <url> [dir]` and
`gitjet init [dir]`; see [getting started](getting-started.md). The in-app clone runs without a
terminal, so a repository that needs a password prompt is better cloned with `gitjet clone`, which
runs `git clone` in your terminal.

## Keys

### Common keys

| Key | Action |
|---|---|
| `Tab` / `Shift+Tab` | Switch between the section list and the section's list |
| `.` / `m` | Context menu of the section |
| `O` | Switch repository |
| `/` | Filter the section's list |
| `Enter` | Run the section's main action; where there is none, edit |
| `Esc` | Clear the filter; otherwise go back to the section list |

In the **section list**: `↑` `↓` / `j` `k` change section, `Home` / `g` and `End` / `G` go to the
first / last section, `Enter`, `→` or `l` move into the list. Other letter keys run the section's
actions.

In the **section's list**: `↑` `↓` / `j` `k` move, `PgUp` / `PgDn` move ten rows, `Home` / `End`
jump to the first / last row, `←` goes back to the section list.

While typing a filter: `Enter`, `↑` or `↓` stop typing, `Esc` clears it. The filter matches the
name and the main values of each row (for example a tag's commit and subject, a remote's URLs).

### Tags

| Key | Menu item |
|---|---|
| `Enter` | Show tagged commit |
| `w` | Compare with work tree |
| `n` | New tag… |
| `c` | Checkout tag (detached HEAD)… |
| `p` | Push tag |
| `P` | Push all tags |
| `D` | Delete local tag… |
| `X` | Delete tag on remote… |

### Remotes

| Key | Menu item |
|---|---|
| `n` | Add remote… |
| `e` / `Enter` | Edit URL… |
| `u` | Set push URL… |
| `R` | Rename… |
| `D` | Remove… |
| `f` | Fetch |
| `F` | Fetch all remotes |
| `P` | Prune stale remote branches |

### Worktrees

| Key | Menu item |
|---|---|
| `Enter` | Open worktree |
| `n` | Add worktree… |
| `D` | Remove worktree… |
| `X` | Force remove worktree (discard its changes)… |
| `l` | Lock… / Unlock (from the list; in the section list `l` moves into the list) |
| `P` | Prune stale worktrees |

### Submodules

| Key | Menu item |
|---|---|
| `Enter` | Open submodule |
| `i` | Init |
| `u` | Update (init, recursive) |
| `s` | Sync URLs |
| `n` | Add submodule… |
| `d` | Deinit… |

### Ignore rules

| Key | Menu item |
|---|---|
| `Enter` | Edit file at this rule |
| `n` | Add pattern to .gitignore… |
| `x` | Add pattern to .git/info/exclude… |
| `D` | Remove pattern… |
| `c` | Check path… |
| `I` | Show ignored files |
| `e` | Edit .gitignore in editor |
| `E` | Edit .git/info/exclude in editor |

### Config

| Key | Menu item |
|---|---|
| `e` / `Enter` | Edit repository value… |
| `g` | Edit global value… (from the list; in the section list `g` goes to the first section) |
| `x` | Unset repository value |
| `o` | Open repository config in editor |

### Repository switcher

| Key | Action |
|---|---|
| type | Filter the list |
| `↑` / `↓` | Move |
| `PgUp` / `PgDn` | Move ten rows |
| `Enter` | Open the selected repository, or run Clone / Init |
| `Esc` | Close |

Remote pickers (for push and delete of tags) are plain menus: `↑` `↓` / `j` `k` move, `Enter`
picks, `Esc` or `q` closes.

## Action palette

| Entry | What it does |
|---|---|
| Switch repository… | Open the repository switcher |
| Clone repository… | Clone a repository, then open it |
| Init repository… | Create a repository, then open it |
| New tag… | Create a tag (starts at `HEAD`) |
| Add remote… | Add a remote |
| Add worktree… | Add a worktree |
| Add submodule… | Add a submodule |
| Update submodules | `git submodule update --init --recursive` |
| Fetch all remotes | `git fetch --all` (with `--prune` when `update.prune` is on) |
| Push all tags | Push all tags to a remote |
| Show repository settings (tags, remotes, worktrees…) | Switch to the Repository tab |
