fn is_word(c: char) -> bool {
    c.is_alphabetic() || c.is_numeric()
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3005 | 0x3007 | 0x3021..=0x3029 | 0x3038..=0x303B // Han marks and numerals
        | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
        | 0x20000..=0x2FA1F | 0x30000..=0x323AF // Han
        | 0x3041..=0x3096 | 0x309D..=0x309F | 0x1B001..=0x1B11F | 0x1B150..=0x1B152 // Hiragana
        | 0x30A1..=0x30FA | 0x30FC..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F | 0x1B164..=0x1B167) // Katakana, incl. ー
}

// shortcut: hand-listed combining blocks, no Unicode tables in core; upgrade if a script's marks mis-split.
fn is_mark(c: char) -> bool {
    matches!(c as u32,
        0x300..=0x36F | 0x483..=0x489 | 0x591..=0x5BD | 0x64B..=0x65F | 0x1AB0..=0x1AFF
        | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0x3099 | 0x309A | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F)
}

/// Each Han/kana character is a word; other scripts count runs of letters and
/// digits, joined across a single `'` or `-` between them. A combining mark
/// joins the character before it.
pub fn count_words(text: &[u16]) -> usize {
    let chars: Vec<char> = char::decode_utf16(text.iter().copied())
        .map(|r| r.unwrap_or('\u{fffd}'))
        .collect();
    let (mut count, mut in_word) = (0, false);
    for (i, &c) in chars.iter().enumerate() {
        if is_word(c) && is_cjk(c) {
            count += 1;
            in_word = false;
        } else if is_word(c) {
            if !in_word {
                count += 1;
                in_word = true;
            }
        } else if is_mark(c) {
            // joins the previous character: no change
        } else if matches!(c, '\'' | '-')
            && in_word
            && chars.get(i + 1).is_some_and(|&n| is_word(n) && !is_cjk(n))
        {
            // joiner inside a word
        } else {
            in_word = false;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn counts_words() {
        let cases: &[(&str, usize)] = &[
            ("one two-three don't 42", 4),
            ("你好世界", 4),
            ("你好 世界", 4),
            ("中文 iPad AI", 4),
            ("foo中bar", 3),
            ("日本語テスト", 6),
            ("你好，世界。", 4),
            ("コーヒー", 4),
            ("か\u{3099}", 1), // が, decomposed
            ("𠀀𠀁 foo", 3),
            ("안녕하세요 세계", 2),
            ("", 0),
        ];
        for (s, n) in cases {
            assert_eq!(count_words(&u(s)), *n, "{s:?}");
        }
    }

    #[test]
    fn combining_mark_joins_preceding_character() {
        assert_eq!(count_words(&u("e\u{301}a")), 1);
        assert_eq!(count_words(&u("cafe\u{301} bar")), 2);
    }

    #[test]
    fn lone_surrogate_does_not_panic() {
        assert_eq!(count_words(&[0x61, 0xd800, 0x62]), 2);
    }
}
