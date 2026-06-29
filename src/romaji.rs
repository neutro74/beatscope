//! Japanese kana → rōmaji transliteration (Hepburn-ish).
//!
//! Handles hiragana and katakana, small ya/yu/yo digraphs, the sokuon (っ/ッ)
//! consonant doubling, and the katakana long-vowel mark (ー). Kanji and other
//! scripts pass through unchanged — full kanji readings need a dictionary, which
//! is out of scope, so this gives pronunciation for the kana portions.

fn to_hiragana(c: char) -> char {
    let u = c as u32;
    if (0x30A1..=0x30F6).contains(&u) {
        char::from_u32(u - 0x60).unwrap_or(c)
    } else {
        c
    }
}

fn base(c: char) -> Option<&'static str> {
    Some(match c {
        'あ' => "a", 'い' => "i", 'う' => "u", 'え' => "e", 'お' => "o",
        'か' => "ka", 'き' => "ki", 'く' => "ku", 'け' => "ke", 'こ' => "ko",
        'が' => "ga", 'ぎ' => "gi", 'ぐ' => "gu", 'げ' => "ge", 'ご' => "go",
        'さ' => "sa", 'し' => "shi", 'す' => "su", 'せ' => "se", 'そ' => "so",
        'ざ' => "za", 'じ' => "ji", 'ず' => "zu", 'ぜ' => "ze", 'ぞ' => "zo",
        'た' => "ta", 'ち' => "chi", 'つ' => "tsu", 'て' => "te", 'と' => "to",
        'だ' => "da", 'ぢ' => "ji", 'づ' => "zu", 'で' => "de", 'ど' => "do",
        'な' => "na", 'に' => "ni", 'ぬ' => "nu", 'ね' => "ne", 'の' => "no",
        'は' => "ha", 'ひ' => "hi", 'ふ' => "fu", 'へ' => "he", 'ほ' => "ho",
        'ば' => "ba", 'び' => "bi", 'ぶ' => "bu", 'べ' => "be", 'ぼ' => "bo",
        'ぱ' => "pa", 'ぴ' => "pi", 'ぷ' => "pu", 'ぺ' => "pe", 'ぽ' => "po",
        'ま' => "ma", 'み' => "mi", 'む' => "mu", 'め' => "me", 'も' => "mo",
        'や' => "ya", 'ゆ' => "yu", 'よ' => "yo",
        'ら' => "ra", 'り' => "ri", 'る' => "ru", 'れ' => "re", 'ろ' => "ro",
        'わ' => "wa", 'ゐ' => "wi", 'ゑ' => "we", 'を' => "wo", 'ん' => "n",
        'ゔ' => "vu",
        'ぁ' => "a", 'ぃ' => "i", 'ぅ' => "u", 'ぇ' => "e", 'ぉ' => "o",
        _ => return None,
    })
}

/// Consonant cluster used when forming a digraph with small ya/yu/yo. The
/// cluster already carries the glide ("ky", "sh", "ch", "j", …), so only the
/// vowel is appended: き+ゃ → kya, し+ゃ → sha, じ+ゃ → ja.
fn digraph(c: char, small: char) -> Option<String> {
    let cluster = match c {
        'き' => "ky", 'ぎ' => "gy",
        'し' => "sh", 'じ' | 'ぢ' => "j",
        'ち' => "ch",
        'に' => "ny", 'ひ' => "hy", 'び' => "by",
        'ぴ' => "py", 'み' => "my", 'り' => "ry",
        _ => return None,
    };
    let vowel = match small {
        'ゃ' => "a", 'ゅ' => "u", 'ょ' => "o",
        _ => return None,
    };
    Some(format!("{cluster}{vowel}"))
}

fn is_small_y(c: char) -> bool {
    matches!(c, 'ゃ' | 'ゅ' | 'ょ' | 'ャ' | 'ュ' | 'ョ')
}

/// A "word" character — kana or kanji — used to spot grammatical particles.
fn is_word_char(c: char) -> bool {
    let u = c as u32;
    (0x3041..=0x3096).contains(&u)      // hiragana
        || (0x30A1..=0x30FA).contains(&u) // katakana
        || (0x3400..=0x4DBF).contains(&u) // CJK ext-A
        || (0x4E00..=0x9FFF).contains(&u) // CJK unified
}

