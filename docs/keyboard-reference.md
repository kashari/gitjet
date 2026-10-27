[← Back to docs index](README.md)

# Keyboard reference

Every key in gitjet, grouped by screen. Inside gitjet, press `?` to see the keys of the current
tab, and look at the hint line at the bottom for the keys of the open dialog.

Keys are case-sensitive: `d` and `D` are different keys. `Ctrl+X` means hold Ctrl and press X.

## Contents

- [Global keys](#global-keys)
- [Common dialogs](#common-dialogs)
- [Text fields](#text-fields)
- [Changes tab](#changes-tab)
- [Log tab](#log-tab)
- [Branches tab](#branches-tab)
- [Stash & Shelf tab](#stash--shelf-tab)
- [Files tab](#files-tab)
- [Conflicts tab](#conflicts-tab)
- [Pull Requests tab](#pull-requests-tab)
- [Local History tab](#local-history-tab)
- [Repository tab](#repository-tab)
- [Diff viewer](#diff-viewer)
- [Interactive rebase editor](#interactive-rebase-editor)

## Global keys

These work on every tab, unless a text field in the tab has focus (then the letter keys go into
the field). While a dialog or fullscreen view is open, it receives all keys instead; only
`Ctrl+C` always works.

| Key | Action |
|---|---|
| `1` to `9` | Switch to tab 1 (Changes) to 9 (Repository) |
| `[` / `]` | Previous / next tab |
| `:` or `` Alt+` `` | Action palette (search all actions) |
| `?` | Keys of the current tab, global keys and the config file path |
| `F5` | Refresh everything |
| `q` | Quit |
| `Ctrl+C` | Quit (always, also inside dialogs) |

Most tabs also follow these conventions (see each tab for the exact keys):

| Key | Action |
|---|---|
| `.` or `m` | Context menu for the selection |
| `Tab` | Next pane |
| `/` | Filter |
| `Enter` | Open |
| `Esc` | Back, clear filter or marks |
| `j` / `k`, `↓` / `↑` | Move |

## Common dialogs

### Action palette and context menus

| Key | Action |
|---|---|
| `↑` / `↓` | Move |
| `PgUp` / `PgDn` | Move 10 entries |
| `Enter` | Run the selected entry |
| `Esc` | Close |
| `j` / `k` | Move (context menus only; in the palette, letters go into the search field) |
| Shortcut character | Run the entry with that one-character shortcut (context menus only) |
| `q` | Close (context menus only) |

### Confirmation

| Key | Action |
|---|---|
| `y`, `Y` or `Enter` | Yes |
| `n`, `N`, `Esc` or `q` | No |

### Single-line input

| Key | Action |
|---|---|
| `Enter` | OK (empty input is ignored unless the dialog allows it) |
| `Esc` | Cancel |

### Multi-line text

| Key | Action |
|---|---|
| `Ctrl+S` or `Alt+Enter` | Save (empty text is ignored) |
| `Enter` | New line |
| `↑` `↓` `←` `→`, `Home` / `End`, `PgUp` / `PgDn` | Move the cursor |
| `Esc` | Cancel |

### Message and error dialogs

| Key | Action |
|---|---|
| `↑` / `↓`, `j` / `k` | Scroll one line |
| `PgUp` / `PgDn`, `Space` | Scroll a page |
| `Esc`, `Enter` or `q` | Close |

## Text fields

Single-line text fields (filters, inputs, the palette search) support:

| Key | Action |
|---|---|
| `←` / `→` | Move the cursor |
| `Home` / `End`, `Ctrl+A` / `Ctrl+E` | Start / end of the line |
| `Backspace` / `Delete` | Delete a character |
| `Ctrl+W`, `Alt+Backspace` | Delete the previous word |
| `Ctrl+U` | Delete everything before the cursor |

## Changes tab

See [Changes and commit](changes-and-commit.md).

| Key | Action |
|---|---|
| `j` / `k`, `↓` / `↑` | Move (scroll the preview when it has focus) |
| `PgUp` / `PgDn` | Page up / down (in the preview when it has focus) |
| `g` / `Home` | First row |
| `G` / `End` | Last row |
| `h` / `←` | Collapse, or go to the parent |
| `l` / `→` | Expand |
| `Enter` | Open the full diff; resolve a conflicted file; toggle a header or directory |
| `Tab` / `Shift+Tab` | Switch focus between list and preview |
| `J` / `K` | Scroll the preview down / up 3 lines |
| `Ctrl+D` / `Ctrl+U` | Scroll the preview half a page down / up |
| `d` | Cycle the preview mode: Local (HEAD), Unstaged, Staged |
| `t` | Flat list / directory tree |
| `/` | Filter by path (`Enter` keep, `Esc` clear, `↑`/`↓` move while typing) |
| `v` | Mark / unmark |
| `Space` | Stage, or unstage when everything selected is fully staged |
| `a` | Stage all |
| `A` | Unstage all |
| `p` or `e` | Partial staging of the selected file |
| `c` | Commit… |
| `C` | Commit and push… |
| `r` | Rollback… |
| `D` | Delete unversioned files… |
| `i` | Add to .gitignore… |
| `n` | New changelist (takes the marked files) |
| `M` | Move to another changelist… |
| `R` | Rename the selected changelist |
| `X` | Delete the selected changelist |
| `E` | Edit the selected changelist's description |
| `*` | Set the changelist as active |
| `s` | Shelve the selection… |
| `S` | Stash all changes… |
| `o` | Open in editor |
| `b` | Annotate (blame) |
| `H` | File history |
| `L` | Local history |
| `x` | Resolve conflict |
| `.` or `m` | Context menu for the selection |
| `Esc` | Dismiss the notice, else clear marks, else clear the filter, else focus the list |

### Changes context menu (`.` or `m`)

| Key | Item |
|---|---|
| `c` | Commit… |
| `C` | Commit and Push… |
| `Space` | Stage / Unstage |
| `p` | Partial stage / unstage lines… |
| `d` | Show diff |
| `r` | Rollback… |
| `M` | Move to another changelist… |
| `n` | New changelist… |
| `R` | Rename changelist… |
| `E` | Edit changelist description… |
| `*` | Set as active changelist |
| `X` | Delete changelist… |
| `s` | Shelve changes… |
| `S` | Stash all changes… |
| `b` | Show blame |
| `H` | File history |
| `L` | Local history |
| `x` | Resolve conflict… |
| `o` | Open in editor |
| `D` | Delete unversioned files… |
| `i` | Add to .gitignore… |
| `a` | Stage all |
| `A` | Unstage all |
| `F` | Refresh |

### Move to changelist menu

| Key | Action |
|---|---|
| `1` - `9` | Move to that changelist |
| `n` | New changelist… |
| `↑` / `↓`, `j` / `k`, `Enter` | Choose |
| `Esc` | Close |

### Commit dialog (editing)

| Key | Action |
|---|---|
| `Ctrl+S` or `Alt+Enter` | Commit |
| `Tab` / `Shift+Tab` | Next / previous field: message, options, author |
| `Ctrl+P` / `Ctrl+N` | Previous / next message from history |
| `Ctrl+H` or `Ctrl+R` | Message history list |
| `Esc` | Close (keeps the message as a draft) |
| `←` / `→`, `h` / `l` | Options row: move between options |
| `Space` / `Enter` | Options row: toggle the option |
| `a` `s` `g` `c` `p` | Options row: toggle Amend, Sign-off, GPG sign, Run checks, Push after commit |
| `Enter` | Author field: back to the message |

### Commit dialog (running)

| Key | Action |
|---|---|
| `Esc` | Hide the dialog; the commit keeps running |

### Commit dialog (checks failed)

| Key | Action |
|---|---|
| `c` | Commit anyway |
| `e` / `Esc` | Back to the message |
| `↑` / `↓`, `j` / `k` | Scroll the output |
| `PgUp` / `PgDn`, `Space` | Scroll a page |

### Partial staging screen

| Key | Action |
|---|---|
| `j` / `k`, `↓` / `↑` | Move |
| `Ctrl+D` / `Ctrl+U` | Half a page down / up |
| `PgUp` / `PgDn` | Page up / down |
| `g` / `Home`, `G` / `End` | First / last row |
| `n` or `]` | Next hunk |
| `N` or `[` | Previous hunk |
| `Space` or `Enter` | Select / unselect the line (on a hunk header: the whole hunk) |
| `H` | Select / unselect the hunk under the cursor |
| `a` | Select / unselect all changed lines |
| `c` | Clear the selection |
| `d` or `Tab` | Switch between the unstaged and the staged diff |
| `s` | Stage the selected lines |
| `u` | Unstage the selected lines |
| `x` | Discard the selected lines (asks first) |
| `r` | Reload |
| `Esc` or `q` | Close |

## Log tab

See [Log](log.md).

Both panes:

| Key | Action |
|---|---|
| `/` | Filter by text |
| `f` | Filter menu |
| `F` | Clear all filters |
| `a` | All branches / `HEAD` |
| `g` | Go to hash, branch, tag or revision |
| `t` | Relative / absolute dates |
| `D` | Show / hide the details pane |

Commit list:

| Key | Action |
|---|---|
| `j`/`k`, `↓`/`↑` | Move |
| `PgDn` / `PgUp` | Page |
| `Home` / `End` | First / last loaded commit |
| `Enter` | Show the commit diff |
| `v`, `Space` | Mark / unmark and move down |
| `Esc` | Clear marks |
| `y` | Copy the full hash |
| `.`, `m` | Commit menu |
| `Tab`, `Shift+Tab` | Focus the details pane |

Details pane:

| Key | Action |
|---|---|
| `j`/`k`, `↓`/`↑` | Select a changed file |
| `Home` / `End` | First / last file |
| `PgDn` / `PgUp` | Scroll the commit message |
| `Enter` | Diff of the selected file |
| `d` | Diff of the whole commit |
| `b` | Annotate the file at this revision |
| `h` | Git history of the file |
| `l` | Compare the file with the work tree |
| `y` | Copy the file path |
| `.`, `m` | File menu |
| `Tab`, `Shift+Tab`, `Esc` | Back to the commit list |

### Commit menu

| Key | Item |
|---|---|
| `c` | Copy revision hash / Copy N hashes |
| `o` | Checkout revision (detached HEAD)… |
| `b` | New branch here… |
| `t` | New tag here… |
| `p` | Cherry-pick / Cherry-pick N commits |
| `r` | Revert commit / Revert N commits |
| `R` | Reset current branch to here… |
| `u` | Undo last commit (only on `HEAD`) |
| `e` | Edit commit message… |
| `i` | Interactively rebase from here… |
| `d` | Show diff |
| `P` | Compare with previous |
| `l` | Compare with local |
| `C` | Compare the two marked commits (exactly two marked) |
| `v` | Mark / unmark |
| `a` | Show only commits by `<author>` |

### File menu

| Key | Item |
|---|---|
| `d` | Show diff |
| `l` | Compare with local |
| `b` | Annotate (blame) at this revision |
| `h` | Show history |
| `c` | Copy path |

### Reset menu

| Key | Item |
|---|---|
| `s` | Soft: keep changes staged |
| `m` | Mixed: keep changes in the work tree, unstaged |
| `h` | Hard: discard all changes (asks for confirmation) |
| `K` | Keep: discard commits, keep local changes |

### Filter menu

| Key | Item |
|---|---|
| `b` | Branch: … |
| `u` | User: … |
| `d` | Date: … |
| `p` | Path: … |
| `t` | Text: … |
| `s` | Search text in: commit messages / diff content (-S) |
| `F` | First-parent only: on/off |
| `n` | No merges: on/off |
| `x` | Clear all filters |

### Date filter menu

| Key | Item |
|---|---|
| `1` | Last 24 hours |
| `7` | Last 7 days |
| `3` | Last 30 days |
| `y` | Last year |
| `s` | Since… |
| `u` | Until… |
| `x` | Any date |

The branch and user pickers are searchable lists: type to filter, `↑`/`↓` and `PgUp`/`PgDn` to
move, `Enter` to pick, `Esc` to close.

## Branches tab

See [Branches and remotes](branches-and-remotes.md).

### Branches tree

Keys that act on a branch work in both panes and use the branch selected in the tree.

| Key | Action |
|---|---|
| `↑` `↓` / `j` `k` | Move |
| `PgUp` `PgDn` | Page up / down |
| `Home` / `g`, `End` / `G` | First / last row |
| `Enter` | On a branch: checkout. On a section or remote header: collapse / expand |
| `Space` | On a section or remote header: collapse / expand |
| `←` / `h` | On a header: collapse. On a branch: jump to its header |
| `→` / `l` | On a header: expand. On a branch: focus the branch commits pane |
| `Tab` / `Shift+Tab` | Focus the branch commits pane |
| `/` | Filter branches by name |
| `Esc` | Clear the filter |
| `c` | Checkout |
| `n` | New branch from the selected branch (from `HEAD` on a header) |
| `C` | Checkout the selected branch and rebase it onto the current one |
| `M` | Merge the selected branch into the current one (menu) |
| `r` | Rebase the current branch onto the selected one |
| `=` | Compare with the current branch |
| `w` | Show diff with working tree |
| `u` | Set or unset upstream |
| `P` | Push (dialog for the current branch; background push for another local branch) |
| `R` | Rename |
| `D` | Delete |
| `*` | Add to / remove from favorites |
| `f` | Fetch all remotes |
| `p` | Pull (menu) |
| `U` | Update project |
| `.` / `m` | Context menu |
| Mouse click | Select a row, or focus the commits pane |

While typing a filter: `Enter` keeps the filter and returns to the list, `Esc` clears it, `↑` `↓`
move the selection.

### Branch commits pane

| Key | Action |
|---|---|
| `↑` `↓` / `j` `k` | Move |
| `PgUp` `PgDn` | Page up / down |
| `Home` / `g`, `End` / `G` | First / last commit |
| `Enter` | Show the commit's diff |
| `Esc` / `Tab` / `Shift+Tab` / `←` / `h` | Back to the branches tree |

The branch keys (`c`, `n`, `M`, `P`, `.` …) also work here.

### Branch context menu (`.` / `m`)

| Key | Menu item |
|---|---|
| `c` | Checkout / Checkout as new local branch tracking *remote/branch*… |
| `n` | New branch from *branch*… (on a header: New branch from HEAD…) |
| `C` | Checkout and rebase onto *current* |
| `=` | Compare with *current* |
| `w` | Show diff with working tree |
| `r` | Rebase *current* onto *branch* |
| `M` | Merge *branch* into *current*… |
| `P` | Push… |
| `R` | Rename… |
| `u` | Set upstream… |
| `*` | Add to favorites / Remove from favorites |
| `D` | Delete / Delete *branch* on the remote… |
| `f` | Fetch all remotes |
| `p` | Pull… |
| `U` | Update project |
| | Fetch (in terminal) |
| | Pull (in terminal) |
| | Push (in terminal) |

### Checkout blocked by local changes

| Key | Menu item |
|---|---|
| `s` | Smart checkout (stash, checkout, unstash) |
| `v` | Show the N untracked file(s) in the way |
| `f` | Force checkout (discard local changes) |
| `Esc` | Cancel |

### New branch dialog

| Key | Action |
|---|---|
| `Enter` | Create |
| `Tab` / `↓` | Next field |
| `Shift+Tab` / `↑` | Previous field |
| `Space` | Toggle the focused checkbox |
| `Esc` | Cancel |

### Merge menu

| Key | Menu item |
|---|---|
| `m` | Merge |
| `n` | Merge (--no-ff, always create a merge commit) |
| `s` | Squash merge (stage changes, commit yourself) |
| `f` | Fast-forward only |

### Pull menu

| Key | Menu item |
|---|---|
| `m` | Pull, merge |
| `r` | Pull, rebase |
| `f` | Pull, fast-forward only |
| `t` | Pull (in terminal) |

### Compare dialog

| Key | Action |
|---|---|
| `↑` `↓` / `j` `k` | Move |
| `PgUp` `PgDn` | Page up / down |
| `Home` / `g`, `End` / `G` | First / last commit |
| `Tab` / `Shift+Tab` / `←` `→` / `h` `l` | Switch list |
| `Enter` | Show commit diff |
| `d` | Diff from the merge base to the branch |
| `r` | Reload |
| `Esc` / `q` | Close |

### Push dialog

| Key | Action |
|---|---|
| `p` / `Ctrl+S` | Push |
| `Enter` | Show diff of the selected commit or file; on the Push button: push |
| `Tab` / `Shift+Tab` | Next / previous pane (commits, files, Push button) |
| `↑` `↓` / `j` `k` | Move in the focused list |
| `PgUp` `PgDn` | Page up / down |
| `Home` / `g`, `End` / `G` | First / last |
| `f` | Toggle force with lease |
| `t` | Toggle push tags |
| `u` | Toggle set upstream |
| `R` | Change target remote and branch |
| `Esc` / `q` | Cancel |

The upstream chooser and the palette's branch pickers are filterable menus: type to filter, `↑`
`↓` to move, `Enter` to choose, `Esc` to close.

## Stash & Shelf tab

See [Stash, shelf and patches](stash-shelf-patches.md).

### Everywhere in the tab

| Key | Action |
|---|---|
| `Tab` / `Shift+Tab` | Next / previous pane |
| `Enter` | Show the diff of the selected entry (list panes) or file (Files and Diff panes) in the diff viewer |
| `j` / `↓`, `k` / `↑` | Move (in the Diff pane: scroll) |
| `PgDn` / `PgUp` | Page |
| `Home` / `g`, `End` / `G` | First / last |
| `l` / `→` | From a list pane to the Files pane |
| `h` / `←` | From the Files or Diff pane back to the list |
| `J` / `K` | Scroll the diff one line |
| `Ctrl+D` / `Ctrl+U` | Scroll the diff half a page |
| `/` | Filter the focused list (not in the Diff pane); `Enter`, `Tab`, `↑` or `↓` ends editing, `Esc` clears it |
| `Esc` | In the Files or Diff pane: back to the list; in a list: clear its filter |
| `s` | Stash changes… |
| `S` | Shelve all changes… |
| `c` | Create patch from local changes… |
| `F` | Create patch from commit… |
| `P` | Apply patch… |
| `.` or `m` | Context menu |
| Mouse click | Focus a pane and select the clicked row |

### Stashes pane

These keys also work in the Files and Diff panes while a stash is shown.

| Key | Action |
|---|---|
| `a` | Apply stash… |
| `p` | Pop stash… |
| `d` | Drop stash (asks first) |
| `b` | New branch from stash… |
| `C` | Clear all stashes (asks first) |

### Shelf pane

These keys also work in the Files and Diff panes while a shelf entry is shown, except `u`.

| Key | Action |
|---|---|
| `u` | Unshelve… (file checklist) |
| `R` | Rename… |
| `x` | Export as patch… |
| `d` | Delete (asks first) |
| `D` | Clean up unshelved entries (asks first) |

### Files pane (shelf entry)

| Key | Action |
|---|---|
| `Space` | Mark or unmark the file and move down |
| `u` | Unshelve the marked files, or the selected file (also in the Diff pane) |

### Context menu

For a stash: "Show diff" `Enter`, "Apply stash…" `a`, "Pop stash…" `p`,
"New branch from stash…" `b`, "Drop stash" `d`, "Clear all stashes" `C`.

For a shelf entry: "Show diff" `Enter`, "Unshelve…" `u`, "Unshelve selected file" or
"Unshelve N marked files" `f` (only from the Files and Diff panes), "Rename…" `R`,
"Export as patch…" `x`, "Delete" `d`, "Clean up unshelved entries" `D`.

Always: "Stash changes…" `s`, "Shelve all changes…" `S`, "Create patch from local changes…" `c`,
"Create patch from commit…" `F`, "Apply patch…" `P`.

### Stash, shelve, unshelve and apply-stash dialogs

| Key | Action |
|---|---|
| `Enter` | Confirm |
| `↑` / `↓` | Move between rows |
| `j` / `k` | Move (outside the text field) |
| `PgDn` / `PgUp` | Move by 12 rows (outside the text field) |
| `Tab` / `Shift+Tab` | Next / previous section (text, files, options) |
| `Space` | Toggle the file or option on the current row |
| `a` | Toggle all files (on a file row) |
| `Alt+`*key* | Toggle the option with that key, from anywhere in the dialog |
| *key* | Toggle the option with that key (outside the text field) |
| `Esc` | Cancel |

Option keys: Stash Changes `u` (include untracked), `i` (keep index); Apply / Pop stash `p` (pop),
`r` (restore index); Shelve Changes `w` (keep changes in the work tree).

### Apply patch preview

| Key | Action |
|---|---|
| `Enter` or `y` | Apply the patch |
| `j` / `↓`, `k` / `↑` | Select a file |
| `Home` / `g`, `End` / `G` | First / last file |
| `PgDn` or `Space`, `PgUp` | Scroll the diff a page |
| `J` / `K` | Scroll the diff a line |
| `Esc` or `q` | Cancel |

## Files tab

See [Files, blame and history](files-blame-history.md).

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
| `d` | Show local changes (file, or all changed files under the directory) |
| `l` | Local history of the file |
| `o` | Open in editor |
| `/` | Filter files (fuzzy) |
| `E` / `C` | Expand all / collapse all (not while filtering) |
| `Esc` | Clear the filter |
| `Tab` / `Shift+Tab` | Switch between tree and preview |
| `.` / `m` | Context menu (also has Shelve changes… `s` and Resolve conflict… `x`) |

Preview focused: `↑` `↓` / `j` `k` scroll, `PgUp` / `PgDn` / `Space` page, `Home` / `End` / `G`
start / end, `Esc` back to the tree. Filter field: `↑` / `↓` move, `Enter` / `Tab` stop typing,
`Esc` clear.

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
| `o` | Open in editor at this line, or show the file at this revision |
| `r` | Reload |
| `.` / `m` | Context menu |
| `Esc` | Clear the selection; otherwise close |
| `q` | Close |

### File history

Also used for directory history and history for lines.

| Key | Action |
|---|---|
| `↑` `↓` / `j` `k` | Select a revision |
| `PgUp` / `PgDn`, `Space` | Page |
| `Home` / `End`, `G` | First / last revision |
| `Tab` / `Shift+Tab` | Switch between list and diff preview |
| `Enter` / `d` | Show the full diff |
| `p` | Compare with previous revision |
| `w` | Compare with local (work tree) |
| `o` | Show file at this revision |
| `b` | Annotate this revision |
| `c` | Commit details |
| `R` | Get this revision (restore file) |
| `/` | Filter commits (subject, author, hash, path) |
| `r` | Reload |
| `.` / `m` | Context menu |
| `Esc` | Clear the filter, leave the diff preview, or close |
| `q` | Close |

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

### Commit details

| Key | Action |
|---|---|
| `↑` `↓` / `j` `k` | Scroll |
| `PgUp` / `PgDn`, `Space` | Page |
| `Esc` / `Enter` / `q` | Close |

## Conflicts tab

See [Conflicts and merge resolver](conflicts.md).

| Key | Action |
|---|---|
| `Enter` | Open the 3-way merge resolver |
| `o` | Accept yours for the selected file |
| `t` | Accept theirs for the selected file |
| `a` | Mark as resolved (asks first if conflict markers remain) |
| `e` | Open in the external editor at the first conflict marker |
| `v` | Compare yours with theirs |
| `d` | Resolve by deleting the file (asks first) |
| `H` | Show the Git history of the file |
| `L` | Show the local history of the file |
| `O` | Accept yours for all files (asks first) |
| `T` | Accept theirs for all files (asks first) |
| `C` | Continue the operation |
| `S` | Skip the current commit (not for merges) |
| `A` | Abort the operation (asks first) |
| `.` or `m` | Context menu |
| `/` | Filter the list by path; `Enter`, `↑` or `↓` ends editing, `Esc` clears it |
| `Esc` | Clear the filter |
| `↓` / `j`, `↑` / `k` | Move |
| `PgDn` / `PgUp` | Page down / up |
| `Home` / `g`, `End` / `G` | First / last file |
| Mouse click | Select a file; click the selected file again to open the resolver |

### Conflicts context menu

With a file selected: "Merge… (3-way resolver)" `Enter`, "Accept yours" `o`, "Accept theirs" `t`,
"Mark as resolved" `a`, "Open in external editor" `e`, "Compare yours with theirs" `v`,
"Show your changes (base → yours)" `y`, "Show their changes (base → theirs)" `i`,
"Resolve by deleting the file…" `d`, "Show history" `H`, "Show local history" `L`.

Always: "Accept yours for all files…" `O`, "Accept theirs for all files…" `T`,
"Continue *operation*" `C`, "Skip this commit…" `S`, "Abort *operation*…" `A` (the operation name,
such as "merge" or "rebase", is filled in; items that do not apply are disabled).

### Merge resolver (text)

| Key | Action |
|---|---|
| `n` / `N` | Next / previous unresolved change |
| `Tab` or `]` | Next change |
| `Shift+Tab` or `[` | Previous change |
| `h` | Accept yours for this change |
| `l` | Accept theirs for this change |
| `b` | Accept both (yours, then theirs) |
| `B` | Accept both (theirs, then yours) |
| `x` | Discard both (keep base) |
| `u` | Undo this change's resolution |
| `A` | Apply all non-conflicting changes |
| `H` / `L` | Accept yours / theirs for every change |
| `R` | Reset: drop text edits, or clear all resolutions (asks first) |
| `e` | Edit the result in the external editor (asks before replacing hand edits) |
| `W` | Use the work-tree file as the result |
| `j` / `↓`, `k` / `↑` | Scroll one line |
| `PgDn` or `Space`, `PgUp` | Scroll one page |
| `Ctrl+D` / `Ctrl+U` | Scroll half a page |
| `Home` / `g`, `End` / `G` | Top / bottom |
| `←` / `→` | Scroll sideways |
| `Ctrl+S` | Save and mark resolved |
| `?` | Show the resolver keys |
| `Esc` | Close (asks first if there are unsaved changes) |

### Merge resolver (choice)

| Key | Action |
|---|---|
| `h` or `o` | Accept yours |
| `l` or `t` | Accept theirs |
| `?` | Show the keys |
| `Esc` or `q` | Close |

## Pull Requests tab

| Key | Action |
|---|---|
| `↑` `↓` / `j` `k` | Move / scroll |
| `Tab` | Switch between list and details |
| `PgUp` `PgDn` / `Space` | Page |
| `g` / `Home`, `End` | First / last |
| `/` | Search the loaded pull requests |
| `s` | Cycle the state filter (open, closed, merged, all) |
| `Enter` or `d` | Show the diff |
| `c` | Check out as `pr/N` |
| `n` | Create a pull request |
| `N` | Create a pull request in the browser |
| `C` | Comment |
| `r` | Review (approve, request changes, comment) |
| `G` | Merge |
| `x` | Close / reopen |
| `w` | Open the pull request in the browser |
| `W` / `B` | Open the repository / current branch in the browser |
| `L` | Sign in |
| `R` | Reload |
| `.` or `m` | All actions |
| `Esc` | Clear the search / back |

On hosts without a supported API the tab offers `W`, `B`, `H` (HEAD commit in the browser), `N`,
`c` / `d` by number where the provider has a pull request ref, `L` and `R` (check again). See
[Pull requests](pull-requests.md).

## Local History tab

See [Local history](local-history.md).

| Key | Action |
|---|---|
| `↑` `↓` / `j` `k` | Move in the focused list, or scroll the preview |
| `PgUp` / `PgDn` | Ten rows, or a page of the preview |
| `Home` / `g` | First entry / top of the preview |
| `End` / `G` | Last entry / end of the preview |
| `Tab` / `Shift+Tab` | Next / previous pane |
| `Enter` / `→` | File list: go to its snapshots. Otherwise: full diff against the current file |
| `←` | Back to the file list |
| `R` | Revert to the selected snapshot |
| `l` | Put label |
| `s` | Snapshot changed files now |
| `P` | Prune old snapshots |
| `t` | Switch between files and timeline |
| `/` | Filter (list focused) |
| `h` | Git history of the file |
| `b` | Annotate (blame) the file |
| `e` | Open the file in your editor |
| `Esc` | Clear the filter; otherwise back to the list |
| `.` / `m` | Context menu |

### Local history popup

Opened with `l` in the Files tab and `L` in the Changes tab, diff viewer and Conflicts tab.

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

## Repository tab

See [Repository](repository.md). Each section's menu keys work directly in the list and in the
section menu.

| Key | Action |
|---|---|
| `Tab` / `Shift+Tab` | Switch between section list and section's list |
| `.` / `m` | Context menu of the section |
| `O` | Switch repository |
| `/` | Filter the section's list |
| `Enter` | Main action of the section (edit where there is none) |
| `Esc` | Clear the filter; otherwise back to the section list |
| Section list: `↑` `↓` / `j` `k` | Change section |
| Section list: `Home` / `g`, `End` / `G` | First / last section |
| Section list: `Enter` / `→` / `l` | Move into the list |
| List: `↑` `↓` / `j` `k` | Move |
| List: `PgUp` / `PgDn` | Ten rows |
| List: `Home` / `End` | First / last row |
| List: `←` | Back to the section list |

### Tags section

| Key | Action |
|---|---|
| `Enter` | Show tagged commit |
| `w` | Compare with work tree |
| `n` | New tag… |
| `c` | Checkout tag (detached HEAD)… |
| `p` | Push tag |
| `P` | Push all tags |
| `D` | Delete local tag… |
| `X` | Delete tag on remote… |

### Remotes section

| Key | Action |
|---|---|
| `n` | Add remote… |
| `e` / `Enter` | Edit URL… |
| `u` | Set push URL… |
| `R` | Rename… |
| `D` | Remove… |
| `f` | Fetch |
| `F` | Fetch all remotes |
| `P` | Prune stale remote branches |

### Worktrees section

| Key | Action |
|---|---|
| `Enter` | Open worktree |
| `n` | Add worktree… |
| `D` | Remove worktree… |
| `X` | Force remove worktree (discard its changes)… |
| `l` | Lock… / Unlock (list focused) |
| `P` | Prune stale worktrees |

### Submodules section

| Key | Action |
|---|---|
| `Enter` | Open submodule |
| `i` | Init |
| `u` | Update (init, recursive) |
| `s` | Sync URLs |
| `n` | Add submodule… |
| `d` | Deinit… |

### Ignore rules section

| Key | Action |
|---|---|
| `Enter` | Edit file at this rule |
| `n` | Add pattern to .gitignore… |
| `x` | Add pattern to .git/info/exclude… |
| `D` | Remove pattern… |
| `c` | Check path… |
| `I` | Show ignored files |
| `e` | Edit .gitignore in editor |
| `E` | Edit .git/info/exclude in editor |

### Config section

| Key | Action |
|---|---|
| `e` / `Enter` | Edit repository value… |
| `g` | Edit global value… (list focused) |
| `x` | Unset repository value |
| `o` | Open repository config in editor |

### Repository switcher

| Key | Action |
|---|---|
| type | Filter |
| `↑` / `↓` | Move |
| `PgUp` / `PgDn` | Move ten rows |
| `Enter` | Open the repository, or run Clone / Init |
| `Esc` | Close |

## Diff viewer

Fullscreen popup opened from many screens. See [Diff viewer](diff-viewer.md).

| Key | Action |
|---|---|
| `n`, `F7` | Next change (continues into the next file) |
| `N`, `Shift+F7` | Previous change (continues into the previous file) |
| `]` / `[` | Next / previous file |
| `s` | Toggle unified / side-by-side |
| `w` | Toggle word-level highlighting |
| `h` | Toggle syntax highlighting |
| `W` | Toggle ignore whitespace |
| `+`, `=` / `-` | More / fewer context lines |
| `e` | Toggle whole file / changes only |
| `f` | Show / hide the file list |
| `r`, `F5` | Reload |
| `Tab`, `Shift+Tab` | Switch focus between file list and diff |
| `/` | Filter the file list (more than one file) |
| `o`, `F4` | Open the file in your editor at the caret line |
| `b` | Annotate (blame) |
| `H` | File history |
| `L` | Local history |
| `.`, `m` | Diff actions menu |
| `Esc` | Clear the file filter, otherwise close |
| `q` | Close |

Diff pane:

| Key | Action |
|---|---|
| `j`/`k`, `↓`/`↑` | Move the caret |
| `Ctrl+D` / `Ctrl+U` | Half a page down / up |
| `PgDn`, `Space` / `PgUp` | Page down / up |
| `g`, `Home` / `G`, `End` | Top / bottom |
| `←` / `→` | Scroll sideways |
| `Enter` | Open the file in your editor at this line |

File list (focused):

| Key | Action |
|---|---|
| `j`/`k`, `↓`/`↑` | Next / previous file |
| `PgDn` / `PgUp` | Page down / up |
| `g`, `Home` / `G`, `End` | First / last file |
| `Enter`, `→`, `l` | Focus the diff |

File filter field:

| Key | Action |
|---|---|
| type | Filter (fuzzy) |
| `↑` / `↓` | Previous / next matching file |
| `Enter` | Keep the filter |
| `Esc` | Clear the filter |

### Diff actions menu

| Key | Item |
|---|---|
| `n` | Next difference |
| `N` | Previous difference |
| `]` | Next file |
| `[` | Previous file |
| `s` | Side-by-side view / Unified view |
| `w` | Word-level highlighting (on/off) |
| `h` | Syntax highlighting (on/off) |
| `W` | Ignore whitespace (on/off) |
| `+` | More context (N lines) |
| `-` | Less context |
| `e` | Show whole file / Show changes only |
| `f` | Hide file list / Show file list |
| `r` | Reload |
| `o` | Open in editor |
| `b` | Annotate (blame) |
| `H` | Show file history |
| `L` | Show local history |

### Compare with branch or revision

Chooser:

| Key | Action |
|---|---|
| type | Filter refs or enter a revision |
| `↑` / `↓` | Move |
| `PgUp` / `PgDn` | Move by 10 |
| `Enter` | Compare |
| `Esc` | Close |

Then the comparison menu:

| Key | Item |
|---|---|
| `w` | Working tree vs `<rev>` |
| `b` | Changes on `<rev>` since it diverged from `<current branch>` |
| `c` | Changes on `<current branch>` since it diverged from `<rev>` |
| `d` | `<current branch>` vs `<rev>` (direct) |

## Interactive rebase editor

See [Interactive rebase](interactive-rebase.md).

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

While searching: `Enter` closes the search field and keeps the matches highlighted, `Esc` clears the
search.

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

### Rebase status menu (palette: Rebase status…)

| Key | Menu item |
|---|---|
| `c` | Continue rebase |
| `s` | Skip commit… |
| `a` | Abort rebase… |
| `g` | Resolve conflicts (N) |
| `i` | Show rebase details |
