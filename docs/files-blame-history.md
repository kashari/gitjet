[← Back to docs index](README.md)

# Files, blame and history

The **Files** tab (key `5`) is a project tree of your repository with a content preview. From any
file you can open **Annotate** (blame), **file history**, **history for selected lines**, the
**file at a revision**, and a **comparison with a revision**.

All of these views are also reachable from other screens: the Changes tab (`b` / `H`), the diff
viewer (`b` / `H`), the Log details pane (`b` / `h`), the Conflicts tab (`H`) and the Local History
tab (`h` / `b`).

## The Files tab

The left pane (45% of the width) is a tree of every tracked file plus untracked files that are not
ignored. Directory chains with a single child are shown compacted (`src/ui/views`). Each entry is
colored by its status and shows its status letter; deleted files are struck through. A directory
with changes below it shows `N changed`.

The right pane previews the selected entry:

- a text file shows its first 256 KB with line numbers (longer files say the preview is
  truncated);
- a binary file shows its size;
- a directory shows how many files it contains and how many are changed;
- a file that was deleted from the work tree is reported as missing.

## How to

### Find a file

1. Press `/` and type. The tree turns into a flat list of matching paths (fuzzy matching; file
   names are preferred over directory names; at most 5000 rows).
2. Use `↑` / `↓` to move while typing, or press `Enter` (or `Tab`) to stop typing and keep the
   filter.
3. Press `Esc` to clear the filter. The tree comes back with the selected file revealed.

### Annotate a file (blame)

1. Select a file and press `Enter` or `b`.
2. The Annotate view opens fullscreen. Each block of lines shows the short hash, the author (when
   the terminal is at least 70 columns wide) and the date of the commit that last changed it.
3. The gutter color shows the age of the change: the newest commit gets the accent color, the
   oldest the dim color, everything else is blended in between. Lines that are not committed yet
   read `Not Committed Yet` in the "modified" color.
4. Lines from the same commit as the cursor line are bold and marked with `▌`. The footer shows the
   full author, e-mail, time and subject of the cursor line's commit.

Untracked files cannot be annotated. A file that was deleted locally is annotated at `HEAD`.
Binary files are refused.

### Navigate in Annotate

- `Enter` or `d` opens the diff of the commit that changed the cursor line (for an uncommitted
  line: your local changes to the file).
- `p` annotates the **previous revision**: a new Annotate view opens at the revision before the one
  that changed the line, with the cursor on the same line. For an uncommitted line it annotates
  `HEAD`. If the line was added in the first commit, gitjet says so.
- `c` shows the commit details (hash, author, dates, refs, full message, changed files).
- `n` / `N` jump to the next / previous block of lines from the same commit.
- `/` finds text (case-insensitive), `F3` finds the next match, `Shift+F3` the previous one.
- `g` asks for a line number to go to.
- `o` opens the file in your editor at the cursor line. When you are annotating an older revision,
  `o` shows the file at that revision instead.
- `r` reloads.

### Show the history for selected lines

1. In Annotate, press `v` on the first line and move to the last line. The footer shows
   `lines A-B selected`.
2. Press `L`. The history popup lists only the commits that touched those lines
   (`git log -L`), each with the hunks that affected them.
3. Press `Esc` once to clear the selection; again to close Annotate.

Without a selection, `L` uses the cursor line. From the Files tab you can press `L` on a file and
type a range instead: `42`, `10-20`, `10,20` or `10:20`.

### Show a file's history

1. Select a file (or a directory) and press `h`.
2. The history popup opens fullscreen: the commit list on top, the diff of the selected commit for
   that file below. For a file, the history follows renames (`git log --follow`); renames are shown
   as `[old → new]`. For a directory, every commit touching it is listed.
3. Commits load 500 at a time; more are loaded as you scroll down.
4. Press `Tab` to move into the diff preview and scroll it; `Tab` or `Esc` brings you back.

In the history popup:

