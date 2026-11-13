[← Back to docs index](README.md)

# The built-in editor

Files open inside gitjet in a small Vim-style editor (normal, insert and visual modes, `:w`,
`:q`, `/` search, `:%s/a/b/g`; `:help` lists the commands). It is used when you press `Enter` on
a result of [Find in files or Go to file](search.md), and for "open in editor" actions. To use
your own editor instead, set `builtin_editor = false` (see [Configuration](configuration.md)).

## Change marks

For a file that is in the last commit, a thin colored bar between the line number and the text
shows how each line differs from that commit, while you type:

| Mark | Color | Meaning |
|---|---|---|
| `▎` | green (`added`) | New line |
| `▎` | blue (`modified`) | Changed line. When one line is replaced by several, all of them count as changed. |
| `▔` | red (`removed`) | Lines were deleted just above this line |
| `▁` | red (`removed`) | Lines were deleted after this line, at the end of the file |

The marks compare with the last commit (`HEAD`), not with the file on disk, so they stay after
you save and disappear when you commit or undo the change. The colors come from the theme and
can be replaced under `[colors]`.

## Blame next to the text

| Key | Action |
|---|---|
| `gb` (press `g`, then `b`) or `Alt+B`, in normal mode | Show or hide the blame column |

With blame on, every line starts with the date and author of the commit that last changed it.
The lines of the commit under the cursor are lit in the accent color, and the bottom line shows
that commit: hash, author, how long ago, and its subject.

Lines you changed since the last save, and lines that are not committed yet, have an empty
blame cell. Blame is read from the saved file: after `:w` it is loaded again.

Plain `b` and `B` are not used for this because in Vim they move back by a word. For the full
Annotate view (jump to the commit, annotate the previous revision, history for lines) use the
Files tab: see [Files, blame and history](files-blame-history.md).
