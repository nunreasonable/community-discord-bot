//! /ping, /userinfo, /serverinfo, /poll e /help (commands/Utility.cs).
//!
//! /ping, /userinfo e /help funcionam em qualquer lugar (inclusive como app
//! pessoal) e nao dependem de servidor. Os que PRECISAM de servidor -
//! /serverinfo e /poll - carregam `guild_only` no proprio comando.

use poise::serenity_prelude as serenity;
use serenity::{CreateEmbed, CreateEmbedFooter, Timestamp};

use super::commands::edit;
use crate::{Context, Error, embeds, scope};

/// Show the bot's latency
#[poise::command(slash_command, rename = "ping")]
pub async fn ping(ctx: Context<'_>) -> Result<(), Error> {
    let latency = ctx.ping().await.as_millis();
    ctx.send(
        poise::CreateReply::default().embed(
            CreateEmbed::new()
                .title("🏓 Pong")
                .field("Gateway", format!("{latency} ms"), true)
                .colour(embeds::colors::BLURPLE),
        ),
    )
    .await?;
    Ok(())
}

/// Show information about a user
#[poise::command(slash_command, rename = "userinfo")]
pub async fn userinfo(
    ctx: Context<'_>,
    #[description = "Whose information to show"] user: Option<serenity::User>,
) -> Result<(), Error> {
    ctx.defer().await?;

    let target = user.unwrap_or_else(|| ctx.author().clone());

    let mut embed = CreateEmbed::new()
        .title(target.tag())
        .thumbnail(avatar_256(&target.face()))
        .colour(embeds::colors::BLURPLE)
        .field("ID", format!("`{}`", target.id), true)
        .field(
            "Account created",
            format!("<t:{}:D>", target.id.created_at().unix_timestamp()),
            true,
        )
        .field("Bot", if target.bot { "Yes" } else { "No" }, true);

    // Os dados de membro (entrada, cargos, timeout) so existem num servidor
    // em que o BOT esta. Em DM, ou como app pessoal num servidor sem o
    // Sollarety, sai so a parte da conta.
    let guild = scope::bot_guild(ctx);

    // SO o "usuario nao esta no servidor" vira None. Engolir tudo fazia rate
    // limit, 5xx e cancelamento virarem a mesma afirmacao falsa no rodape
    // ("is not in this server"), sem deixar nada no /logs.
    let member = match guild {
        Some(guild_id) => {
            crate::hierarchy::try_get_member(ctx.serenity_context(), guild_id, target.id).await?
        }
        None => None,
    };

    if let (Some(member), Some(guild_id)) = (&member, guild) {
        embed = embed.field(
            "Joined server",
            member.joined_at.map_or_else(
                || "—".to_owned(),
                |at| format!("<t:{}:D>", at.unix_timestamp()),
            ),
            true,
        );

        // Ordem decrescente e sem o @everyone, que todo mundo tem e nao
        // informa nada. Cargo fora do cache fica de fora.
        let roles = ctx
            .serenity_context()
            .cache
            .guild(guild_id)
            .map(|g| role_mentions(&g, member))
            .unwrap_or_default();
        let value = if roles.is_empty() {
            "*none*".to_owned()
        } else {
            embeds::trim(&roles.join(" "), 1000)
        };
        embed = embed.field(format!("Roles ({})", roles.len()), value, false);

        if let Some(until) = member.communication_disabled_until
            && until.unix_timestamp() > Timestamp::now().unix_timestamp()
        {
            embed = embed.field(
                "Timed out until",
                format!("<t:{}:f>", until.unix_timestamp()),
                true,
            );
        }
    } else if guild.is_some() {
        embed = embed.footer(CreateEmbedFooter::new("This user is not in this server."));
    } else if ctx.guild_id().is_some() {
        embed = embed.footer(CreateEmbedFooter::new(
            "Sollarety isn't in this server, so server details aren't shown.",
        ));
    }

    edit(ctx, embed).await
}

/// O avatar no tamanho que o C# pedia (256); o serenity monta com 1024.
fn avatar_256(url: &str) -> String {
    url.replace("?size=1024", "?size=256")
}

