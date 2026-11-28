//! Niveis por XP: ganho por mensagem (com cooldown de 1 minuto por pessoa
//! por servidor), a DM de level up com os botoes "lvl:" e os comandos
//! /rank, /level-dms e /xp give|take (Levels.cs, Services/Levels/*.cs).
//!
//! Desligado por padrao em cada servidor: so liga quem tem Manage Server,
//! com /config leveling (modulo moderation). Aqui so se le a flag.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use poise::{CreateReply, serenity_prelude as serenity};
use serenity::Mentionable;

use super::common;
use crate::{Context, Data, Error, embeds, scope, store::GuildSettingsStore};

/// O prefixo dos botoes da DM de level up, e os dois ids inteiros.
const PREFIX: &str = "lvl:";
const DM_YES: &str = "lvl:dm:yes";
const DM_NO: &str = "lvl:dm:no";

/// Teto do laco de `progress`: um XP absurdo no arquivo nao pode virar laco
/// sem fim.
const MAX_LEVEL: i32 = 10_000;

// ------------------------------------------------------------------
// LevelMath
// ------------------------------------------------------------------

/// A curva de nivel do MEE6: do nivel L para o L+1 sao 5L² + 50L + 100 XP. O
/// nivel 1 sai com 100 XP, o 10 com 4.675, o 50 com 268.375.
pub fn xp_to_next(level: i32) -> i64 {
    5i64 * i64::from(level) * i64::from(level) + 50i64 * i64::from(level) + 100
}

pub struct Progress {
    pub level: i32,
    pub into: i64,
    pub needed: i64,
}

/// Nivel, XP ja feito dentro dele e quanto o nivel inteiro pede.
pub fn progress(total_xp: i64) -> Progress {
    let mut level = 0i32;
    let mut remaining = total_xp.max(0);

    while level < MAX_LEVEL && remaining >= xp_to_next(level) {
        remaining -= xp_to_next(level);
        level += 1;
    }

    Progress {
        level,
        into: remaining,
        needed: xp_to_next(level),
    }
}

pub fn level_of(total_xp: i64) -> i32 {
    progress(total_xp).level
}

fn leveling_enabled(data: &Data, guild_id: u64) -> bool {
    guild_leveling_enabled(&data.guild_settings, guild_id)
}

/// Assinatura separada para o `global_board`, que recebe so o fechamento.
fn guild_leveling_enabled(settings: &GuildSettingsStore, guild_id: u64) -> bool {
    settings
        .for_guild(guild_id)
        .map(|s| s.leveling_enabled)
        .unwrap_or(false)
}

fn leveling_off() -> serenity::CreateEmbed {
    embeds::error(
        "Levels are off in this server",
        "Someone with **Manage Server** can turn them on with `/config leveling`.",
    )
}

fn bots_dont_level() -> serenity::CreateEmbed {
    embeds::error("Bots don't level up", "Only people earn XP.")
}

/// Levels.ProgressLine: barra de progresso em blocos.
pub fn progress_line(xp: i64) -> String {
    let p = progress(xp);
    let filled = if p.needed <= 0 {
        0
    } else {
        common::round_half_even(12.0 * p.into as f64 / p.needed as f64) as i64
    };
    let filled = filled.clamp(0, 12) as usize;
    let bar = "█".repeat(filled) + &"░".repeat(12 - filled);
    format!(
        "**Level {}** · {}/{} XP\n`{bar}`",
        p.level,
        embeds::number(p.into),
        embeds::number(p.needed)
    )
}

async fn ephemeral(ctx: Context<'_>, embed: serenity::CreateEmbed) -> Result<(), Error> {
    ctx.send(CreateReply::default().embed(embed).ephemeral(true))
        .await?;
    Ok(())
}

// ------------------------------------------------------------------
// /rank
// ------------------------------------------------------------------

