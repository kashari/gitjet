[← Back to docs index](README.md)

# Search: find in files, go to file or class, code history

Three searches work from every tab. Both search while you type, without blocking the screen, and
open the result in the editor (the built-in one, or yours; see
[Configuration](configuration.md)).

| Key | Palette entry | What it does |
|---|---|---|
| `,` or `F3` | **Find in files…** | Text search in all files |
| `;` or `F2` | **Go to file or class…** | Open a file by name, or a class / function by name |
| `;` `;` | **Code history…** | The commits that changed a piece of code, and where it is now |

`Ctrl+F` and `Ctrl+N` / `Ctrl+P` do the same where the terminal passes them on. Some terminals
keep those for themselves, which is why the plain keys exist. Like `q` and the digits, the
plain keys go into a text field when one has the focus (a filter, the commit message).

Tracked and untracked files are searched. Files ignored by `.gitignore` (build output,
dependencies) and binary files are not.

> **About `Shift` `Shift` and `Ctrl+Shift+F`.** A terminal does not tell a program that Shift
> was pressed on its own, so a double Shift cannot be detected, and most terminals send the same
> code for `Ctrl+Shift+F` as for `Ctrl+F`, or use it themselves (in Git Bash it switches to full
> screen). That is why the keys are `,` and `;`.

## Find in files (`,`)

Type at least two characters. The list shows every matching line as `path:line  text` with the
match highlighted; below it is a preview of the file around the selected match.

| Key | Action |
|---|---|
| `↑` / `↓`, `PgUp` / `PgDn` | Select a match |
| `Enter` | Open the file at that line. Closing the editor brings you back to the results. |
| `Alt+C` | Match case (off: `login` also finds `Login`) |
| `Alt+W` | Whole words only |
| `Alt+R` | Regular expression (extended syntax, as `grep -E`) instead of plain text |
| `Esc` | Close |

The three switches are shown at the right of the search field (`Aa`, `W`, `.*`) and are lit
when on. At most 1000 matches are listed; when there are more, the status line says so and the
search stops early, so a very common word does not slow anything down.

## Go to file or class (`;`)

Type part of a name. Files come first, best match on top:

| You type | Finds |
|---|---|
| `userservice`, `UserServ` | `UserService.java` (case does not matter) |
| `UsSe`, `US` | `UserService.java`, `user_service.py`: the starts of the words in the name (CamelHumps) |
| `util/str` | Files whose path contains `util/str` |
| `strings:42` | `strings.rs`, opened at line 42 |

From three characters on, gitjet also looks inside the files for **declarations** with that
name (`class`, `interface`, `struct`, `enum`, `trait`, `record`, `type`, `def`, `fn`, `func`,
`function` and similar). They are listed after the files, in the accent color, as the
declaration line with `path:line`. So a class that lives in a file with another name is found
too.

`Enter` opens the selection, `Esc` closes.

## Code history (`;` `;`)

Press `;` twice (the second `;` in the empty "go to" field switches to it). Type a piece of
code: a function name, a call such as `check(`, a string, a config key. gitjet shows:

- **History**: every commit of the current branch that added, removed or changed a line
  containing it (`git log -G`), newest first, with hash, date, author and subject. Below the
  list are only the hunks of the selected commit that touch that code, and a summary such as
  "added 2 lines" or "+1 −1 lines".
- **In the code now** (`Tab`): every place it is in the work tree today (`git grep`), with a
  preview of the file and the method or class each place is in.

The status line sums it up, for example "4 commits changed it · 2 places in 1 file now", or
"not in the code any more" for code that was deleted.

| Key | Action |
|---|---|
| `Tab` | Switch between History and In the code now |
| `↑` / `↓`, `PgUp` / `PgDn` | Select a commit or a place |
| `Shift+↑` / `Shift+↓` | Scroll the hunks of the selected commit |
| `Enter` | History: the whole commit in the diff viewer. Now: open the place in the editor |
| `Ctrl+B` or `F12` | Go to the declaration (see below) |
| `Alt+C`, `Alt+W`, `Alt+R` | Match case, whole words, regular expression (as in find in files) |
| `Esc` | Close |

**Go to declaration** (`Ctrl+B` or `F12`) opens the editor at:

- in History, when what you typed is a name: the class, method or function declared with that
  name. When there are several, "go to" opens with the name filled in so you can pick one.
- otherwise, and in "In the code now": the method, function or class that contains the
  selected place (the first place, from History). This works by indentation and declaration
  keywords, so it fits most languages (Java, C#, Kotlin, Python, Rust, Go, JavaScript...).

Closing the editor brings you back to the code history.