fn role_mentions(guild: &serenity::Guild, member: &serenity::Member) -> Vec<String> {
    let everyone = serenity::RoleId::new(guild.id.get());
    let mut roles: Vec<&serenity::Role> = member
        .roles
        .iter()
        .filter(|&&id| id != everyone)
        .filter_map(|id| guild.roles.get(id))
        .collect();
    roles.sort_by_key(|r| std::cmp::Reverse(r.position));
    roles.into_iter().map(|r| format!("<@&{}>", r.id)).collect()
}

/// Show information about the server
#[poise::command(slash_command, rename = "serverinfo", guild_only)]
pub async fn serverinfo(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;
    let guild_id = ctx
        .guild_id()
        .ok_or("comando de servidor fora de servidor")?;

    // Copiado do cache antes do proximo `.await`.
    let embed = {
        let guild = ctx
            .serenity_context()
            .cache
            .guild(guild_id)
            .ok_or("servidor fora do cache")?;

        let mut embed = CreateEmbed::new()
            .title(guild.name.clone())
            .colour(embeds::colors::BLURPLE)
            .field("ID", format!("`{}`", guild.id), true)
            .field(
                "Created",
                format!("<t:{}:D>", guild.id.created_at().unix_timestamp()),
                true,
            )
            .field("Owner", format!("<@{}>", guild.owner_id), true)
            .field("Members", guild.member_count.to_string(), true)
            .field("Roles", guild.roles.len().to_string(), true)
            .field("Channels", guild.channels.len().to_string(), true)
            .field("Boost level", premium_tier_name(guild.premium_tier), true);
        if let Some(icon) = guild.icon_url().filter(|u| !u.trim().is_empty()) {
            embed = embed.thumbnail(icon);
        }
        embed
    };

    edit(ctx, embed).await
}

/// O nome que o `PremiumTier.ToString()` do DisCatSharp dava.
fn premium_tier_name(tier: serenity::PremiumTier) -> String {
    match tier {
        serenity::PremiumTier::Tier0 => "None".into(),
        serenity::PremiumTier::Tier1 => "Tier_1".into(),
        serenity::PremiumTier::Tier2 => "Tier_2".into(),
        serenity::PremiumTier::Tier3 => "Tier_3".into(),
        _ => "Unknown".into(),
    }
}

/// Create a yes/no poll with reactions
#[poise::command(
    slash_command,
    rename = "poll",
    guild_only,
    required_bot_permissions = "ADD_REACTIONS | READ_MESSAGE_HISTORY",
    check = "crate::cooldown::user::<2, 30>"
)]
pub async fn poll(
    ctx: Context<'_>,
    #[description = "What you're asking"] question: String,
) -> Result<(), Error> {
    // Os votos vivem nas reacoes, nao no processo: uma enquete que morre ao
    // reiniciar o bot nao serve para nada.
    let embed = CreateEmbed::new()
        .title("📊 Poll")
        .description(embeds::safe_trim(&question, 1500))
        .colour(embeds::colors::BLURPLE)
        .footer(CreateEmbedFooter::new(format!(
            "Created by {}",
            ctx.author().tag()
        )));

    let handle = ctx.send(poise::CreateReply::default().embed(embed)).await?;

    // Falha ao reagir NAO pode escapar daqui. A enquete ja foi publicada: se
    // o erro subisse, o on_error trocaria o embed da enquete pelo de erro - a
    // enquete simplesmente sumia por causa de uma reacao que nao foi.
    let reacted = async {
        let message = handle.message().await?;
        for emoji in ["👍", "👎"] {
            message
                .react(ctx, serenity::ReactionType::Unicode(emoji.into()))
                .await?;
        }
        Ok::<_, serenity::Error>(())
    };
    if let Err(cause) = reacted.await {
        tracing::warn!("[poll] nao consegui reagir na enquete: {cause}");
    }
    Ok(())
}

