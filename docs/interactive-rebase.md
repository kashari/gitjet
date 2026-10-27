[← Back to docs index](README.md)

# Interactive rebase

gitjet's interactive rebase editor lets you rewrite the commits after a base commit. You edit the todo list in a fullscreen editor (pick, reword, edit, squash, fixup,
drop, reorder), review a summary, and gitjet runs `git rebase -i` for you without opening a text
editor. Continue, skip and abort are available while the rebase is in progress.

## How to start

- **From the Log** (tab `2`): select a commit, open its menu with `.` or `m` and choose
  **Interactively rebase from here…** (`i`). The selected commit and everything after it up to
  `HEAD` are listed. The commit must be on the current branch. See [Log](log.md).
- **From the Log, older commit message**: **Edit commit message…** on a commit other than `HEAD`
  also opens this editor.
- **From the action palette** (`:`):
  - **Interactive rebase…** asks for the base revision. The commits *after* it are rebased; the base
    itself is not included. Enter a branch, tag, hash, `HEAD~N`, `@{upstream}`, or `--root` for the
    whole history. The suggested base is `@{upstream}` when you have unpushed commits, otherwise
    `HEAD~10` (or `--root` when the branch has 10 commits or fewer).
  - **Squash last N commits…** asks for a number (default `2`), opens the editor with every commit
    after the oldest one set to *squash*, and opens the combined message right away.
  - **Reword commit…** asks for a commit (default `HEAD`), opens the editor starting at that commit,
    and opens its message for editing. Merge commits and commits outside the current branch are
    refused.

