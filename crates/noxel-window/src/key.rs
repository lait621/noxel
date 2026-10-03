//! Key codes.
//!
//! The numbers are chosen so that `key::letter('W') == 87`, which is the ASCII
//! code for `W`. That is not an accident: `noxel_app::InputState::movement_axis`
//! takes raw `u32` codes, and a game that writes `movement_axis(87, 83, 65, 68)`
//! gets exactly what it expects, with no translation table between the window
//! and the engine.
//!
//! Non-printable keys live above `0xFF` so they can never collide with an ASCII
//! byte.

/// The W key, and so on for every letter: the ASCII uppercase code.
pub const W: u32 = b'W' as u32;
/// A.
pub const A: u32 = b'A' as u32;
/// S.
pub const S: u32 = b'S' as u32;
/// D.
pub const D: u32 = b'D' as u32;
/// Q.
pub const Q: u32 = b'Q' as u32;
/// E.
pub const E: u32 = b'E' as u32;
/// R.
pub const R: u32 = b'R' as u32;
/// F.
pub const F: u32 = b'F' as u32;
/// C.
pub const C: u32 = b'C' as u32;
/// M.
pub const M: u32 = b'M' as u32;
/// P.
pub const P: u32 = b'P' as u32;

/// Escape.
pub const ESCAPE: u32 = 0x1B;
/// Space.
pub const SPACE: u32 = 0x20;
/// Enter or return.
pub const ENTER: u32 = 0x0D;
/// Tab.
pub const TAB: u32 = 0x09;
/// Backspace.
pub const BACKSPACE: u32 = 0x08;

/// Left arrow.
pub const LEFT: u32 = 0x100;
/// Right arrow.
pub const RIGHT: u32 = 0x101;
/// Up arrow.
pub const UP: u32 = 0x102;
/// Down arrow.
pub const DOWN: u32 = 0x103;
/// Either shift.
pub const SHIFT: u32 = 0x104;
/// Either control.
pub const CONTROL: u32 = 0x105;
/// Either alt or option.
pub const ALT: u32 = 0x106;
/// Page up.
pub const PAGE_UP: u32 = 0x107;
/// Page down.
pub const PAGE_DOWN: u32 = 0x108;
/// Home.
pub const HOME: u32 = 0x109;
/// End.
pub const END: u32 = 0x10A;
/// Insert.
pub const INSERT: u32 = 0x10B;
/// Delete.
pub const DELETE: u32 = 0x10C;

/// Function key `n`, for `n` in `1..=12`. Anything else returns [`F12`].
#[must_use]
pub const fn function(n: u32) -> u32 {
    if n >= 1 && n <= 12 { 0x120 + n } else { F12 }
}

/// F1.
pub const F1: u32 = 0x121;
/// F2.
pub const F2: u32 = 0x122;
/// F3.
pub const F3: u32 = 0x123;
/// F4.
pub const F4: u32 = 0x124;
/// F5.
pub const F5: u32 = 0x125;
/// F6.
pub const F6: u32 = 0x126;
/// F7.
pub const F7: u32 = 0x127;
/// F8: the demo's frame-dump key.
pub const F8: u32 = 0x128;
/// F9.
pub const F9: u32 = 0x129;
/// F10.
pub const F10: u32 = 0x12A;
/// F11.
pub const F11: u32 = 0x12B;
/// F12.
pub const F12: u32 = 0x12C;

/// The code for a letter, upper-cased.
#[must_use]
pub const fn letter(c: char) -> u32 {
    let c = c.to_ascii_uppercase() as u32;
    if c >= b'A' as u32 && c <= b'Z' as u32 {
        c
    } else {
        0
    }
}

/// The code for a digit `0..=9`. Anything else returns `0`.
#[must_use]
pub const fn digit(d: u32) -> u32 {
    if d <= 9 { b'0' as u32 + d } else { 0 }
}

/// A human-readable name, for the debug overlay.
#[must_use]
pub fn name(code: u32) -> &'static str {
    match code {
        ESCAPE => "escape",
        SPACE => "space",
        ENTER => "enter",
        TAB => "tab",
        BACKSPACE => "backspace",
        LEFT => "left",
        RIGHT => "right",
        UP => "up",
        DOWN => "down",
        SHIFT => "shift",
        CONTROL => "control",
        ALT => "alt",
        PAGE_UP => "page up",
        PAGE_DOWN => "page down",
        HOME => "home",
        END => "end",
        INSERT => "insert",
        DELETE => "delete",
        F1 => "f1",
        F2 => "f2",
        F3 => "f3",
        F4 => "f4",
        F5 => "f5",
        F6 => "f6",
        F7 => "f7",
        F8 => "f8",
        F9 => "f9",
        F10 => "f10",
        F11 => "f11",
        F12 => "f12",
        _ if (b'A' as u32..=b'Z' as u32).contains(&code) => "letter",
        _ if (b'0' as u32..=b'9' as u32).contains(&code) => "digit",
        _ => "unknown",
    }
}

/// True for a code the engine will recognise.
#[must_use]
pub fn is_valid(code: u32) -> bool {
    code != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_use_their_ascii_codes() {
        assert_eq!(W, 87);
        assert_eq!(A, 65);
        assert_eq!(S, 83);
        assert_eq!(D, 68);
        assert_eq!(letter('w'), W);
        assert_eq!(letter('W'), W);
    }

    #[test]
    fn digits_are_ascii() {
        assert_eq!(digit(0), 48);
        assert_eq!(digit(9), 57);
        assert_eq!(digit(10), 0);
    }

    #[test]
    fn non_printable_keys_do_not_collide_with_ascii() {
        for code in [
            LEFT, RIGHT, UP, DOWN, SHIFT, CONTROL, ALT, F1, F12, PAGE_UP, DELETE,
        ] {
            assert!(code > 0xFF, "{code} collides with an ASCII byte");
        }
    }

    #[test]
    fn function_keys_are_distinct_and_in_range() {
        let mut seen = Vec::new();
        for n in 1..=12 {
            let code = function(n);
            assert!(!seen.contains(&code), "f{n} collides");
            seen.push(code);
        }
        assert_eq!(function(1), F1);
        assert_eq!(function(12), F12);
        assert_eq!(function(0), F12, "out of range clamps");
        assert_eq!(function(99), F12);
    }

    #[test]
    fn names_are_human_readable() {
        assert_eq!(name(ESCAPE), "escape");
        assert_eq!(name(W), "letter");
        assert_eq!(name(digit(3)), "digit");
        assert_eq!(name(0xFFFF), "unknown");
    }

    #[test]
    fn validity() {
        assert!(is_valid(W));
        assert!(!is_valid(0), "zero means 'no key'");
    }
}
