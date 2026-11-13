//! What the editor shows left of the text besides line numbers: a colored bar on every line
//! that differs from the last commit, and (when switched on) who last changed each line.

use similar::{capture_diff_slices, Algorithm, DiffOp};

/// How a line of the buffer differs from the committed version of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mark {
    #[default]
    Same,
    /// New line
    Added,
    /// Replaces other lines
    Modified,
    /// Lines were deleted just above this line
    DeletedAbove,
    /// Lines were deleted after this line, the last of the file
    DeletedBelow,
}

/// One mark per line of `new`, compared with `old` (the committed lines).
pub fn marks(old: &[String], new: &[String]) -> Vec<Mark> {
    let mut out = vec![Mark::Same; new.len()];
    for op in capture_diff_slices(Algorithm::Myers, old, new) {
        match op {
            DiffOp::Equal { .. } => {}
            DiffOp::Insert {
                new_index, new_len, ..
            } => out[new_index..new_index + new_len].fill(Mark::Added),
            DiffOp::Replace {
                new_index, new_len, ..
            } => out[new_index..new_index + new_len].fill(Mark::Modified),
            DiffOp::Delete { new_index, .. } => {
                // Shown on the line that follows the gap; a change on that line wins.
                if let Some(m) = out.get_mut(new_index) {
                    if *m == Mark::Same {
                        *m = Mark::DeletedAbove;
                    }
                } else if let Some(m) = out.last_mut() {
                    if *m == Mark::Same {
                        *m = Mark::DeletedBelow;
                    }
                }
            }
        }
    }
    out
}

/// For every line of `new`, the index of the same line in `old` (`None` for changed lines).
/// Used to show blame, which git computes for the file on disk, next to an edited buffer.
pub fn line_map(old: &[String], new: &[String]) -> Vec<Option<usize>> {
    let mut map = vec![None; new.len()];
    for op in capture_diff_slices(Algorithm::Myers, old, new) {
        if let DiffOp::Equal {
            old_index,
            new_index,
            len,
        } = op
        {
            for i in 0..len {
                map[new_index + i] = Some(old_index + i);
            }
        }
    }
    map
}

/// The lines of a file's bytes, as the editor's buffer holds them (no line ends; an empty file
/// is one empty line).
pub fn lines_of(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn marks_added_modified_and_deleted_lines() {
        use Mark::*;
        let old = v(&["a", "b", "c", "d", "e"]);
        assert_eq!(marks(&old, &old), [Same; 5]);
        // "b" changed, a line added after "c", "e" deleted at the end.
        let new = v(&["a", "B", "c", "new", "d"]);
        assert_eq!(
            marks(&old, &new),
            [Same, Modified, Same, Added, DeletedBelow]
        );
        // "b" deleted in the middle.
        assert_eq!(
            marks(&old, &v(&["a", "c", "d", "e"])),
            [Same, DeletedAbove, Same, Same]
        );
        // Everything new.
        assert_eq!(marks(&[], &v(&["x", "y"])), [Added, Added]);
    }

    #[test]
    fn maps_unchanged_lines_back_to_the_old_file() {
        let old = v(&["a", "b", "c"]);
        let new = v(&["top", "a", "B", "c"]);
        assert_eq!(line_map(&old, &new), [None, Some(0), None, Some(2)]);
        assert_eq!(lines_of(b"x\r\ny\n"), v(&["x", "y"]));
        assert_eq!(lines_of(b""), v(&[""]));
    }
}
