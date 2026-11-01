//! Search that ignores the diacritics the query leaves out.

use std::sync::OnceLock;

// shortcut: Latin-1, Latin Extended-A/B and Greek tonos only (what canonical
// decomposition yields there), no Latin Extended Additional such as
// Vietnamese ạ ế; add its rows, or a normalization crate, when needed.
const MARKED: &str = "àáâãäåçèéêëìíîïñòóôõöùúûüýÿāăąćĉċčďēĕėęěĝğġģĥĩīĭįĵķĺļľńņňōŏőŕŗřśŝşšţťũūŭůűųŵŷźżžơưǎǐǒǔǖǘǚǜǟǡǣǧǩǫǭǯǰǵǹǻǽǿȁȃȅȇȉȋȍȏȑȓȕȗșțȟȧȩȫȭȯȱȳΐάέήίΰϊϋόύώϓϔ";
const BASE: &str = "aaaaaaceeeeiiiinooooouuuuyyaaaccccdeeeeegggghiiiijklllnnnooorrrssssttuuuuuuwyzzzouaiouuuuuaaægkooʒjgnaæøaaeeiioorruusthaeooooyιαεηιυιυουωϒϒ";
// A stroke or missing dot is part of the letter, not a mark on it.
const PLAIN: [(char, char); 5] = [('ł', 'l'), ('đ', 'd'), ('ø', 'o'), ('ħ', 'h'), ('ı', 'i')];

fn lower(c: char) -> char {
    match c {
        'ς' => 'σ',
        'ſ' => 's',
        _ => c.to_lowercase().next().unwrap_or(c),
    }
}

/// Case-folded, then stripped of marks where the letter is Latin or Greek.
fn fold_char(c: char) -> char {
    let l = lower(c);
    let base = MARKED.chars().zip(BASE.chars()).find(|&(m, _)| m == l).map_or(l, |(_, b)| b);
    PLAIN.iter().find(|&&(f, _)| f == base).map_or(base, |&(_, t)| t)
}

fn table() -> &'static [u16] {
    static T: OnceLock<Vec<u16>> = OnceLock::new();
    T.get_or_init(|| {
        (0..=0xFFFFu32)
            .map(|u| char::from_u32(u).map_or(u as u16, |c| fold_char(c) as u16))
            .collect()
    })
}

/// Unit by unit through the table, then each surrogate pair case-folded as the
/// one letter it encodes, which stays a pair.
fn folded(text: &[u16]) -> Vec<u16> {
    let t = table();
    let mut out: Vec<u16> = text.iter().map(|&u| t[u as usize]).collect();
    let mut i = 0;
    while i + 1 < text.len() {
        if (0xD800..0xDC00).contains(&text[i]) && (0xDC00..0xE000).contains(&text[i + 1]) {
            let cp = 0x10000 + ((text[i] as u32 - 0xD800) << 10) + (text[i + 1] as u32 - 0xDC00);
            let f = char::from_u32(cp).map_or(cp, |c| lower(c) as u32);
            if f >= 0x10000 {
                out[i] = 0xD800 + ((f - 0x10000) >> 10) as u16;
                out[i + 1] = 0xDC00 + ((f - 0x10000) & 0x3FF) as u16;
            }
            i += 1;
        }
        i += 1;
    }
    out
}

/// Romanian is written with ș and ț but much text has the older ş and ţ.
fn comma_below(u: u16) -> u16 {
    match u {
        0x015F => 0x0219,
        0x0163 => 0x021B,
        u => u,
    }
}

fn case_folded(u: u16) -> u16 {
    char::from_u32(u as u32).map_or(u, |c| lower(c) as u16)
}

/// Start offsets of non-overlapping matches, each `query.len()` units long.
pub fn find_matches(text: &[u16], query: &[u16]) -> Vec<usize> {
    let len = query.len();
    let mut matches = vec![];
    if len == 0 || text.len() < len {
        return matches;
    }
    let folded_query = folded(query);
    // The query's accented letters, which the text must match as written.
    let exact: Vec<(usize, u16)> = query
        .iter()
        .enumerate()
        .filter(|&(i, &u)| !(0xD800..0xE000).contains(&u) && folded_query[i] != case_folded(u))
        .map(|(i, &u)| (i, comma_below(case_folded(u))))
        .collect();
    let folded_text = folded(text);
    let mut from = 0;
    while from + len <= text.len() {
        if folded_text[from..from + len] == folded_query[..]
            && exact.iter().all(|&(i, u)| comma_below(case_folded(text[from + i])) == u)
        {
            matches.push(from);
            from += len;
        } else {
            from += 1;
        }
    }
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn find(text: &str, query: &str) -> Vec<usize> {
        find_matches(&u(text), &u(query))
    }

    #[test]
    fn search_ignores_diacritics_the_query_leaves_out() {
        // Plain letters find their accented forms, in either case.
        assert_eq!(find("Țară și ŞTIRI", "tara"), [0]);
        assert_eq!(find("Țară și ŞTIRI", "si"), [5]);
        assert_eq!(find("Țară și ŞTIRI", "stiri"), [8]);
        assert_eq!(find("în câmp", "in camp"), [0]);
        assert_eq!(find("élève, Łódź, Straße", "eleve"), [0]);
        assert_eq!(find("élève, Łódź, Straße", "lodz"), [7]);

        // An accented letter in the query matches only itself, though the
        // cedilla and comma-below forms of s and t are one letter.
        assert_eq!(find("tara țară", "țară"), [5]);
        assert_eq!(find("tara Ţară", "țară"), [5]);
        assert_eq!(find("mama mamă", "mamă"), [5]);

        // Scripts whose marks make different letters keep them.
        assert_eq!(find("й и", "и"), [2]);
        assert_eq!(find("が か", "か"), [2]);
        assert_eq!(find("한 하", "하"), [2]);

        // Letters beyond the Basic Multilingual Plane still ignore case.
        assert_eq!(find("x 𐐀𐐁", "𐐨𐐩"), [2]);
        assert_eq!(find("x 𐐨𐐩", "𐐀𐐁"), [2]);

        // Matches do not overlap, and a near miss does not hide the next one.
        assert_eq!(find("aaa", "aa"), [0]);
        assert_eq!(find("tțara", "țara"), [1]);
    }

    #[test]
    fn greek_tonos_folds_and_degenerate_input_is_safe() {
        assert_eq!(find("Ελλάδα", "ελλαδα"), [0]);
        assert_eq!(find("abc", ""), Vec::<usize>::new());
        assert_eq!(find("ab", "abc"), Vec::<usize>::new());
        // lone surrogates match themselves and never panic
        assert_eq!(find_matches(&[0x61, 0xD800, 0x62], &[0xD800]), [1]);
        assert_eq!(find_matches(&[0xDC00, 0xD801], &[0xD801, 0xDC28]), Vec::<usize>::new());
    }
}
