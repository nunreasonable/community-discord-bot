//! As transformacoes do /text e o codigo Morse do /morse (TextTransforms.cs).
//!
//! O C# trabalha por ELEMENTO DE TEXTO (StringInfo), nao por char, porque no
//! UTF-16 um emoji fora do BMP ou uma letra com acento combinado em separado
//! ocupam mais de uma unidade. Em Rust um `char` JA e um scalar value
//! inteiro - o caso do emoji (par substituto no C#) nao existe aqui. O que
//! falta e o caso da marca combinante separada (ex.: "e" + acento agudo
//! combinante, em vez do "e" acentuado pronto): sem o crate
//! `unicode-segmentation` (nao esta nas dependencias do port) cada `char` e
//! tratado como um elemento. Na pratica quase todo texto que chega por um
//! cliente de Discord ja vem normalizado (NFC, forma composta), entao essa
//! diferenca so aparece com entrada deliberadamente decomposta. Ver o
//! relatorio final do port para a sugestao de dependencia.

use std::collections::HashMap;
use std::sync::LazyLock;

use unicode_normalization::UnicodeNormalization;

use super::legacy_random::LegacyRandom;
use super::stable_hash::stable_hash_of;

/// ｖａｐｏｒｗａｖｅ: ASCII visivel vira a forma de largura cheia.
pub fn vaporwave(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c == ' ' {
            out.push('\u{3000}');
        } else if ('!'..='~').contains(&c) {
            // deslocamento fixo para a forma "fullwidth" do Unicode.
            out.push(char::from_u32(c as u32 + 0xFEE0).unwrap_or(c));
        } else {
            out.push(c);
        }
    }
    out
}

/// Q U A L I T Y
pub fn quality(text: &str) -> String {
    text.to_uppercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// 👏 clap 👏 between 👏 words 👏
pub fn clap(text: &str, emoji: &str) -> String {
    let words: Vec<&str> = text.split(' ').filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        emoji.to_owned()
    } else {
        format!("{emoji} {} {emoji}", words.join(&format!(" {emoji} ")))
    }
}

/// mOcKiNg TeXt. A semente vem do proprio texto (StableHash), entao o mesmo
/// texto sempre zomba do mesmo jeito - precisa do `LegacyRandom` para dar o
/// MESMO resultado que o `new Random(seed)` do C#.
pub fn mock(text: &str) -> String {
    let mut random = LegacyRandom::new(stable_hash_of(text) as i32);
    text.chars()
        .map(|c| {
            if random.next_max(2) == 0 {
                first_or(c.to_lowercase(), c)
            } else {
                first_or(c.to_uppercase(), c)
            }
        })
        .collect()
}

/// `char::to_lowercase`/`to_uppercase` devolvem um iterador porque alguns
/// mapeamentos Unicode (ex.: "ß" maiusculo em algumas regras) expandem para
/// mais de um char; o `char.ToUpperInvariant` do C# nunca expande - fica no
/// mapeamento simples. Pegar so o primeiro cobre todo caractere comum
/// (ASCII e acentuado latino) e diverge so nesses casos raros.
fn first_or(mut chars: impl Iterator<Item = char>, fallback: char) -> char {
    chars.next().unwrap_or(fallback)
}

pub fn reverse(text: &str) -> String {
    text.chars().rev().collect()
}

const FLIP_FROM: &str =
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.,!?'\"()[]{}<>&_;";
const FLIP_TO: &str =
    "ɐqɔpǝɟƃɥᴉɾʞlɯuodbɹsʇnʌʍxʎz∀ꓭƆꓷƎℲ⅁HIſꓘ˥WNOԀꝹꓤSꓕՈΛMX⅄Z0⇂ᄅƐㄣϛ9ㄥ86˙'¡¿,„)(][}{><⅋‾؛";

static FLIP_TABLE: LazyLock<HashMap<char, char>> = LazyLock::new(|| {
    FLIP_FROM
        .chars()
        .zip(FLIP_TO.chars())
        .collect::<HashMap<char, char>>()
});

/// ɐʍoɹp ǝpᴉsdn: cada letra virada, e a frase de tras para a frente.
pub fn upside_down(text: &str) -> String {
    text.chars()
        .rev()
        .map(|c| FLIP_TABLE.get(&c).copied().unwrap_or(c))
        .collect()
}

// ------------------------------------------------------------------
// Morse
// ------------------------------------------------------------------

const MORSE_PAIRS: &[(char, &str)] = &[
    ('A', ".-"),
    ('B', "-..."),
    ('C', "-.-."),
    ('D', "-.."),
    ('E', "."),
    ('F', "..-."),
    ('G', "--."),
    ('H', "...."),
    ('I', ".."),
    ('J', ".---"),
    ('K', "-.-"),
    ('L', ".-.."),
    ('M', "--"),
    ('N', "-."),
    ('O', "---"),
    ('P', ".--."),
    ('Q', "--.-"),
    ('R', ".-."),
    ('S', "..."),
    ('T', "-"),
    ('U', "..-"),
    ('V', "...-"),
    ('W', ".--"),
    ('X', "-..-"),
    ('Y', "-.--"),
    ('Z', "--.."),
    ('0', "-----"),
    ('1', ".----"),
    ('2', "..---"),
    ('3', "...--"),
    ('4', "....-"),
    ('5', "....."),
    ('6', "-...."),
    ('7', "--..."),
    ('8', "---.."),
    ('9', "----."),
    ('.', ".-.-.-"),
    (',', "--..--"),
    ('?', "..--.."),
    ('\'', ".----."),
    ('!', "-.-.--"),
    ('/', "-..-."),
    ('(', "-.--."),
    (')', "-.--.-"),
    ('&', ".-..."),
    (':', "---..."),
    (';', "-.-.-."),
    ('=', "-...-"),
    ('+', ".-.-."),
    ('-', "-....-"),
    ('_', "..--.-"),
    ('"', ".-..-."),
    ('$', "...-..-"),
    ('@', ".--.-."),
];