A new rebase cannot start while a merge, cherry-pick or similar operation is in progress. If a rebase
is already in progress, these entries show the rebase status menu instead (see
[below](#continue-skip-or-abort)).

## The editor

The list shows the oldest commit at the top: Git replays it from top to bottom. Columns are
ACTION, HASH, SUBJECT and AUTHOR.

- `•` marks rows you changed.
- `↑` after a hash marks a commit that is already on a remote-tracking branch.
- `↳` marks squash and fixup rows, which meld into the commit above them.
- `✎` and an italic subject show a new message.
- Dropped rows are crossed out.

The top line summarizes the plan, for example `3 pick, 1 reword · order changed`. Below the list a
pane shows the selected commit's hash, author, date and full message (or the new message, or which
commit a squash/fixup melds into).

Warnings appear above the list when:

- the range contains merge commits (they are not kept; history becomes linear),
- commits that are already pushed would be rewritten (you will need a force push),
- every commit is dropped (the branch is reset to the base).

Plans that cannot run are flagged in red: the first commit that is not dropped cannot be a squash or
fixup, and a reword needs a non-empty message.

The editor lists at most 2000 commits; choose a closer base for longer ranges. Like Git's own todo,
it leaves out merge commits and commits already applied upstream.

## How to edit the plan

1. Move to a commit with `j` / `k` or the arrow keys.
2. Set its action:
   - `p` pick: keep the commit.
   - `r` reword: opens the message editor. Save with `Ctrl+S` (or `Alt+Enter`); the row becomes
     *reword*. On a squash group, `r` edits the message of the whole melded commit, prefilled with
     the messages of the commits being squashed.
   - `e` edit: the rebase stops after this commit so you can amend it.
   - `s` squash: meld into the commit above and keep this message.
   - `f` fixup: meld into the commit above and discard this message.
   - `d` drop: remove the commit.
3. Reorder with `Alt+↑` / `Alt+↓` or `K` / `J`.
4. `u` resets the selected row (back to *pick*, original message, original position); `U` resets
   the whole list.
5. `Enter` opens the selected commit's diff in the [diff viewer](diff-viewer.md).
6. Press `x` or `Ctrl+S` to start. A confirmation shows the plan, the changed rows and any warnings
   (it is shown in red when commits are dropped or there are warnings). Confirm with `y` or `Enter`.

`Esc` closes the editor; if you changed the plan, gitjet asks before discarding it.

## How gitjet runs the rebase

- The plan is written to `.git/gitjet/rebase/git-rebase-todo` (and new messages to
  `.git/gitjet/rebase/msg-N.txt`).
- gitjet runs `git rebase -i --autostash <base>` (or `--root`). `GIT_SEQUENCE_EDITOR` is set to
  `gitjet __write-file <todo>`, which replaces the todo Git proposes with yours. `GIT_EDITOR` is
  `gitjet __noop-editor`, which accepts the message Git proposes (for example the combined message
  of a squash).
- Rewording is done with an `exec git commit --amend --only --no-verify -F <message file>` line after
  the commit (or after its squash group), so any number of messages can change in one run.
- Uncommitted changes are stashed and restored automatically (`--autostash`).
- If `HEAD` moved since the editor was opened, gitjet refuses to start; reopen the editor.

When the rebase finishes, gitjet shows "Rebase finished successfully".

## Continue, skip or abort

A rebase stops when:

- **a commit conflicts**: gitjet switches to the [Conflicts](conflicts.md) tab. Resolve the files,
  then continue.
- **a commit is marked edit**: amend it (for example with the commit dialog's amend option, see
  [Changes and commit](changes-and-commit.md)), then continue.
- **something else fails** (for example an `exec` line): Git's message is shown as an error.

The status bar shows `REBASING` while a rebase is in progress. Use these action palette entries:

- **Continue rebase**: `git rebase --continue`, accepting Git's proposed message.
- **Skip commit**: asks for confirmation, then `git rebase --skip` (the commit's changes are left
  out).
- **Abort rebase**: asks for confirmation, then `git rebase --abort` (the branch goes back to where
  it was before the rebase started).
- **Rebase status…**: a menu with the same actions plus the conflict count and details:

  | Key | Menu item |
  |---|---|
  | `c` | Continue rebase |
  | `s` | Skip commit… |
  | `a` | Abort rebase… |
  | `g` | Resolve conflicts (N), switches to the Conflicts tab |
  | `i` | Show rebase details (branch, onto, step, stopped-at commit, state, remaining steps) |

The [Conflicts](conflicts.md) tab also offers continue, abort and skip while a rebase is stopped.

## Keys

### Rebase editor

| Key | Action |
|---|---|
| `↑` `↓` / `j` `k` | Move |
| `PgUp` `PgDn` | Page up / down |
| `Home` / `g`, `End` / `G` | First / last commit |
| `p` | pick |
| `r` | reword (opens the message editor) |
| `e` | edit |
| `s` | squash |
| `f` | fixup |
| `d` | drop |
| `Alt+↑` / `K` | Move the commit up |
| `Alt+↓` / `J` | Move the commit down |
| `u` | Reset this commit |
| `U` | Reset all changes |
| `Enter` | Show the commit's diff |
| `/` | Search (subject, hash prefix, author, new message) |
| `n` / `N` | Next / previous match |
| `.` / `m` | Menu |
| `x` / `Ctrl+S` | Start the rebase (after confirmation) |
| `Esc` | Cancel (asks when the plan was changed) |
| Mouse click | Select a commit |

While searching: type to jump to the first match, `Enter` closes the search field and keeps the
matches highlighted, `Esc` clears the search.

### Editor menu (`.` / `m`)

| Key | Menu item |
|---|---|
| `p` | pick — keep the commit |
| `r` | reword — edit the message… |
| `e` | edit — stop to amend the commit |
| `s` | squash — meld into previous, keep message |
| `f` | fixup — meld into previous, discard message |
| `d` | drop — remove the commit |
| `K` | Move up |
| `J` | Move down |
| `u` | Reset this commit |
| `U` | Reset all changes |
| `Enter` | Show commit diff |
| `x` | Start rebase… |
| `c` | Cancel and close the editor |

### Message editor

| Key | Action |
|---|---|
| `Ctrl+S` / `Alt+Enter` | Save the message |
| `Esc` | Cancel |

The first line turns the warning color when it is longer than 72 characters.

## Action palette

- Interactive rebase…
- Squash last N commits…
- Reword commit…
- Continue rebase
- Abort rebase
- Skip commit
- Rebase status…
