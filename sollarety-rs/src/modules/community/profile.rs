//! /profile e /leaderboard: so mostram o que os niveis e a economia ja
//! guardam. Cada parte so aparece onde o servidor ligou o sistema dela
//! (Profile.cs).

use poise::{CreateReply, serenity_prelude as serenity};

use super::{common, economy, levels};
use crate::{Context, Data, Error, embeds, scope};

const PER_PAGE: usize = 10;

#[derive(poise::ChoiceParameter, PartialEq)]
pub enum Board {
    #[name = "Levels"]
    Levels,
    #[name = "SOL$"]
    Sol,
}

#[derive(poise::ChoiceParameter, PartialEq)]
pub enum Scope {
    #[name = "This server"]
    Server,
    #[name = "Global"]
    Global,
}

fn leveling_on(data: &Data, guild_id: u64) -> bool {
    data.guild_settings
        .for_guild(guild_id)
        .map(|s| s.leveling_enabled)
        .unwrap_or(false)
}

fn economy_on(data: &Data, guild_id: u64) -> bool {
    data.guild_settings
        .for_guild(guild_id)
        .map(|s| s.economy_enabled)
        .unwrap_or(false)
}

/// Show someone's Sollarety profile: levels, SOL$ and more
#[poise::command(
    slash_command,
    rename = "profile",
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn profile(
    ctx: Context<'_>,
    #[description = "Whose profile (default: you)"] user: Option<serenity::User>,
) -> Result<(), Error> {
    let target = user.unwrap_or_else(|| ctx.author().clone());
    if target.bot {
        ctx.send(
            CreateReply::default()
                .embed(embeds::error(
                    "Bots don't have profiles",
                    "Only people earn XP and SOL$.",
                ))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    ctx.defer().await?;

    // So o servidor em que o BOT esta conta: como app pessoal, num servidor
    // sem o Sollarety, o perfil e o global, como em DM.
    let guild_id = scope::bot_guild(ctx);
    let guild_name = ctx.guild().map(|g| g.name.clone());
    let data = ctx.data();
    let now = chrono::Utc::now();

    let name = common::display_name(ctx, guild_id, &target).await;
    let mut embed = serenity::CreateEmbed::new()
        .title(embeds::trim(&format!("{name}'s profile"), 256))
        .colour(embeds::colors::GOLD)
        .footer(serenity::CreateEmbedFooter::new("Sollarety profile"));
    if let Some(icon) = target.avatar_url() {
        embed = embed.thumbnail(icon);
    }

    // Nivel neste servidor.
    if let Some(guild_id) = guild_id
        && leveling_on(data, guild_id.get())
    {
        let board = data.levels.guild_board(guild_id.get());
        let xp = data.levels.xp_of(guild_id.get(), target.id.get());
        let position = board.iter().position(|&(u, _)| u == target.id.get());
        let detail = levels::progress_line(xp)
            + &match position {
                Some(p) => format!(
                    "\nRank **#{}** of {}",
                    p + 1,
                    embeds::number(board.len() as i64)
                ),
                None => "\nNot ranked yet.".to_owned(),
            };
        embed = embed.field(
            format!(
                "Level in {}",
                embeds::trim(guild_name.as_deref().unwrap_or_default(), 200)
            ),
            detail,
            false,
        );
    }

    // Nivel global: a soma dos servidores com nivel ligado.
    let global = data.levels.global_board(|g| leveling_on(data, g));
    let global_index = global.iter().position(|&(u, _)| u == target.id.get());
    embed = embed.field(
        "Global level",
        match global_index {
            Some(i) => format!(
                "**Level {}** · {} XP\nRank **#{}** of {}",
                levels::level_of(global[i].1),
                embeds::number(global[i].1),
                i + 1,
                embeds::number(global.len() as i64)
            ),
            None => "No XP yet.".to_owned(),
        },
        true,
    );

    embed = embed.field(
        "Leveling in",
        format!(
            "{} server(s)",
            data.levels
                .active_guild_count(target.id.get(), |g| leveling_on(data, g))
        ),
        true,
    );

    // Carteira: em DM (ou como app pessoal), ou onde a economia esta ligada.
    if guild_id.is_none_or(|g| economy_on(data, g.get())) {
        let wallet = data.economy.for_user(target.id.get());
        let board = data.economy.board();
        let position = board.iter().position(|&(u, _)| u == target.id.get());
        let next = economy::next_daily(wallet.as_ref(), now);

        embed = embed.field(
            "Wallet",
            format!(
                "**{}**{}\n🔥 Daily streak: {} day(s)\nNext daily: {}",
                crate::store::economy::format(wallet.as_ref().map(|w| w.balance).unwrap_or(0)),
                match position {
                    Some(p) => format!(" · Rank **#{}**", p + 1),
                    None => String::new(),
                },
                economy::live_streak(wallet.as_ref(), now),
                match next {
                    Some(at) => format!("<t:{}:R>", at.timestamp()),
                    None => "ready now — `/daily`".to_owned(),
                }
            ),
            false,
        );
    }

    embed = embed.field(
        "Account created",
        format!("<t:{}:D>", target.id.created_at().unix_timestamp()),
        true,
    );

    if let Some(guild_id) = guild_id
        && let Ok(Some(member)) = crate::hierarchy::try_get_member(ctx, guild_id, target.id).await
        && let Some(joined) = member.joined_at
    {
        embed = embed.field(
            "Joined this server",
            format!("<t:{}:D>", joined.unix_timestamp()),
            true,
        );
    }

    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Top members by level or SOL$, in this server or worldwide
#[poise::command(
    slash_command,
    rename = "leaderboard",
    guild_only,
    check = "crate::cooldown::user::<3, 15>"
)]
pub async fn leaderboard(
    ctx: Context<'_>,
    #[description = "What to rank by (default: levels)"] board: Option<Board>,
    #[description = "This server or every server (default: this server)"] scope: Option<Scope>,
    #[description = "Which page"]
    #[min = 1]
    page: Option<i64>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild_only garante o servidor");
    let data = ctx.data();
    let is_levels = board.unwrap_or(Board::Levels) == Board::Levels;
    let is_global = scope.unwrap_or(Scope::Server) == Scope::Global;

    // O servidor decide: com o sistema desligado aqui, nem o ranking global
    // dele aparece aqui.
    let enabled = if is_levels {
        leveling_on(data, guild_id.get())
    } else {
        economy_on(data, guild_id.get())
    };
    if !enabled {
        let embed = if is_levels {
            embeds::error(
                "Levels are off in this server",
                "Someone with **Manage Server** can turn them on with `/config leveling`.",
            )
        } else {
            embeds::error(
                "The economy is off in this server",
                "Someone with **Manage Server** can turn it on with `/config economy`.",
            )
        };
        ctx.send(CreateReply::default().embed(embed).ephemeral(true))
            .await?;
        return Ok(());
    }

    // Os nomes do ranking global vem por REST para quem nao esta no cache.
    ctx.defer().await?;

    let rows: Vec<(u64, i64)> = match (is_levels, is_global) {
        (true, false) => data.levels.guild_board(guild_id.get()),
        (true, true) => data.levels.global_board(|g| leveling_on(data, g)),
        (false, true) => data.economy.board(),
        // SOL$ no servidor: a carteira e global, entao "do servidor" e o
        // ranking global filtrado pelos membros que o bot tem em cache.
        (false, false) => {
            let members: std::collections::HashSet<u64> = ctx
                .guild()
                .map(|g| g.members.keys().map(|id| id.get()).collect())
                .unwrap_or_default();
            data.economy
                .board()
                .into_iter()
                .filter(|(u, _)| members.contains(u))
                .collect()
        }
    };

    let pages = (rows.len().div_ceil(PER_PAGE)).max(1);
    let page = page.unwrap_or(1).clamp(1, pages as i64) as usize;

    let mut text = String::new();
    if rows.is_empty() {
        text.push_str(if is_levels {
            "*Nobody has XP yet — start chatting!*\n"
        } else {
            "*Nobody has SOL$ yet — try `/daily`.*\n"
        });
    }

    let mut rank = (page - 1) * PER_PAGE;
    for &(user_id, value) in rows.iter().skip((page - 1) * PER_PAGE).take(PER_PAGE) {
        rank += 1;
        let medal = match rank {
            1 => "🥇".to_owned(),
            2 => "🥈".to_owned(),
            3 => "🥉".to_owned(),
            _ => format!("`#{rank}`"),
        };
        let name = embeds::plain(&name_of(ctx, user_id, is_global).await, 40);
        let detail = if is_levels {
            format!(
                "Level {} · {} XP",
                levels::level_of(value),
                embeds::number(value)
            )
        } else {
            crate::store::economy::format(value)
        };
        text.push_str(&format!("{medal} **{name}** — {detail}\n"));
    }

    let mine = rows.iter().position(|&(u, _)| u == ctx.author().id.get());
    text.push('\n');
    text.push_str(&match mine {
        Some(i) => format!("Your position: **#{}**", i + 1),
        None if is_levels => "You're not ranked yet.".to_owned(),
        None => "You don't have any SOL$ yet.".to_owned(),
    });

    let guild_name = ctx.guild().map(|g| g.name.clone()).unwrap_or_default();
    let title = match (is_levels, is_global) {
        (true, false) => format!("🏆 Levels — {guild_name}"),
        (true, true) => "🌍 Levels — global".to_owned(),
        (false, false) => format!("💰 SOL$ — {guild_name}"),
        (false, true) => "🌍 SOL$ — global".to_owned(),
    };

    let embed = serenity::CreateEmbed::new()
        .title(embeds::trim(&title, 256))
        .description(embeds::trim(&text, 4000))
        .footer(serenity::CreateEmbedFooter::new(format!(
            "Page {page}/{pages} · {} ranked",
            embeds::number(rows.len() as i64)
        )))
        .colour(if is_levels {
            embeds::colors::GOLD
        } else {
            serenity::Colour::new(0xF2B705)
        });

    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Apelido no servidor quando a pessoa esta nele; senao o nome da conta. No
/// ranking global usa sempre o nome da conta - o apelido e coisa de um
/// servidor, e a pessoa pode nem estar neste.
async fn name_of(ctx: Context<'_>, user_id: u64, global: bool) -> String {
    let user_id = serenity::UserId::new(user_id);
    if !global && let Some(member) = ctx.guild().and_then(|g| g.members.get(&user_id).cloned()) {
        return member.display_name().to_owned();
    }

    match user_id.to_user(ctx).await {
        Ok(user) => common::global_name(&user),
        Err(_) => "Unknown user".to_owned(),
    }
}

pub fn commands() -> Vec<poise::Command<Data, Error>> {
    vec![scope::anywhere(profile()), scope::guild_only(leaderboard())]
}