/// Show someone's level and XP in this server
#[poise::command(
    slash_command,
    rename = "rank",
    guild_only,
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn rank(
    ctx: Context<'_>,
    #[description = "Whose rank to show (default: you)"] user: Option<serenity::User>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild_only garante o servidor");
    if !leveling_enabled(ctx.data(), guild_id.get()) {
        return ephemeral(ctx, leveling_off()).await;
    }

    let target = user.unwrap_or_else(|| ctx.author().clone());
    if target.bot {
        return ephemeral(ctx, bots_dont_level()).await;
    }

    let board = ctx.data().levels.guild_board(guild_id.get());
    let xp = ctx.data().levels.xp_of(guild_id.get(), target.id.get());
    let position = board.iter().position(|&(u, _)| u == target.id.get());

    let rank_line = match position {
        Some(p) => format!(
            "Rank **#{}** of {} · {} XP in total",
            p + 1,
            board.len(),
            embeds::number(xp)
        ),
        None => "Not ranked yet — chat a little to earn XP.".to_owned(),
    };

    let guild_name = ctx.guild().map(|g| g.name.clone()).unwrap_or_default();
    let name = common::display_name(ctx, Some(guild_id), &target).await;

    let mut author =
        serenity::CreateEmbedAuthor::new(embeds::trim(&format!("{name} — {guild_name}"), 256));
    if let Some(icon) = target.avatar_url() {
        author = author.icon_url(icon);
    }

    let embed = serenity::CreateEmbed::new()
        .author(author)
        .description(format!("{}\n{rank_line}", progress_line(xp)))
        .colour(embeds::colors::GOLD);

    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

// ------------------------------------------------------------------
// /level-dms
// ------------------------------------------------------------------

/// Turn the DM you get when you level up on or off
#[poise::command(
    slash_command,
    rename = "level-dms",
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn level_dms(
    ctx: Context<'_>,
    #[description = "Get a DM when you level up"] enabled: bool,
) -> Result<(), Error> {
    ctx.data().levels.set_dm(ctx.author().id.get(), enabled);

    let embed = if enabled {
        embeds::ok(
            "Level-up DMs on",
            "I'll DM you when you level up in a server that has levels on.",
        )
    } else {
        embeds::ok(
            "Level-up DMs off",
            "No more level-up DMs. Turn them back on anytime with `/level-dms`.",
        )
    };
    ephemeral(ctx, embed).await
}

// ------------------------------------------------------------------
// /xp give|take
// ------------------------------------------------------------------

/// Give or take XP
#[poise::command(
    slash_command,
    rename = "xp",
    subcommands("xp_give", "xp_take"),
    subcommand_required,
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD"
)]
pub async fn xp(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Give XP to a member
#[poise::command(
    slash_command,
    rename = "give",
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD",
    check = "crate::cooldown::user::<5, 30>"
)]
async fn xp_give(
    ctx: Context<'_>,
    #[description = "Who gets the XP"] user: serenity::User,
    #[description = "How much XP (1 to 1,000,000)"]
    #[min = 1]
    #[max = 1_000_000]
    amount: i64,
) -> Result<(), Error> {
    change_xp(ctx, user, amount.clamp(1, 1_000_000)).await
}

/// Take XP from a member
#[poise::command(
    slash_command,
    rename = "take",
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD",
    check = "crate::cooldown::user::<5, 30>"
)]
async fn xp_take(
    ctx: Context<'_>,
    #[description = "Who loses the XP"] user: serenity::User,
    #[description = "How much XP (1 to 1,000,000)"]
    #[min = 1]
    #[max = 1_000_000]
    amount: i64,
) -> Result<(), Error> {
    change_xp(ctx, user, -amount.clamp(1, 1_000_000)).await
}

async fn change_xp(ctx: Context<'_>, target: serenity::User, delta: i64) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild_only garante o servidor");

    let refusal = if !leveling_enabled(ctx.data(), guild_id.get()) {
        Some(leveling_off())
    } else if target.bot {
        Some(bots_dont_level())
    } else {
        None
    };
    if let Some(refusal) = refusal {
        return ephemeral(ctx, refusal).await;
    }

    let (old_xp, new_xp) = ctx
        .data()
        .levels
        .add_xp(guild_id.get(), target.id.get(), delta);
    let old_level = level_of(old_xp);
    let new_level = level_of(new_xp);
    let changed = new_xp - old_xp;

    let verb = if delta > 0 { "Gave" } else { "Took" };
    let level_note = if new_level == old_level {
        format!("still level {new_level}")
    } else {
        format!("level {old_level} → **{new_level}**")
    };

    let embed = embeds::ok(
        if delta > 0 { "XP given" } else { "XP taken" },
        format!(
            "{verb} **{} XP** {} {} — {level_note}, {} XP in total.",
            embeds::number(changed.abs()),
            if delta > 0 { "to" } else { "from" },
            target.mention(),
            embeds::number(new_xp)
        ),
    );
    ephemeral(ctx, embed).await?;

    // Subir de nivel por XP dado tambem avisa - quem recebe merece saber, e
    // quem desligou as DMs continua sem receber.
    if new_level > old_level {
        let http = ctx.serenity_context().http.clone();
        let guild_name = ctx.guild().map(|g| g.name.clone()).unwrap_or_default();
        let guild_icon = ctx.guild().and_then(|g| g.icon_url());
        let target = target.clone();
        let levels = ctx.data().levels.clone();
        tokio::spawn(async move {
            notify_level_up(
                &http,
                &levels,
                &guild_name,
                guild_icon,
                &target,
                new_level,
                true,
            )
            .await;
        });
    }

    crate::modlog::record(
        ctx,
        &ctx.data().guild_settings,
        guild_id,
        crate::modlog::Entry::new(
            if delta > 0 { "XP given" } else { "XP removed" },
            ctx.author(),
        )
        .target(&target)
        .extra(&format!(
            "{}{} XP — now level {new_level} ({} XP)",
            if changed >= 0 { "+" } else { "" },
            embeds::number(changed),
            embeds::number(new_xp)
        )),
    )
    .await;

    Ok(())
}