/// Pronunciation override for the grammatical particles は/へ/を, which are read
/// differently from their dictionary syllable (e.g. こんにちは → "konnichiwa",
/// not "konnichiha"). Particles are hiragana and attach to a preceding word, so
/// we treat は/へ as the particle when they follow a word character; を is only
/// ever the object particle, so it's always "o".
fn particle_reading(raw: char, chars: &[char], i: usize) -> Option<&'static str> {
    let after_word = i > 0 && is_word_char(chars[i - 1]);
    match raw {
        'を' => Some("o"),
        'は' if after_word => Some("wa"),
        'へ' if after_word => Some("e"),
        _ => None,
    }
}

/// True if the string contains any kana worth romanizing.
pub fn has_kana(s: &str) -> bool {
    s.chars().any(|c| {
        let u = c as u32;
        (0x3041..=0x3096).contains(&u) || (0x30A1..=0x30FA).contains(&u)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_hiragana() {
        assert_eq!(romanize("こんにちは"), "konnichiwa");
        assert_eq!(romanize("ありがとう"), "arigatou");
    }

    #[test]
    fn particles_pronounced() {
        // は as the topic particle is read "wa"; word-initial は stays "ha".
        assert_eq!(romanize("わたしは"), "watashiwa");
        assert_eq!(romanize("はな"), "hana");
        // へ as the direction particle is read "e".
        assert_eq!(romanize("そらへ"), "sorae");
        // を is always the object particle, read "o".
        assert_eq!(romanize("すしを"), "sushio");
    }

    #[test]
    fn sokuon_and_digraph() {
        assert_eq!(romanize("がっこう"), "gakkou");
        assert_eq!(romanize("きゃ"), "kya");
        assert_eq!(romanize("しゃ"), "sha");
    }

    #[test]
    fn katakana_and_long_vowel() {
        assert_eq!(romanize("シャツ"), "shatsu");
        assert_eq!(romanize("コーヒー"), "koohii");
    }

    #[test]
    fn passthrough_non_kana() {
        // Kanji stays; kana around it converts.
        assert_eq!(romanize("彼が"), "彼ga");
        assert!(has_kana("彼が"));
        assert!(!has_kana("hello"));
    }
}

/// Romanize only if the result is fully Latin (i.e. the line was pure kana).
/// Mixed kanji+kana lines return `None` so we wait for the network reading
/// instead of showing an ugly half-converted string.
pub fn romanize_if_clean(s: &str) -> Option<String> {
    if !has_kana(s) {
        return None;
    }
    let r = romanize(s);
    if r.chars().all(|c| c.is_ascii() || c.is_whitespace()) {
        Some(r)
    } else {
        None
    }
}

pub fn romanize(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut sokuon = false;

    let push = |out: &mut String, sokuon: &mut bool, r: &str| {
        if *sokuon {
            if let Some(first) = r.chars().next() {
                if !"aeiou".contains(first) {
                    out.push(first);
                }
            }
            *sokuon = false;
        }
        out.push_str(r);
    };

    while i < chars.len() {
        let raw = chars[i];
        let c = to_hiragana(raw);

        // Digraph with following small ya/yu/yo.
        if i + 1 < chars.len() && is_small_y(chars[i + 1]) {
            let sm = to_hiragana(chars[i + 1]);
            if let Some(d) = digraph(c, sm) {
                push(&mut out, &mut sokuon, &d);
                i += 2;
                continue;
            }
        }
        // Sokuon: double the next consonant.
        if c == 'っ' {
            sokuon = true;
            i += 1;
            continue;
        }
        // Long vowel mark: repeat the previous vowel.
        if raw == 'ー' {
            if let Some(v) = out.chars().last().filter(|v| "aeiou".contains(*v)) {
                out.push(v);
            }
            i += 1;
            continue;
        }
        if let Some(r) = base(c) {
            // Read は/へ/を as particles when context calls for it (pronunciation,
            // not literal kana spelling).
            let r = particle_reading(raw, &chars, i).unwrap_or(r);
            push(&mut out, &mut sokuon, r);
        } else {
            sokuon = false;
            out.push(raw); // kanji / latin / punctuation
        }
        i += 1;
    }
    out
}