- `Enter` or `d` opens the full diff of the commit for this file.
- `p` compares the revision with the previous revision in the list.
- `w` compares the revision with your local (work tree) file.
- `o` shows the file at that revision; `b` annotates that revision.
- `c` shows the commit details.
- `/` filters the list by subject, author, hash or path.

For a commit that deleted the file, these actions use the version just before the deletion.

### Get (restore) an old revision of a file

1. Open the file's history (`h`) and select the revision.
2. Press `R` and confirm. The local file is replaced with the content from that revision.
   Uncommitted changes to the file are lost.

When the paths are the same this runs `git checkout <rev> -- <path>`, which updates the index and
the work tree. If the file was renamed since, the old content is written to the file's current
path so it keeps its new name.

### Show a file at a revision

1. Select a file and press `r`.
2. Type a revision (branch, tag, hash, `HEAD~2`…; the field starts with `HEAD`) and press `Enter`.
3. The file opens read-only. Use `/`, `F3`, `g` to search and jump, `b` to annotate that revision
   (at the cursor line), `w` to compare it with the local file, `h` for the file history and `c`
   for the commit details.

### Compare a file with a revision

1. Select a file and press `w`.
2. Type a revision and press `Enter`. The [diff viewer](diff-viewer.md) opens with that revision on
   the left and your local file on the right.

### Other file actions

- `d` shows your local changes to the selected file, or to every changed file under the selected
  directory, in the [diff viewer](diff-viewer.md).
- `l` opens the file's [local history](local-history.md).
- `o` opens the file in your editor (see `editor` in [configuration](configuration.md)).
- The context menu (`.` or `m`) also has **Shelve changes…** for the changed files at or below the
  selection (see [stash, shelf and patches](stash-shelf-patches.md)) and **Resolve conflict…** for a
  conflicted file (see [conflicts](conflicts.md)).

## Keys

### Files tab

| Key | Action |
|---|---|
| `↑` `↓` / `j` `k` | Move |
| `PgUp` / `PgDn`, `Space` | Page up / down |
| `Home` / `End`, `G` | First / last row |
| `←` | Collapse the directory, or go to the parent directory |
| `→` | Expand the directory, or move to its first child |
| `Enter` | Directory: expand / collapse. File: annotate |
| `b` | Annotate (blame) |
| `h` | File or directory history |
| `L` | History for lines (asks for a range) |
| `r` | Show file at revision (asks for a revision) |
| `w` | Compare with revision (asks for a revision) |
| `d` | Show local changes of the file, or of all changed files under the directory |
| `l` | Local history of the file |
| `o` | Open in editor |
| `/` | Filter files |
| `E` / `C` | Expand all / collapse all (not while filtering) |
| `Esc` | Clear the filter |
| `Tab` / `Shift+Tab` | Switch between tree and preview |
| `.` / `m` | Context menu |

When the preview has focus, `↑` `↓` / `j` `k` scroll, `PgUp` / `PgDn` / `Space` page, `Home` /
`End` / `G` jump to the start or end, and `Esc` returns to the tree. The action keys above still
work on the selected file.

While typing a filter: `↑` / `↓` move, `Enter` or `Tab` stop typing, `Esc` clears the filter.

Mouse: click a row to select it, click the selected directory again to expand or collapse it,
click the preview to focus it.

### Files context menu

| Item | Key | Available for |
|---|---|---|
| Expand / Collapse | `Enter` | Directories |
| Annotate (blame) | `b` | Tracked files |
| Show history | `h` | Directories and tracked files |
| Show history for lines… | `L` | Tracked files |
| Show file at revision… | `r` | Tracked files |
| Compare with revision… | `w` | Tracked files |
| Local history | `l` | Files |
| Open in editor | `o` | Files |
| Resolve conflict… | `x` | Conflicted files |
| Show local changes | `d` | When there are changes at or below the selection |
| Shelve changes… | `s` | When there are changes (not for a conflicted file) |
| Filter files | `/` | Always |
| Expand all | `E` | When not filtering |
| Collapse all | `C` | When not filtering |

### Annotate (blame)

