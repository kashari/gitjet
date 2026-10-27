[← Back to docs index](README.md)

# Local history

Local history keeps automatic snapshots of the files you change, independent of commits, so you can
get back a version you never committed or stashed. You can browse the snapshots, put labels on
them and revert a file to any of them.

The **Local History** tab is key `8`.

## How snapshots are taken

- **Automatically.** While gitjet runs, it takes a snapshot in the background every
  `local_history.interval_secs` seconds (default 60, never more often than every 5 seconds). This
  is silent: no spinner, no message. Turn it off with `local_history.enabled = false`; the tab then
  shows *(automatic snapshots off)* in its titles, and manual snapshots still work.
- **Which files.** Every file that `git status` reports as changed or untracked (ignored files and
  submodules excluded), plus every file that already has local history. The second group is why
  local history also records when you revert a file to its committed version or delete an
  untracked file. At most 5000 files are examined per run.
- **Only real changes.** A file is stored only when its content differs from its latest snapshot.
  Identical contents are stored once.
- **Size limit.** Files larger than `local_history.max_file_kb` (default 1024 KB) are skipped.
- **Deletions.** When a file that has history disappears, a *deleted* entry is recorded.
- **Retention.** After each snapshot, entries older than `local_history.keep_days` (default 14) are
  removed, together with stored contents nothing refers to any more.

See [configuration](configuration.md) for the `[local_history]` settings.

### Where it is stored

Local history lives in the repository's gitjet data directory, so it never shows up as a change in
your work tree:

```text
.git/gitjet/local-history/
  objects/     file contents, named by a content hash (deduplicated)
  index.jsonl  one line per snapshot (path, time, content hash, optional label)
  state.json   size and modification time per file, so unchanged files are skipped quickly
  lock         serializes writers
```

Each linked worktree has its own store under `.git/gitjet/local-history/worktrees/<id>/`, because
the paths in the index are relative to the work tree.

## The Local History tab

The tab has two modes. Press `t` to switch between them.

**Files** (the default):

- Left: the files that have snapshots, most recent first, with the time of the newest snapshot.
  Files whose newest entry is a deletion are shown in the "removed" color.
- Right, top: the snapshots of the selected file, newest first. Labels are shown with `◆`.
- Right, bottom: a preview of the selected snapshot compared with the current file
  (`- snapshot   + current file`). It says when the snapshot is identical to the current file, or
  when the file no longer exists.

**Timeline**: every snapshot of every file in one list (with paths and labels), newest first, plus
the preview.

## How to

### See what a file looked like earlier

1. Press `8` to open the tab, select the file (press `/` to filter by path).
2. Press `Enter` (or `→`) to move into its snapshots and pick one. The preview shows the
   difference to the current file.
3. Press `Enter` again to open the full comparison in the [diff viewer](diff-viewer.md).

To jump straight to one file's local history from elsewhere, press `l` in the Files tab, `L` in
the Changes tab, the diff viewer or the Conflicts tab, or use the palette entry
**Show local history for file…**. This opens the [local history popup](#local-history-popup).

### Revert a file to a snapshot

1. Select the snapshot and press `R`.
2. Confirm. The file gets the snapshot's content. If the snapshot is a *deleted* entry, the file
   is deleted.

Before writing, gitjet saves the file's current content to local history (whatever its size), and
it records the reverted state right after, so a revert can itself be reverted. gitjet refuses to
revert through a symbolic link, onto something that is not a regular file, or outside the work
tree.

### Put a label

1. Press `l` and type a label, for example `before refactor`.
2. All currently changed files are snapshotted under that label, even if they did not change since
   their last snapshot, and a project-wide label marker is added. The marker shows up in the
   timeline; selecting it explains which files carry the label.

### Snapshot now, prune now

- `s` snapshots all changed files immediately (in the popup: just that file).
- `P` removes entries older than `keep_days` right away. This also happens after every snapshot.

### Jump to Git

From a snapshot (or the selected file): `h` opens the file's Git history, `b` annotates it, `e`
opens it in your editor. See [files, blame and history](files-blame-history.md).

## Keys

### Local History tab

| Key | Action |
|---|---|
| `↑` `↓` / `j` `k` | Move in the focused list, or scroll the preview |
| `PgUp` / `PgDn` | Ten rows up / down, or a page of the preview |
| `Home` / `g` | First entry (top of the preview) |
| `End` / `G` | Last entry (end of the preview) |
| `Tab` / `Shift+Tab` | Next / previous pane (files, snapshots, preview; in timeline mode: list, preview) |
| `Enter` / `→` | In the file list: go to its snapshots. Otherwise: open the full diff against the current file |
| `←` | Back to the file list |
| `R` | Revert to the selected snapshot (asks first) |
| `l` | Put label |
| `s` | Snapshot changed files now |
| `P` | Prune old snapshots |
| `t` | Switch between files and timeline |
| `/` | Filter (when the list has focus): by path; in the timeline also by label |
| `h` | Git history of the file |
| `b` | Annotate (blame) the file |
| `e` | Open the file in your editor |
| `Esc` | Clear the filter; otherwise go back to the list |
| `.` / `m` | Context menu |

While typing a filter: `Enter`, `↑` or `↓` stop typing, `Esc` clears it.

### Local History context menu

| Item | Key |
|---|---|
| Show diff with current file | `Enter` |
| Revert to this snapshot… | `R` |
| Put label… | `l` |
| Snapshot changed files now | `s` |
| Prune old snapshots | `P` |
| Show timeline (all files) / Show files | `t` |
| Git history of this file | `h` |
| Annotate (blame) this file | `b` |
| Open file in editor | `e` |

### Local history popup

The popup shows one file's snapshots on the left and the preview on the right.

| Key | Action |
|---|---|
| `↑` `↓` / `j` `k` | Move, or scroll the preview |
| `PgUp` / `PgDn` | Ten rows, or a page of the preview |
| `Home` / `g`, `End` / `G` | First / last snapshot |
| `Tab` / `Shift+Tab` / `←` / `→` | Switch between list and preview |
| `Enter` | Full diff against the current file |
| `R` | Revert to the selected snapshot |
| `s` | Snapshot this file now |
| `l` | Put label |
| `Esc` / `q` | Close |

## Action palette

| Entry | What it does |
|---|---|
| Put local history label… | Ask for a label and snapshot all changed files under it |
| Show local history | Switch to the Local History tab |
| Snapshot changed files to local history | Snapshot all changed files now |
| Show local history for file… | Ask for a repository-relative path and open its local history popup |
| Prune old local history | Remove entries older than `keep_days` |
