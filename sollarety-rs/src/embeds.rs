//! Embeds padrao e a limpeza de texto que vai para dentro deles (Embeds.cs).
//!
//! Regra de cor do ccore: vermelho e RESERVADO para erro e recusa. Num bot que
//! bane e expulsa gente, a cor mais forte da tela tem de querer dizer algo.
//!
//! Os tamanhos aqui contam como o C# conta (`string.Length`, unidades UTF-16),
//! que e tambem como o Discord mede os limites de embed.

use poise::serenity_prelude as serenity;

/// As cores do DiscordColor do DisCatSharp que o bot usa.
pub mod colors {
    use poise::serenity_prelude::Colour;

    pub const BLURPLE: Colour = Colour::new(0x5865F2);
    pub const GOLD: Colour = Colour::new(0xFFD700);
    pub const GRAY: Colour = Colour::new(0x808080);
    pub const INDIAN_RED: Colour = Colour::new(0xCD5C5C);
    pub const ORANGE: Colour = Colour::new(0xFFA500);
    pub const SPRING_GREEN: Colour = Colour::new(0x00FF7F);
    pub const RED: Colour = Colour::new(0xED4245);
}

pub fn error(title: impl Into<String>, description: impl Into<String>) -> serenity::CreateEmbed {
    simple(title, description, colors::INDIAN_RED)
}

pub fn ok(title: impl Into<String>, description: impl Into<String>) -> serenity::CreateEmbed {
    simple(title, description, colors::SPRING_GREEN)
}

pub fn info(title: impl Into<String>, description: impl Into<String>) -> serenity::CreateEmbed {
    simple(title, description, colors::BLURPLE)
}

fn simple(
    title: impl Into<String>,
    description: impl Into<String>,
    colour: serenity::Colour,
) -> serenity::CreateEmbed {
    serenity::CreateEmbed::new()
        .title(title)
        .description(description)
        .colour(colour)
}

/// Texto de usuario indo para dentro de um EMBED. Descricao de embed
/// renderiza link mascarado (`[clique](https://…)`), coisa que mensagem comum
/// nao faz: sem isto, comando aberto vira cartao assinado pelo bot com link
/// de destino escondido.
///
/// A ORDEM E OBRIGATORIA: a barra invertida primeiro, senao `\[x](url)`
/// virava `\\[`, o Discord consumia `\\` e soltava o colchete.
pub fn safe(value: &str) -> String {
    value.replace('\\', "\\\\").replace('[', "\\[")
}

/// Escapa e SO ENTAO corta. Na ordem inversa, 900 colchetes cortados em 900
/// viravam 1800 caracteres, acima do teto de 1024 de um campo.
pub fn safe_trim(value: &str, max: usize) -> String {
    trim(&safe(value), max)
}

/// Nome de pessoa ou servidor como texto puro: escapa TODA a marcacao. Um
/// username com "_" viraria italico no meio de um ranking.
pub fn plain(value: &str, max: usize) -> String {
    let text = trim(value, max);
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if matches!(
            c,
            '\\' | '*'
                | '_'
                | '~'
                | '`'
                | '|'
                | '>'
                | '['
                | ']'
                | '('
                | ')'
                | '#'
                | '-'
                | '<'
                | '@'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// "1,250" - separador fixo, como o "N0" da cultura invariante.
pub fn number(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if value < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Tamanho como o `string.Length` do C# (unidades UTF-16).
pub fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// Corta no limite do Discord, com reticencias. Nunca parte um caractere ao
/// meio: no C# o cuidado era com o par substituto, aqui um `char` inteiro ou
/// cabe ou fica de fora - o mesmo resultado.
pub fn trim(value: &str, max: usize) -> String {
    if utf16_len(value) <= max {
        return value.to_owned();
    }

    let budget = max.saturating_sub(1);
    let mut used = 0;
    let mut out = String::new();
    for c in value.chars() {
        used += c.len_utf16();
        if used > budget {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

/// Deixa o texto seguro dentro de um bloco ```: a crase tripla fecharia o
/// bloco. O zero-width space separa as crases.
pub fn fence_safe(text: &str) -> String {
    text.replace("```", "`\u{200b}`\u{200b}`")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_escapa_a_barra_antes_do_colchete() {
        assert_eq!(safe(r"\[x](u)"), r"\\\[x](u)");
        assert_eq!(safe_trim(&"[".repeat(10), 5), "\\[\\[…");
    }

    #[test]
    fn trim_conta_como_o_csharp() {
        assert_eq!(trim("abc", 3), "abc");
        assert_eq!(trim("abcd", 3), "ab…");
        // Emoji fora do BMP vale 2: nao cabe partido.
        assert_eq!(trim("a😀b", 3), "a…");
        assert_eq!(trim("ação", 3), "aç…");
        assert_eq!(utf16_len("😀"), 2);
    }

    #[test]
    fn number_com_virgula_fixa() {
        assert_eq!(number(0), "0");
        assert_eq!(number(999), "999");
        assert_eq!(number(1250), "1,250");
        assert_eq!(number(-1234567), "-1,234,567");
        assert_eq!(number(i64::MIN), "-9,223,372,036,854,775,808");
    }

    #[test]
    fn plain_escapa_marcacao() {
        assert_eq!(plain("a_b*c", 100), r"a\_b\*c");
    }
}