/// List the available commands
#[poise::command(slash_command, rename = "help")]
pub async fn help(ctx: Context<'_>) -> Result<(), Error> {
    ctx.send(
        poise::CreateReply::default()
            .embed(help_embed())
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

fn help_embed() -> CreateEmbed {
    CreateEmbed::new()
        .title("Commands")
        .description(
            "The commands under **Works anywhere** also work in DMs, group chats and any server \
             once you add Sollarety to **your apps**. Everything else needs Sollarety in the \
             server. Moderation commands only show up for members who have the matching permission.",
        )
        .colour(embeds::colors::BLURPLE)
        // A lista espelha os comandos registrados com `scope::anywhere`.
        // Mudou la, muda aqui - senao o /help promete o que o Discord nao
        // mostra.
        .field(
            "Works anywhere",
            "`/8ball` `/roll` `/coinflip` `/choose` `/avatar` `/ship` `/rate` `/cancel` `/roleplay` \
             `/text` `/morse` `/rps` `/tictactoe` `/profile` `/level-dms` `/userinfo` `/unverify` \
             `/ping` `/help`",
            false,
        )
        .field(
            "Moderation",
            "`/ban` `/softban` `/kick` `/timeout` `/untimeout` `/purge` `/slowmode` `/lock` `/unlock`",
            false,
        )
        .field("Warnings", "`/warn` `/warnings` `/delwarn`", false)
        .field(
            "Tickets",
            "`/ticket` `/ticket-close` `/ticket-add` `/ticket-remove` `/ticket-panel`",
            false,
        )
        .field(
            "Roblox verification",
            "`/verify` `/unverify` `/update` `/whois` `/verify-panel` \
             `/bind add` `/bind remove` `/bind list`",
            false,
        )
        .field(
            "Music",
            "`/play` `/skip` `/stop` `/pause` `/resume` `/queue` `/nowplaying` \
             `/volume` `/loop` `/shuffle` `/remove`",
            false,
        )
        .field(
            "Levels",
            "`/rank` `/leaderboard` `/profile` `/level-dms` `/xp give` `/xp take`",
            false,
        )
        .field(
            "Economy (SOL$)",
            "`/daily` `/work` `/pay` `/balance` `/leaderboard board:SOL$`",
            false,
        )
        .field(
            "Fun",
            "`/8ball` `/roll` `/coinflip` `/choose` `/avatar` `/ship` `/rate` `/cancel` `/say`",
            false,
        )
        .field(
            "Roleplay",
            "`/roleplay hug` `kiss` `slap` `pat` `highfive` `dance` `attack` `cuddle` `poke` `bonk`",
            false,
        )
        .field(
            "Text & games",
            "`/text vaporwave` `clap` `mock` `quality` `upsidedown` `reverse` \
             `/morse encode` `/morse decode` `/rps` `/tictactoe`",
            false,
        )
        .field(
            "Configuration",
            "`/config view` `/config mod-log` `/config auto-softban` \
             `/config tickets-category` `/config tickets-role` `/config tickets-log` \
             `/config verify-role` `/config verify-unverified-role` \
             `/config verify-nickname` `/config verify-min-age` \
             `/config leveling` `/config economy`",
            false,
        )
        // Sem /logs: ele e de quem administra o BOT, nao de quem usa o
        // servidor, e anunciar um comando que so uma pessoa no mundo pode
        // rodar so rende tentativa recusada.
        .field(
            "Utility",
            "`/ping` `/userinfo` `/serverinfo` `/poll` `/help`",
            false,
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avatar_pede_256() {
        assert_eq!(
            avatar_256("https://cdn.discordapp.com/avatars/1/abc.webp?size=1024"),
            "https://cdn.discordapp.com/avatars/1/abc.webp?size=256"
        );
        assert_eq!(
            avatar_256("https://cdn.discordapp.com/embed/avatars/0.png"),
            "https://cdn.discordapp.com/embed/avatars/0.png"
        );
    }

    #[test]
    fn nivel_de_boost_com_o_nome_do_csharp() {
        assert_eq!(premium_tier_name(serenity::PremiumTier::Tier0), "None");
        assert_eq!(premium_tier_name(serenity::PremiumTier::Tier2), "Tier_2");
    }

    #[test]
    fn help_tem_os_campos_do_csharp() {
        let json = serde_json::to_value(help_embed()).unwrap();
        let fields = json["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 13);
        assert_eq!(fields[0]["name"], "Works anywhere");
        assert_eq!(
            fields[12]["value"],
            "`/ping` `/userinfo` `/serverinfo` `/poll` `/help`"
        );
    }
}
