//! Copy to the system clipboard with the OSC 52 terminal escape (works over SSH and in tmux with
//! `set-clipboard on`; supported by Windows Terminal, iTerm2, kitty, WezTerm, Alacritty, foot...).

use std::io::Write;

use crate::action::Action;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
pub fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The escape sequence that asks the terminal to put `text` on the clipboard.
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

/// Copy `text` to the clipboard and confirm with a toast showing what was copied.
pub fn copy(text: &str) -> Action {
    let mut out = std::io::stdout();
    let written = out
        .write_all(osc52(text).as_bytes())
        .and_then(|_| out.flush());
    match written {
        Ok(()) => Action::Toast(format!("Copied {text}")),
        Err(e) => Action::Error(format!("Could not copy to the clipboard ({e}): {text}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        let cases = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (input, expected) in cases {
            assert_eq!(base64(input.as_bytes()), expected, "{input}");
        }
        assert_eq!(base64(&[0xff, 0xfe, 0xfd]), "//79");
        assert_eq!(base64("é".as_bytes()), "w6k=");
    }

    #[test]
    fn osc52_sequence() {
        assert_eq!(osc52("abc"), "\x1b]52;c;YWJj\x07");
    }
}
