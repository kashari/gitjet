[← Back to docs index](README.md)

# Search: find in files, go to file or class

Two searches work from every tab. Both search while you type, without blocking the screen, and
open the result in the editor (the built-in one, or yours; see
[Configuration](configuration.md)).

| Key | Palette entry | What it does |
|---|---|---|
| `,` or `F3` | **Find in files…** | Text search in all files |
| `;` or `F2` | **Go to file or class…** | Open a file by name, or a class / function by name |

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