static MORSE: LazyLock<HashMap<char, &'static str>> =
    LazyLock::new(|| MORSE_PAIRS.iter().copied().collect());

static MORSE_BACK: LazyLock<HashMap<&'static str, char>> =
    LazyLock::new(|| MORSE_PAIRS.iter().map(|&(c, code)| (code, c)).collect());

/// Letras separadas por espaco, palavras por " / ". Devolve tambem o que nao
/// teve codigo (em ordem alfabetica, sem repetir - como o `SortedSet` do C#).
pub fn morse_encode(text: &str) -> (String, Vec<String>) {
    let plain = remove_diacritics(text).to_uppercase();

    let mut unknown = std::collections::BTreeSet::new();
    let mut words = Vec::new();

    for word in plain.split(' ').filter(|w| !w.is_empty()) {
        let mut letters = Vec::new();
        for c in word.chars() {
            if let Some(&code) = MORSE.get(&c) {
                letters.push(code);
            } else {
                unknown.insert(c.to_string());
            }
        }
        if !letters.is_empty() {
            words.push(letters.join(" "));
        }
    }

    (words.join(" / "), unknown.into_iter().collect())
}

/// Aceita os pontos e tracos "bonitos" que teclado de celular e tradutor
/// online costumam colar no lugar de . e -.
pub fn morse_decode(code: &str) -> (String, usize) {
    let normalized = code
        .replace(['·', '•', '∙'], ".")
        .replace(['−', '–', '—', '_'], "-")
        .replace('|', " / ");

    let mut unknown = 0usize;
    let mut words = Vec::new();

    for word in normalized
        .split('/')
        .map(str::trim)
        .filter(|w| !w.is_empty())
    {
        let mut decoded = String::new();
        for letter in word.split(' ').filter(|l| !l.is_empty()) {
            match MORSE_BACK.get(letter) {
                Some(&c) => decoded.push(c),
                None => {
                    decoded.push('\u{FFFD}');
                    unknown += 1;
                }
            }
        }
        if !decoded.is_empty() {
            words.push(decoded);
        }
    }

    (words.join(" "), unknown)
}

/// Remove acento: "é" vira "E" em vez de ser descartado. NFD separa a letra
/// da marca, filtra a marca, e NFC recompoe o que restou.
///
/// `is_combining_mark` cobre a categoria Unicode "Mark" inteira (Mn+Mc+Me),
/// um pouco mais ampla que o `NonSpacingMark` (so Mn) que o C# filtra; para
/// acentos latinos decompostos (o caso real de uso aqui) o resultado e
/// identico, porque a marca decomposta sempre cai em Mn.
fn remove_diacritics(text: &str) -> String {
    text.nfd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .nfc()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vaporwave_larga_so_o_imprimivel() {
        assert_eq!(vaporwave("ab c"), "ａｂ\u{3000}ｃ");
    }

    #[test]
    fn quality_maiusculo_sem_espaco() {
        assert_eq!(quality("ab c"), "A B C");
        assert_eq!(quality(""), "");
    }

    #[test]
    fn clap_entre_palavras() {
        assert_eq!(clap("a b c", "👏"), "👏 a 👏 b 👏 c 👏");
        assert_eq!(clap("", "👏"), "👏");
        assert_eq!(clap("  a  b  ", "x"), "x a x b x");
    }

    #[test]
    fn mock_e_deterministico_e_bate_com_o_csharp() {
        assert_eq!(mock("hello world"), "helLO WoRLd");
        assert_eq!(mock("Sollarety is cool"), "sOlLARety IS COOL");
        assert_eq!(mock("a"), "a");
        assert_eq!(mock("The Quick Brown Fox"), "The QUicK brOwn FOX");
        // Mesmo texto, mesma saida sempre.
        assert_eq!(mock("repete"), mock("repete"));
    }

    #[test]
    fn reverse_simples() {
        assert_eq!(reverse("abc"), "cba");
    }

    #[test]
    fn upside_down_vira_letra_e_frase() {
        assert_eq!(upside_down("abc"), "ɔqɐ");
        // Caracter sem mapeamento passa direto (so a ordem inverte).
        assert_eq!(upside_down("a€b"), "q€ɐ");
    }

    #[test]
    fn morse_encode_decode_ida_e_volta() {
        let (code, unknown) = morse_encode("SOS");
        assert_eq!(code, "... --- ...");
        assert!(unknown.is_empty());

        let (text, bad) = morse_decode("... --- ...");
        assert_eq!(text, "SOS");
        assert_eq!(bad, 0);
    }

    #[test]
    fn morse_encode_separa_palavras_e_ignora_acento() {
        let (code, unknown) = morse_encode("sao jose");
        assert!(code.contains(" / "));
        assert!(unknown.is_empty());
    }

    #[test]
    fn morse_decode_aceita_pontuacao_alternativa_e_marca_invalido() {
        // "." fica no lugar de "·" e "-" no lugar de "–": ".-." e R valido;
        // "zzz" nao e codigo Morse nenhum e vira um char de substituicao.
        let (text, bad) = morse_decode("·–· zzz");
        assert_eq!(text, "R\u{FFFD}");
        assert_eq!(bad, 1);
    }
}