// ------------------------------------------------------------------
// Ganho de XP por mensagem
// ------------------------------------------------------------------

/// Um ganho por minuto por pessoa por servidor, como no MEE6: e o que faz
/// floodar o chat nao valer a pena.
const GAIN_COOLDOWN: Duration = Duration::from_secs(60);
const MIN_GAIN: i64 = 15;
const MAX_GAIN: i64 = 25;

static LAST_GAIN: LazyLock<Mutex<HashMap<(u64, u64), Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Trata `FullEvent::Message`: ganho de XP sincrono e sem I/O, igual ao
/// `OnMessageCreated` do C#. O aviso de level up (que pode falhar e demorar)
/// vai para um `tokio::spawn` e nao bloqueia o resto do dispatch.
pub async fn on_message(
    ctx: &serenity::Context,
    message: &serenity::Message,
    data: &Data,
) -> Result<(), Error> {
    let Some(guild_id) = message.guild_id else {
        return Ok(());
    };
    if message.author.bot || message.webhook_id.is_some() {
        return Ok(());
    }
    // Entrada no servidor, pin, boost: mensagem de sistema nao e conversa.
    if !matches!(
        message.kind,
        serenity::MessageType::Regular | serenity::MessageType::InlineReply
    ) {
        return Ok(());
    }
    if !leveling_enabled(data, guild_id.get()) {
        return Ok(());
    }

    let key = (guild_id.get(), message.author.id.get());
    let now = Instant::now();
    {
        let mut last_gain = LAST_GAIN.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&last) = last_gain.get(&key)
            && now.duration_since(last) < GAIN_COOLDOWN
        {
            return Ok(());
        }
        last_gain.insert(key, now);
        // Faxina barata: so quando o mapa cresce, como no cooldown.rs.
        if last_gain.len() > 4096 {
            last_gain.retain(|_, &mut at| now.duration_since(at) < GAIN_COOLDOWN);
        }
    }

    let delta = rand::Rng::random_range(&mut rand::rng(), MIN_GAIN..=MAX_GAIN);
    let (old_xp, new_xp) = data
        .levels
        .add_xp(guild_id.get(), message.author.id.get(), delta);
    let new_level = level_of(new_xp);

    if new_level > level_of(old_xp) {
        let http = ctx.http.clone();
        let levels = data.levels.clone();
        let author = message.author.clone();
        let guild_name = guild_id
            .to_guild_cached(ctx)
            .map(|g| g.name.clone())
            .unwrap_or_default();
        let guild_icon = guild_id.to_guild_cached(ctx).and_then(|g| g.icon_url());
        tokio::spawn(async move {
            notify_level_up(
                &http,
                &levels,
                &guild_name,
                guild_icon,
                &author,
                new_level,
                false,
            )
            .await;
        });
    }

    Ok(())
}