| Key | Action |
|---|---|
| `↑` `↓` / `j` `k` | Move one line |
| `PgUp` / `PgDn`, `Space` | Page |
| `Home` / `End`, `G` | First / last line |
| `Enter` / `d` | Diff of the commit that changed this line |
| `p` | Annotate previous revision |
| `c` | Commit details |
| `h` | File history |
| `v` | Start / clear a line selection |
| `L` | History for the selection (or the cursor line) |
| `/` | Find in file |
| `F3` / `Shift+F3` | Find next / previous |
| `g` | Go to line |
| `n` / `N` | Next / previous block of the same commit |
| `o` | Open in editor at this line (work tree), or show the file at this revision |
| `r` | Reload |
| `.` / `m` | Context menu |
| `Esc` | Clear the selection; otherwise close |
| `q` | Close |

In the find and go-to-line prompts, `Enter` confirms and `Esc` cancels.

The Annotate context menu has: Show diff of this commit (`d`), Annotate previous revision (`p`),
Commit details (`c`), Show file history (`h`), Start / clear line selection (`v`), Show history for
selection (`L`), Find in file (`/`), Find next (`F3`), Go to line (`g`), Next change of this commit
(`n`), Previous change of this commit (`N`), and Open in editor or Show file at this revision
(`o`).

### File history and history for lines

| Key | Action |
|---|---|
| `↑` `↓` / `j` `k` | Select a revision |
| `PgUp` / `PgDn`, `Space` | Page |
| `Home` / `End`, `G` | First / last revision |
| `Tab` / `Shift+Tab` | Switch between list and diff preview |
| `Enter` / `d` | Show the full diff |
| `p` | Compare with previous revision |
| `w` | Compare with local (work tree) |
| `o` | Show file at this revision (not for directories) |
| `b` | Annotate this revision (not for directories) |
| `c` | Commit details |
| `R` | Get this revision (restore file), after confirmation (not for directories) |
| `/` | Filter commits |
| `r` | Reload |
| `.` / `m` | Context menu |
| `Esc` | Clear the filter, leave the diff preview, or close |
| `q` | Close |

In the diff preview: `↑` `↓` / `j` `k` scroll, `PgUp` / `PgDn` / `Space` page, `Home` / `End` /
`G` jump. While typing a filter: `↑` / `↓` move, `Enter` or `Tab` stop typing, `Esc` clears it.

The context menu has: Show diff (`d`), Compare with previous revision (`p`), Compare with local
(work tree) (`w`), Show file at this revision (`o`), Annotate this revision (`b`), Commit details
(`c`), Get this revision (restore file)… (`R`), Filter commits (`/`), Focus diff preview (`Tab`).

### File at revision

| Key | Action |
|---|---|
| `↑` `↓` / `j` `k` | Move one line |
| `PgUp` / `PgDn`, `Space` | Page |
| `Home` / `End`, `G` | First / last line |
| `/` | Find in file |
| `F3` / `Shift+F3` | Find next / previous |
| `g` | Go to line |
| `b` | Annotate this revision |
| `w` | Compare with local (work tree) |
| `h` | File history |
| `c` | Commit details |
| `.` / `m` | Context menu |
| `Esc` / `q` | Close |

The context menu has: Annotate this revision (`b`), Compare with local (work tree) (`w`), Show
file history (`h`), Commit details (`c`), Find in file (`/`), Go to line (`g`).

### Commit details

A scrollable message window: `↑` `↓` / `j` `k` scroll, `PgUp` / `PgDn` / `Space` page, `Esc`,
`Enter` or `q` close.

## Action palette

Press `:` and type any of these:

| Entry | What it does |
|---|---|
| Annotate file… | Pick a tracked file (or type a path), then annotate it |
| Annotate file at revision… | Type a revision, pick a file in that revision, annotate it there |
| Show file history… | Pick a tracked file, then open its history |
| Show history for lines… | Pick a tracked file, type a line range, open the line history |
| Show file at revision… | Type a revision, pick a file in that revision, show it |

The file pickers are filterable; their first entry, **Type a path…**, lets you enter a
repository-relative path by hand.
