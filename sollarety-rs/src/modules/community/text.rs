//! /text e /morse: transformacoes de texto no estilo do /text da Loritta
//! (Text.cs).
//!
//! A resposta sai como mensagem comum, sem embed, porque o ponto e copiar o
//! resultado. Nenhuma mencao e notificada, e o resultado passa pelo
//! `embeds::safe` - mensagem comum tambem renderiza link mascarado, e o
//! /text clap preservaria um `[clique](url)` inteiro.

use poise::{CreateReply, serenity_prelude as serenity};

use super::text_transforms as tt;
use crate::{Context, Data, Error, embeds, scope};

async fn reply(ctx: Context<'_>, result: &str) -> Result<(), Error> {
    let text = if result.trim().is_empty() {
        "*(nothing left after the transformation)*".to_owned()
    } else {
        embeds::safe_trim(result, 1990)
    };

    ctx.send(
        CreateReply::default()
            .content(text)
            .allowed_mentions(serenity::CreateAllowedMentions::new()),
    )
    .await?;
    Ok(())
}

/// Ｍａｋｅ ｉｔ ａｅｓｔｈｅｔｉｃ
#[poise::command(
    slash_command,
    rename = "vaporwave",
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn vaporwave(
    ctx: Context<'_>,
    #[description = "The text to transform"]
    #[max_length = 900]
    text: String,
) -> Result<(), Error> {
    reply(ctx, &tt::vaporwave(&text)).await
}

/// Put 👏 between 👏 every 👏 word
#[poise::command(
    slash_command,
    rename = "clap",
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn clap(
    ctx: Context<'_>,
    #[description = "The text to transform"]
    #[max_length = 900]
    text: String,
    #[description = "What goes between the words (default 👏)"]
    #[max_length = 60]
    emoji: Option<String>,
) -> Result<(), Error> {
    let emoji = emoji
        .map(|e| e.trim().to_owned())
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "👏".to_owned());
    reply(ctx, &tt::clap(&text, &emoji)).await
}

/// wRiTe LiKe ThIs
#[poise::command(
    slash_command,
    rename = "mock",
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn mock(
    ctx: Context<'_>,
    #[description = "The text to transform"]
    #[max_length = 900]
    text: String,
) -> Result<(), Error> {
    reply(ctx, &tt::mock(&text)).await
}

/// Q U A L I T Y
#[poise::command(
    slash_command,
    rename = "quality",
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn quality(
    ctx: Context<'_>,
    #[description = "The text to transform"]
    #[max_length = 900]
    text: String,
) -> Result<(), Error> {
    reply(ctx, &tt::quality(&text)).await
}

/// uʍop ǝpᴉsdn ʇᴉ uɹnꓕ
#[poise::command(
    slash_command,
    rename = "upsidedown",
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn upsidedown(
    ctx: Context<'_>,
    #[description = "The text to transform"]
    #[max_length = 900]
    text: String,
) -> Result<(), Error> {
    reply(ctx, &tt::upside_down(&text)).await
}

/// sdrawkcab ti etirW
#[poise::command(
    slash_command,
    rename = "reverse",
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn reverse(
    ctx: Context<'_>,
    #[description = "The text to transform"]
    #[max_length = 900]
    text: String,
) -> Result<(), Error> {
    reply(ctx, &tt::reverse(&text)).await
}

/// Transform some text
#[poise::command(
    slash_command,
    rename = "text",
    subcommands("vaporwave", "clap", "mock", "quality", "upsidedown", "reverse"),
    subcommand_required
)]
pub async fn text_root(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Turn text into Morse code
#[poise::command(
    slash_command,
    rename = "encode",
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn morse_encode(
    ctx: Context<'_>,
    #[description = "The text to encode"]
    #[max_length = 300]
    text: String,
) -> Result<(), Error> {
    let (code, unknown) = tt::morse_encode(&text);
    let mut result = if code.is_empty() {
        String::new()
    } else {
        format!("`{code}`")
    };
    if !unknown.is_empty() {
        result.push_str(&format!(
            "\n-# Skipped characters with no Morse code: {}",
            unknown
                .iter()
                .take(20)
                .cloned()
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    reply(ctx, &result).await
}

/// Turn Morse code back into text
#[poise::command(
    slash_command,
    rename = "decode",
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn morse_decode(
    ctx: Context<'_>,
    #[description = "Dots and dashes; letters split by spaces, words by /"]
    #[max_length = 1500]
    code: String,
) -> Result<(), Error> {
    let (text, unknown) = tt::morse_decode(&code);
    let mut result = text;
    if unknown > 0 {
        result.push_str(&format!(
            "\n-# {unknown} code(s) weren't valid Morse and show up as \u{fffd}."
        ));
    }
    reply(ctx, &result).await
}

/// Morse code translator
#[poise::command(
    slash_command,
    rename = "morse",
    subcommands("morse_encode", "morse_decode"),
    subcommand_required
)]
pub async fn morse_root(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

pub fn commands() -> Vec<poise::Command<Data, Error>> {
    vec![scope::anywhere(text_root()), scope::anywhere(morse_root())]
}
