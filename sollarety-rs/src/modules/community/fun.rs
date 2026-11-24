//! Comandos de diversao: /8ball, /roll, /coinflip, /choose, /avatar, /ship,
//! /rate, /cancel e /say (Fun.cs).
//!
//! Todos tem cooldown por pessoa: e o que impede um comando leve de virar
//! ferramenta de flood num servidor grande.

use chrono::Datelike;
use poise::{CreateReply, serenity_prelude as serenity};
use rand::Rng;
use serenity::Mentionable;

use super::{common, dice, stable_hash::stable_hash_of};
use crate::{Context, Data, Error, embeds, scope};

const EIGHT_BALL: &[&str] = &[
    "It is certain.",
    "It is decidedly so.",
    "Without a doubt.",
    "Yes, definitely.",
    "You may rely on it.",
    "As I see it, yes.",
    "Most likely.",
    "Signs point to yes.",
    "Reply hazy, try again.",
    "Ask again later.",
    "Better not tell you now.",
    "Cannot predict now.",
    "Concentrate and ask again.",
    "Don't count on it.",
    "My reply is no.",
    "My sources say no.",
    "Outlook not so good.",
    "Very doubtful.",
];

/// Ask the magic 8-ball a question
#[poise::command(
    slash_command,
    rename = "8ball",
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn eight_ball(
    ctx: Context<'_>,
    #[description = "What you want to know"] question: String,
) -> Result<(), Error> {
    let answer = EIGHT_BALL[rand::rng().random_range(0..EIGHT_BALL.len())];

    let embed = serenity::CreateEmbed::new()
        .title("🎱 Magic 8-ball")
        .field("Question", embeds::safe_trim(&question, 900), false)
        .field("Answer", answer, false)
        .colour(embeds::colors::BLURPLE);
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Roll dice in NdM format
#[poise::command(
    slash_command,
    rename = "roll",
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn roll(
    ctx: Context<'_>,
    #[description = "E.g. 2d6, d20, 4d10"] dice: Option<String>,
) -> Result<(), Error> {
    let input = dice.unwrap_or_else(|| "1d6".to_owned());
    let result = match dice::try_roll(&input) {
        Ok(r) => r,
        Err(cause) => {
            ctx.send(
                CreateReply::default()
                    .embed(embeds::error("Invalid dice", cause))
                    .ephemeral(true),
            )
            .await?;
            return Ok(());
        }
    };

    // Com muitos dados a lista estoura o embed; acima de 20 so o total
    // interessa mesmo.
    let detail = if result.rolls.len() <= 20 {
        result
            .rolls
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(" + ")
    } else {
        format!("{} dice", result.rolls.len())
    };

    let embed = serenity::CreateEmbed::new()
        .title(format!("🎲 {}d{}", result.count, result.sides))
        .description(format!("{detail}\n\n**Total: {}**", result.total))
        .colour(embeds::colors::BLURPLE);
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Flip a coin
#[poise::command(
    slash_command,
    rename = "coinflip",
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn coinflip(ctx: Context<'_>) -> Result<(), Error> {
    let heads = rand::rng().random_range(0..2) == 0;
    let embed = serenity::CreateEmbed::new()
        .title(if heads { "🪙 Heads" } else { "🪙 Tails" })
        .colour(embeds::colors::GOLD);
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Pick one of the options you give
#[poise::command(
    slash_command,
    rename = "choose",
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn choose(
    ctx: Context<'_>,
    #[description = "Separated by commas or semicolons"] options: String,
) -> Result<(), Error> {
    // Aceita os dois separadores: quem escreve uma lista costuma usar
    // virgula, mas a opcao em si pode conter virgula.
    let separator = if options.contains(';') { ';' } else { ',' };

    let parsed: Vec<&str> = options
        .split(separator)
        .map(str::trim)
        .filter(|o| !o.is_empty())
        .collect();

    if parsed.len() < 2 {
        ctx.send(
            CreateReply::default()
                .embed(embeds::error(
                    "Not enough options",
                    "Give at least two options, separated by commas. E.g. `pizza, sushi, burgers`",
                ))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let picked = parsed[rand::rng().random_range(0..parsed.len())];
    let embed = serenity::CreateEmbed::new()
        .title("🤔 I pick")
        .description(format!("**{}**", embeds::safe_trim(picked, 200)))
        .footer(serenity::CreateEmbedFooter::new(format!(
            "out of {} options",
            parsed.len()
        )))
        .colour(embeds::colors::BLURPLE);
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Show a user's avatar at full size
#[poise::command(
    slash_command,
    rename = "avatar",
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn avatar(
    ctx: Context<'_>,
    #[description = "Whose avatar to show"] user: Option<serenity::User>,
) -> Result<(), Error> {
    let target = user.unwrap_or_else(|| ctx.author().clone());
    let url = full_size(&target.face());

    let embed = serenity::CreateEmbed::new()
        .title(format!("{}'s avatar", target.tag()))
        .image(url.clone())
        .url(url)
        .colour(embeds::colors::BLURPLE);
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Acrescenta ?size=1024, como o `GetAvatarUrl(MediaFormat.Auto, 1024)` do
/// DisCatSharp. O CDN do Discord aceita o parametro em qualquer URL de
/// avatar/icone.
fn full_size(url: &str) -> String {
    if url.contains('?') {
        format!("{url}&size=1024")
    } else {
        format!("{url}?size=1024")
    }
}

// ------------------------------------------------------------------
// /ship, /rate e /cancel
// ------------------------------------------------------------------

const SHIP_PERFECT: &[&str] = &[
    "They're perfect for each other! 💞",
    "A match made in heaven. 💍",
];
const SHIP_HIGH: &[&str] = &[
    "They were made for each other! 💕",
    "Something's definitely there. 😍",
    "Wedding bells might be ringing soon. 🔔",
];
const SHIP_MID: &[&str] = &[
    "If they stopped being so shy, maybe it could work. 🤔",
    "Could go either way. 🤷",
    "Worth a shot, maybe?",
];
const SHIP_LOW: &[&str] = &[
    "Nah, sadly it wouldn't work… 😢",
    "Better as friends. 🤝",
    "The stars say no. 🌧️",
];
const SHIP_FRIENDZONE: &[&str] = &[
    "I like you, but only as a friend. 😅",
    "Sorry, I'm married to my job. 🤖",
    "You're sweet, but I'm just a bot. 💾",
];
const SHIP_NONE: &[&str] = &["No. Just no. 💔"];

/// See how compatible two people are
#[poise::command(
    slash_command,
    rename = "ship",
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn ship(
    ctx: Context<'_>,
    #[description = "The first person"] user: serenity::User,
    #[description = "The second person (default: you)"] other: Option<serenity::User>,
) -> Result<(), Error> {
    let a = user;
    let b = other.unwrap_or_else(|| ctx.author().clone());
    let bot_id = ctx.cache().current_user().id;

    // Soma dos ids: simetrica (A+B = B+A) e fixa, como na Loritta - o mesmo
    // casal sempre da o mesmo numero.
    let seed = a.id.get().wrapping_add(b.id.get());

    let (percent, line): (i32, &str) = if a.id == b.id {
        (100, "Self-love is the best love. 💖")
    } else if a.id == bot_id || b.id == bot_id {
        let percent = (seed % 51) as i32;
        (
            percent,
            SHIP_FRIENDZONE[(seed % SHIP_FRIENDZONE.len() as u64) as usize],
        )
    } else {
        let percent = (seed % 101) as i32;
        let tier: &[&str] = match percent {
            100 => SHIP_PERFECT,
            67..=99 => SHIP_HIGH,
            34..=66 => SHIP_MID,
            1..=33 => SHIP_LOW,
            _ => SHIP_NONE,
        };
        (percent, tier[(seed / 101 % tier.len() as u64) as usize])
    };

    // Metade de cada nome por char, e nao por byte: cortar um emoji do nome
    // ao meio deixaria um "?" no nome do casal.
    let name_a: Vec<char> = ship_display_name(&a).chars().collect();
    let name_b: Vec<char> = ship_display_name(&b).chars().collect();
    let take_a = name_a.len().div_ceil(2);
    let skip_b = name_b.len() / 2;
    let ship_name: String = name_a[..take_a]
        .iter()
        .chain(name_b[skip_b..].iter())
        .collect();

    let filled = common::round_half_even(f64::from(percent) / 10.0) as i64;
    let filled = filled.clamp(0, 10) as usize;
    let bar = "█".repeat(filled) + &"░".repeat(10 - filled);

    let color = if percent >= 67 {
        serenity::Colour::new(0xFF6FA8)
    } else if percent >= 34 {
        embeds::colors::GOLD
    } else {
        embeds::colors::GRAY
    };

    let embed = serenity::CreateEmbed::new()
        .title("💘 Matchmaking")
        .description(format!(
            "{} + {} = ✨**{}**✨\n\n`{bar}` **{percent}%**\n{line}",
            a.mention(),
            b.mention(),
            embeds::safe_trim(&ship_name, 64)
        ))
        .colour(color);
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

fn ship_display_name(user: &serenity::User) -> String {
    let name = common::global_name(user);
    if name.is_empty() {
        "?".to_owned()
    } else {
        name
    }
}

const RATE_REASONS: &[&str] = &[
    "Please, just no. 🗑️",
    "Yikes. Hard pass. 😬",
    "I've seen better. Much better. 😐",
    "Not great, not terrible. Mostly not great. 🫤",
    "It has its moments. Few of them. 🙃",
    "Perfectly average. 🤷",
    "Pretty decent, actually. 🙂",
    "I like it! 👍",
    "Really good stuff. 😄",
    "Almost perfect! ✨",
    "Simply perfect. No notes. 🏆",
];

/// I'll rate anything from 0 to 10
#[poise::command(
    slash_command,
    rename = "rate",
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn rate(
    ctx: Context<'_>,
    #[description = "What should I rate?"]
    #[max_length = 200]
    thing: String,
) -> Result<(), Error> {
    let key = thing.trim().to_lowercase();
    let bot_id = ctx.cache().current_user().id.get();

    let (score, reason) = if matches!(key.as_str(), "sollarety" | "you" | "yourself")
        || key == format!("<@{bot_id}>")
        || key == format!("<@!{bot_id}>")
    {
        ("∞".to_owned(), "Obviously. 😌")
    } else {
        // Muda uma vez por dia, como na Loritta: a mesma coisa tem a mesma
        // nota o dia inteiro, e insistir no comando nao sobe a nota.
        let today = chrono::Utc::now();
        let seed = format!("{key}|{}-{}", today.year(), today.ordinal());
        let value = (stable_hash_of(&seed) % 11) as usize;
        (format!("{value}/10"), RATE_REASONS[value])
    };

    let embed = serenity::CreateEmbed::new()
        .description(format!(
            "🤔 I rate **{}** a **{score}**!\n*{reason}*",
            embeds::safe_trim(&thing, 200)
        ))
        .colour(embeds::colors::BLURPLE);
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

const CANCEL_REASONS: &[&str] = &[
    "putting pineapple on pizza",
    "clapping when the plane landed",
    "replying \"k\" to a paragraph",
    "leaving someone on read for three business days",
    "using light mode at 3 AM",
    "saying \"it's giving\" unironically",
    "microwaving fish in the office",
    "spoiling the season finale",
    "not using their turn signal",
    "putting milk before cereal",
    "reheating coffee four times",
    "laughing at their own jokes before the punchline",
    "pronouncing GIF wrong (either way)",
    "having 4,000 unread emails",
    "skipping the tutorial and then asking for help",
    "calling a hot dog a sandwich",
    "saying \"no offense\" and then offending",
    "starting a sentence with \"well, actually\"",
    "never muting their mic",
    "breathing into the mic in voice chat",
    "pinging @everyone for a meme",
    "rating their own music 10/10",
    "eating the last slice without asking",
    "posting \"first\" in the comments",
    "spelling it \"definately\"",
];

/// Cancel someone on the internet (it's a joke)
#[poise::command(
    slash_command,
    rename = "cancel",
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn cancel(
    ctx: Context<'_>,
    #[description = "Who gets cancelled"] user: serenity::User,
) -> Result<(), Error> {
    let reason = CANCEL_REASONS[rand::rng().random_range(0..CANCEL_REASONS.len())];
    let embed = serenity::CreateEmbed::new()
        .description(format!(
            "📢 {} was **cancelled** for {reason}.",
            user.mention()
        ))
        .footer(serenity::CreateEmbedFooter::new(
            "It's a joke — nobody actually got cancelled.",
        ))
        .colour(embeds::colors::ORANGE);
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Make the bot say something
#[poise::command(
    slash_command,
    rename = "say",
    guild_only,
    default_member_permissions = "MANAGE_MESSAGES",
    required_permissions = "MANAGE_MESSAGES",
    required_bot_permissions = "SEND_MESSAGES",
    check = "crate::cooldown::user::<2, 15>"
)]
pub async fn say(
    ctx: Context<'_>,
    #[description = "What the bot should say"] text: String,
) -> Result<(), Error> {
    // Defer ANTES de publicar: publicando primeiro, se o bot nao tivesse
    // permissao de escrever no canal a interacao ficava sem resposta.
    ctx.defer_ephemeral().await?;

    /*
     * Quem usa precisa poder falar NESTE canal. ManageMessages (ja exigido
     * acima) nao e SendMessages: sem esta checagem, um moderador barrado de
     * escrever num canal especifico continuava falando la pela boca do bot.
     *
     * Numa THREAD isto precisa olhar o canal PAI: a permissao de thread nao
     * tem overwrite proprio e cai para a do servidor se nao conferir o pai.
     */
    let guild_id = ctx.guild_id().expect("guild_only garante o servidor");

    let has_permission = check_can_speak_here(ctx).await;
    if !has_permission {
        ctx.send(CreateReply::default().embed(embeds::error(
            "No permission in this channel",
            "You can't send messages here, so you can't do it through me either.",
        )))
        .await?;
        return Ok(());
    }

    // Sem mencao de cargo nem de @everyone: o /say seria um jeito de
    // contornar quem pode mencionar todo mundo.
    let message = serenity::CreateMessage::new()
        .content(embeds::trim(&text, 1900))
        .allowed_mentions(serenity::CreateAllowedMentions::new());

    if let Err(cause) = ctx.channel_id().send_message(ctx, message).await {
        ctx.send(CreateReply::default().embed(embeds::error(
            "Couldn't post the message",
            embeds::trim(&cause.to_string(), 500),
        )))
        .await?;
        return Ok(());
    }

    ctx.send(
        CreateReply::default().embed(embeds::ok("Sent", "The message was posted in the channel.")),
    )
    .await?;

    // Registrado como qualquer outra acao privilegiada: a resposta e
    // efemera, a mensagem sai assinada pelo bot, e o Audit Log do Discord
    // nao cobre envio de mensagem - "quem fez o bot dizer isso?" nao tinha
    // resposta sem isto.
    crate::modlog::record(
        ctx,
        &ctx.data().guild_settings,
        guild_id,
        crate::modlog::Entry::new("Message via /say", ctx.author())
            .extra(&format!(
                "{}: {}",
                ctx.channel_id().mention(),
                embeds::safe_trim(&text, 500)
            ))
            // Nao ha alvo: o rotulo padrao ("entire channel") e do /purge e
            // sairia como "User: entire channel" para uma mensagem publicada.
            .no_target_label("—"),
    )
    .await;

    Ok(())
}

/// Quem usa precisa poder falar NESTE canal. `ManageMessages` (ja exigido no
/// comando) nao e `SendMessages`: sem esta checagem, um moderador barrado de
/// escrever num canal especifico - ou num canal que outro moderador acabou
/// de travar - continuava falando la pela boca do bot.
///
/// Numa THREAD isto precisa olhar o canal PAI: o calculo de permissao do
/// serenity (igual ao PermissionsFor do DisCatSharp) so aplica os overwrites
/// do PROPRIO canal, e thread nao tem overwrite proprio - sem subir pro pai,
/// o calculo caia para a permissao de servidor e ignorava qualquer negacao
/// do canal onde a thread vive.
async fn check_can_speak_here(ctx: Context<'_>) -> bool {
    let Some(channel) = ctx.guild_channel().await else {
        return false;
    };
    let Some(member) = ctx.author_member().await else {
        return false;
    };
    let Some(guild) = ctx.guild() else {
        return false;
    };

    let in_thread = matches!(
        channel.kind,
        serenity::ChannelType::PublicThread
            | serenity::ChannelType::PrivateThread
            | serenity::ChannelType::NewsThread
    );
    let needed = if in_thread {
        serenity::Permissions::SEND_MESSAGES_IN_THREADS
    } else {
        serenity::Permissions::SEND_MESSAGES
    };

    let scope_channel = if in_thread {
        match channel.parent_id {
            Some(parent_id) => guild.channels.get(&parent_id).cloned().unwrap_or(channel),
            None => channel,
        }
    } else {
        channel
    };

    guild
        .user_permissions_in(&scope_channel, &member)
        .contains(needed)
}

pub fn commands() -> Vec<poise::Command<Data, Error>> {
    vec![
        scope::anywhere(eight_ball()),
        scope::anywhere(roll()),
        scope::anywhere(coinflip()),
        scope::anywhere(choose()),
        scope::anywhere(avatar()),
        scope::anywhere(ship()),
        scope::anywhere(rate()),
        scope::anywhere(cancel()),
        say(),
    ]
}