/// A DM de level up, com a pergunta de se a pessoa quer continuar recebendo.
async fn notify_level_up(
    http: &serenity::Http,
    levels: &crate::store::LevelStore,
    guild_name: &str,
    guild_icon: Option<String>,
    user: &serenity::User,
    level: i32,
    by_moderator: bool,
) {
    if !levels.dm_enabled(user.id.get()) {
        return;
    }

    // A pergunta so vai ate a pessoa responder uma vez.
    let ask = !levels.dm_answered(user.id.get());

    let description = format!(
        "You reached **level {level}** in **{}**{}{}",
        embeds::safe_trim(guild_name, 100),
        if by_moderator {
            " (XP given by a moderator)."
        } else {
            "."
        },
        if ask {
            "\n\nWould you like to keep receiving these DMs?"
        } else {
            ""
        }
    );

    let mut embed = serenity::CreateEmbed::new()
        .title("🎉 Level up!")
        .description(description)
        .colour(embeds::colors::GOLD);
    if let Some(icon) = guild_icon {
        embed = embed.thumbnail(icon);
    }

    let mut builder = serenity::CreateMessage::new().embed(embed);
    if ask {
        builder = builder.components(vec![serenity::CreateActionRow::Buttons(vec![
            serenity::CreateButton::new(DM_YES)
                .style(serenity::ButtonStyle::Success)
                .label("Yes"),
            serenity::CreateButton::new(DM_NO)
                .style(serenity::ButtonStyle::Secondary)
                .label("No"),
        ])]);
    }

    match user.direct_message(http, builder).await {
        Ok(_) => {}
        Err(serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(response)))
            if response.status_code == serenity::StatusCode::FORBIDDEN
                || response.status_code == serenity::StatusCode::BAD_REQUEST =>
        {
            // DM fechada ou sem servidor em comum: comum demais para virar
            // linha de log a cada level up.
        }
        Err(cause) => {
            tracing::warn!(
                "[niveis] aviso: nao consegui mandar a DM de level up para {}: {cause}",
                user.id
            );
        }
    }
}

// ------------------------------------------------------------------
// Botoes "lvl:" (Yes/No da DM)
// ------------------------------------------------------------------

/// So trata interacao de componente com o prefixo "lvl:"; o resto nao e
/// daqui.
pub async fn on_component(
    ctx: &serenity::Context,
    data: &Data,
    press: &serenity::ComponentInteraction,
) -> Result<(), Error> {
    let id = press.data.custom_id.as_str();
    if !id.starts_with(PREFIX) {
        return Ok(());
    }

    let keep = match id {
        DM_YES => true,
        DM_NO => false,
        _ => {
            tracing::warn!("[niveis] custom id desconhecido: '{id}'");
            return Ok(());
        }
    };

    data.levels.set_dm(press.user.id.get(), keep);

    press
        .create_response(ctx, serenity::CreateInteractionResponse::Acknowledge)
        .await?;

    let note = if keep {
        "👍 Got it — you'll keep getting a DM when you level up."
    } else {
        "🔕 Got it — no more level-up DMs. Turn them back on anytime with `/level-dms`."
    };

    // Edita a propria DM: tira os botoes e diz o que ficou valendo.
    let original = press.message.embeds.first();
    let mut update = serenity::EditInteractionResponse::new().components(Vec::new());
    update = match original {
        Some(embed) => {
            let description = embeds::trim(
                &embed
                    .description
                    .clone()
                    .unwrap_or_default()
                    .replace("Would you like to keep receiving these DMs?", note),
                4000,
            );
            let mut rebuilt = serenity::CreateEmbed::new().description(description);
            if let Some(title) = &embed.title {
                rebuilt = rebuilt.title(title.clone());
            }
            if let Some(colour) = embed.colour {
                rebuilt = rebuilt.colour(colour);
            }
            if let Some(thumb) = &embed.thumbnail {
                rebuilt = rebuilt.thumbnail(thumb.url.clone());
            }
            update.embed(rebuilt)
        }
        None => update.content(note),
    };

    press.edit_response(ctx, update).await?;
    Ok(())
}

pub fn commands() -> Vec<poise::Command<Data, Error>> {
    vec![
        scope::guild_only(rank()),
        scope::anywhere(level_dms()),
        xp(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curva_igual_ao_mee6() {
        assert_eq!(xp_to_next(0), 100);
        assert_eq!(level_of(0), 0);
        assert_eq!(level_of(99), 0);
        assert_eq!(level_of(100), 1);
        // Os marcos do README: nivel 10 com 4.675 XP, nivel 50 com 268.375.
        assert_eq!(level_of(4_675), 10);
        assert_eq!(level_of(4_674), 9);
        assert_eq!(level_of(268_375), 50);
    }

    #[test]
    fn progresso_dentro_do_nivel() {
        let p = progress(150);
        assert_eq!(p.level, 1);
        assert_eq!(p.into, 50);
        assert_eq!(p.needed, xp_to_next(1));
    }

    #[test]
    fn xp_negativo_fica_em_zero() {
        assert_eq!(level_of(-10), 0);
        let p = progress(-10);
        assert_eq!(p.into, 0);
    }
}
