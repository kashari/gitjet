[← Back to docs index](README.md)

# Feature overview

Common Git tasks and where to find them in gitjet. "Changes: `c`" means: switch to the Changes tab
(`1`) and press `c`. "Log: `.` then `R`" means: open the context menu with `.` (or `m`) and press
`R`. Names in **bold** are entries of the action palette (`:`).

If a task is not listed here, press `:` and type a few words of it: the action palette searches
every action in gitjet.

## Commit

| Task | gitjet |
|---|---|
| See what changed | Changes tab (`1`); `Enter` opens the full diff |
| Switch the preview between all, unstaged and staged changes | Changes: `d` |
| Stage or unstage files | Changes: `Space`; stage all `a`, unstage all `A` |
| Commit part of a file (single lines or hunks) | Changes: `p` on a file, select lines with `Space` or hunks with `H`, then `s` to stage or `u` to unstage |
| Discard single lines | Changes: `p`, select lines, `x` |
| Commit | Changes: `c`; **Commit…** |
| Commit and push | Changes: `C`; or the "Push after commit" option (`p`) in the commit dialog |
| Amend the last commit | Commit dialog: "Amend" option (`a` on the options row); **Amend last commit…** |
| Add a Signed-off-by trailer | Commit dialog: "Sign-off" option (`s`) |
| Sign a commit | Commit dialog: "GPG sign" option (`g`) |
| Commit as another author | Commit dialog: Author field (`Tab` to it), `Name <email>` |
| Reuse an earlier commit message | Commit dialog: `Ctrl+P` / `Ctrl+N`; list with `Ctrl+H` |
| Run tests or linters before every commit | `[[commit.checks]]` in the [config](configuration.md#commit-checks); "Run checks" option (`c`) in the commit dialog |
| Group changes into changelists | Changes: `n` (new list with the marked files), `M` (move files), `*` (set active) |
| Rename, delete or describe a changelist | Changes: `R` / `X` / `E` on a changelist header |
| Roll back changes | Changes: `r`; **Rollback all…** |
| Ignore an untracked file | Changes: `i` |
| Delete untracked files | Changes: `D` |
| Show changes as a directory tree | Changes: `t` |

## History

| Task | gitjet |
|---|---|
| Browse the commit graph | Log tab (`2`) |
| Show the diff of a commit | Log: `Enter` |
| Show the diff of one file in a commit | Log: `Tab` to the details pane, select the file, `Enter` |
| Compare a commit with its parent | Log: `.` then `P` |
| Compare a commit with the work tree | Log: `.` then `l` (commit), or `l` in the details pane (file) |
| Compare two commits | Log: mark two commits with `v`, then `.` then `C` |
| Filter by branch, user, date or path | Log: `f`, then `b` / `u` / `d` / `p` |
| Search commit messages | Log: `/` |
| Search diff content (`-S`) | Log: `f` then `s` |
| Show only one author's commits | Log: `.` then `a` |
| Go to a hash, branch or tag | Log: `g`; **Go to commit…** |
| Copy a commit hash | Log: `y`, or `.` then `c` |
| Cherry-pick commits | Log: `.` then `p` (marked commits are applied oldest first); **Cherry-pick…** |
| Revert a commit | Log: `.` then `r`; **Revert…** |
| Reset the current branch to a commit | Log: `.` then `R`, then soft / mixed / hard / keep; **Reset HEAD…** |
| Undo the last commit | Log: `.` then `u` on `HEAD`; **Undo last commit** |
| Edit a commit message | Log: `.` then `e`; for an older commit also **Reword commit…** |
| Create a branch or tag at a commit | Log: `.` then `b` / `t` |
| Check out a commit (detached HEAD) | Log: `.` then `o` |

## Rewrite history

| Task | gitjet |
|---|---|
| Interactively rebase from a commit | Log: `.` then `i`; **Interactive rebase…** |
| Reorder, reword, squash, fixup, edit or drop commits | In the [rebase editor](interactive-rebase.md): `p` `r` `e` `s` `f` `d`, move with `K` / `J` |
| Squash the last few commits | **Squash last N commits…** |
| Continue, skip or abort a rebase | Conflicts: `C` / `S` / `A`; **Continue rebase**, **Skip commit**, **Abort rebase** |

## Branches and remotes

| Task | gitjet |
|---|---|
| Check out a branch | Branches: `Enter` or `c`; **Checkout branch…** |
| Check out a remote branch | Branches: `Enter` on the remote branch (creates a local tracking branch) |
| Check out with local changes in the way | Branches: checkout, then "Smart checkout (stash, checkout, unstash)" (`s`) or "Force checkout (discard local changes)" (`f`) |
| Create a branch | Branches: `n`; **New branch…** |
| Rename or delete a branch | Branches: `R` / `D` |
| Mark a branch as favorite | Branches: `*` |
| Merge a branch into the current one | Branches: `M`, then merge, `--no-ff`, squash or fast-forward only; **Merge branch…** |
| Rebase the current branch onto another | Branches: `r`; **Rebase onto…** |
| Check out a branch and rebase it onto the current one | Branches: `C` |
| Compare a branch with the current one | Branches: `=`; **Compare with branch…** |
| Diff a branch with the work tree | Branches: `w` |
| Set or unset the upstream | Branches: `u` |
| Fetch | Branches: `f`; **Fetch** |
| Pull | Branches: `p` (merge, rebase or fast-forward only); **Pull…** |
| Fetch and integrate upstream changes in one step | Branches: `U`; **Update Project** (rebase or merge per `update.method`) |
| Review commits before pushing, then push | Branches: `P` on the current branch; **Push…** |
| Force push safely | Push dialog: `f` (force with lease), then `p` |
| Push tags | Push dialog: `t`; Repository > Tags: `p` / `P` |
| Enter a password or SSH passphrase | **Fetch (in terminal)**, **Pull (in terminal)**, **Push (in terminal)** |

## Conflicts

| Task | gitjet |
|---|---|
| See conflicted files | Conflicts tab (`6`); it opens by itself when an operation stops on conflicts |
| Resolve conflicts in a 3-way merge | Conflicts: `Enter` on a file |
| Take your or their change for one conflict | Merge resolver: `h` / `l` |
| Apply all non-conflicting changes at once | Merge resolver: `A` |
| Go to the next or previous conflict | Merge resolver: `n` / `N` |
| Take your or their version of a whole file | Conflicts: `o` / `t`; all files `O` / `T` |
| Compare your version with theirs | Conflicts: `v` |
| Continue, skip or abort the merge, rebase or cherry-pick | Conflicts: `C` / `S` / `A`; **Continue operation**, **Abort operation** |

## Stash, shelf and patches

| Task | gitjet |
|---|---|
| Stash changes | Stash & Shelf: `s`; Changes: `S`; **Stash changes…** |
| Apply or pop a stash | Stash & Shelf: `a` / `p` (with a "Restore index" option); **Unstash (pop latest)** |
| Create a branch from a stash | Stash & Shelf: `b` |
| Drop or clear stashes | Stash & Shelf: `d` / `C` |
| Shelve changes | Stash & Shelf: `S` (all); Changes: `s` (selection); **Shelve all changes…** |
| Unshelve all or some files | Stash & Shelf: `u` on an entry, or mark files with `Space` in its Files pane and press `u`; **Unshelve…** |
| Rename or delete a shelf entry | Stash & Shelf: `R` / `d`; `D` cleans up unshelved entries |
| Create a patch from local changes | Stash & Shelf: `c`; **Create patch from local changes…** |
| Create a patch from a commit | Stash & Shelf: `F`; **Create patch from commit…** |
| Export a shelf entry as a patch | Stash & Shelf: `x` |
| Apply a patch | Stash & Shelf: `P`; **Apply patch…** |

## Files

| Task | gitjet |
|---|---|
| Browse the project tree | Files tab (`5`) |
| Annotate a file (blame) | Files: `b` or `Enter`; also `b` in the Changes tab, the diff viewer and the Log details pane |
| Annotate the revision before a line's last change | Annotate view: `p` |
| Show the commit that changed a line | Annotate view: `Enter` or `d` |
| Show a file's history (follows renames) | Files: `h` |
| Show the history of selected lines | Annotate view: `v` to select, then `L`; or Files: `L` and enter a line range |
| Show a file at a revision | Files: `r`; file history: `o` |
| Compare a file with a revision | Files: `w`; **Compare with branch or revision…** for the whole tree |
| Compare two files | **Compare two files…** |
| Restore a file from an old revision | File history: `R` |
| Open the file in your editor at the current line | Diff viewer: `o`, `F4`, or `Enter` in the diff |

## Diff viewer

| Task | gitjet |
|---|---|
| Next or previous change | `n` / `N`, or `F7` / `Shift+F7` |
| Side-by-side or unified layout | `s` |
| Ignore whitespace | `W` |
| Highlight changed words | `w` |
| Show the whole file | `e` |
| More or less context | `+` / `-` |

## Local history

| Task | gitjet |
|---|---|
| Get back an uncommitted version of a file | Local History tab (`8`); `l` in the Files tab; `L` in the Changes tab and diff viewer; then `R` to revert |
| Put a label on the current state | Local History: `l`; **Put local history label…** |
| Take a snapshot now | Local History: `s`; **Snapshot changed files to local history** |

## Repository

| Task | gitjet |
|---|---|
| Manage remotes | Repository tab (`9`) > Remotes |
| Create, push or delete tags | Repository > Tags: `n`, `p` / `P`, `D` / `X` |
| Work with worktrees | Repository > Worktrees |
| Update submodules | Repository > Submodules: `u`; **Update submodules** |
| Add an ignore pattern | Repository > Ignore rules: `n` (`.gitignore`) or `x` (`info/exclude`) |
| Change `user.name`, `core.autocrlf` and other Git settings | Repository > Config |
| Clone a repository | **Clone repository…**; command line `gitjet clone` |
| Create a repository | **Init repository…**; command line `gitjet init` |
| Switch to another repository, worktree or submodule | Repository: `O`; **Switch repository…** |
