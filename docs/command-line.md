[← Back to docs index](README.md)

# Command line

`gitjet` without a subcommand opens the UI. The subcommands on this page do one task from the
shell and exit, so they also work in scripts and over SSH without a full-screen terminal.

All of them accept `-C <dir>` before the subcommand to work in another repository:
`gitjet -C ~/src/app pick 5c84679`.

## `gitjet pick`: cherry-pick commits by hash

```bash
gitjet pick <hash>...                 # onto the current branch
gitjet pick --onto release/1.4 <hash>...
gitjet pick --push <hash>...
git log --format=%H --author=me -5 main | gitjet pick -y
```

Give the hashes in any order, full or abbreviated. They can be separated by spaces, commas,
semicolons or new lines, so a list copied from a ticket can be pasted as it is. Without hashes on
the command line (or with `-`), the list is read from standard input.

What happens:

1. **Every hash is checked first.** If any is unknown or ambiguous, gitjet lists all of them and
   changes nothing.
2. **The plan is printed**: the commits in the order they will be applied, oldest first by
   commit date. Commits the branch already contains are marked and left out. Duplicates are
   dropped.
3. **gitjet asks** `Cherry-pick? [y/N]`.
4. The commits are applied. Merge commits are picked against their first parent. Commits whose
   change is already on the branch are skipped, and the result says how many.

| Option | Effect |
|---|---|
| `--onto <branch>` | Put the commits on this local branch **without switching to it**. gitjet applies them in a temporary work tree inside `.git/gitjet/`, so your files and uncommitted changes are not touched. If a commit conflicts there, the whole pick is undone and the branch is left as it was; switch to the branch and pick there to resolve the conflict. |
| `--push` | Push the branch afterwards (to its upstream, or to the first remote as a new branch). |
| `--keep-order` | Apply in the order given instead of oldest first. |
| `-n`, `--dry-run` | Print the plan and stop. |
| `-y`, `--yes` | Do not ask. Required when the list is piped in, because there is no terminal to ask on. |

If a pick onto the current branch stops on a conflict, the repository is left in the middle of
the cherry-pick. Open `gitjet` to resolve it in the Conflicts tab, or use
`git cherry-pick --continue` / `git cherry-pick --abort`.

Inside the UI the same thing is the palette entry **Cherry-pick commits…** (see
[Log](log.md#cherry-pick)).

## `gitjet revert`: revert commits by hash

```bash
gitjet revert <hash>...
gitjet revert --push <hash>...
```

Same input rules as `gitjet pick`. The commits are reverted on the current branch, newest first,
with one revert commit each (`git revert --no-edit`). Merge commits are reverted against their
first parent. Options: `--push`, `-n` / `--dry-run`, `-y` / `--yes`.

## `gitjet sync`: bring all branches up to date

```bash
gitjet sync             # fetch, then fast-forward
gitjet sync --no-fetch  # use what was fetched before
```

Fetches all remotes, then fast-forwards every local branch that is only behind its upstream,
without checking it out. Your current branch is fast-forwarded too, unless uncommitted changes
are in the way. Branches that also have commits of their own are never touched; they are
listed as "diverged" so you can merge or rebase them yourself. In the UI: palette
**Sync all branches**.

## `gitjet cleanup`: delete branches you are done with

```bash
gitjet cleanup -n      # only show
gitjet cleanup         # delete merged branches, after asking
gitjet cleanup --gone  # also delete branches whose remote branch is gone but have unmerged commits
```

Lists local branches that are merged into the main branch (where `origin/HEAD` points, else
`main` or `master`; a merge into your local main that is not pushed yet counts), and branches
whose remote branch was deleted. Merged branches are deleted after you confirm. Branches with
commits that are not in the main branch are kept unless you pass `--gone`. The current branch,
the main branch and branches open in another work tree are never listed. In the UI: palette
**Clean up branches…**.

## `gitjet undo`: go back to before the last operation

```bash
gitjet undo --list   # the last operations on this branch, numbered
gitjet undo          # back to before the last one
gitjet undo 3        # back to before the third last
```

Moves the current branch back to where it was before a commit, merge, pull, rebase, reset or
cherry-pick, using Git's reflog. Uncommitted changes are kept; if they touch files that would
change, nothing happens and gitjet tells you to commit or stash first. Nothing is lost: the
undo is itself an operation, so `gitjet undo` again brings the previous state back. In the UI:
palette **Undo last operation…**.

## `gitjet fixup`: put staged changes into an earlier commit

```bash
git add src/login.rs
gitjet fixup 5c84679
```

Folds what is staged into that commit, as if it had been part of it from the start. This
rewrites the commit and every commit after it, so use it on commits you have not pushed. If the
change does not fit that commit without conflicts, nothing is changed and your changes stay
staged. Histories with merge commits after the target are refused. In the UI: Log, `.` on the
commit, then `f`.

## Other subcommands

| Command | Effect |
|---|---|
| `gitjet clone <url> [dir]` | Clone a repository, then open it |
| `gitjet init [dir]` | Create a repository, then open it |
| `gitjet config-path` | Print the location of the config file |
| `gitjet config-init` | Write a config file with all default settings (if none exists) |
