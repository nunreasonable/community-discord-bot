//! O /config: configuracao do bot por servidor, feita por quem manda no
//! servidor (commands/Config.cs).
//!
//! Antes tudo isto vivia num arquivo na maquina do bot, o que exigia acesso
//! SSH para apontar um canal - e, pior, as chaves eram globais do processo:
//! com o bot em mais de um servidor, o log de moderacao de um podia cair no
//! canal de outro.
//!
//! Manage Server e o portao, mesmo criterio do /ticket-panel. O dono sempre a
//! tem e pode delegar sem entregar a conta.
//!
//! Cada subcomando mexe em UMA chave e tem UMA opcao opcional, onde omitir
//! significa limpar. Um comando so com varias opcoes opcionais seria mais
//! curto e ambiguo: nao daria para distinguir "nao mexa nisso" de "limpe
//! isso".

use poise::serenity_prelude as serenity;

use super::util::{self, channel_mention, respond, role_mention};
use crate::{
    Context, Error, config, embeds,
    embeds::colors,
    hierarchy,
    store::guild_settings::{GuildSettings, nickname_format},
};

/// Configures the bot for this server
#[poise::command(
    slash_command,
    rename = "config",
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD",
    subcommand_required,
    subcommands(
        "view",
        "mod_log",
        "auto_softban",
        "tickets_category",
        "tickets_role",
        "tickets_log",
        "verify_role",
        "verify_unverified_role",
        "verify_nickname",
        "verify_min_age",
        "leveling",
        "economy"
    )
)]
pub async fn config_group(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Shows this server's current configuration
#[poise::command(slash_command, guild_only, required_permissions = "MANAGE_GUILD")]
async fn view(ctx: Context<'_>) -> Result<(), Error> {
    util::defer(ctx).await?;
    let guild_id = util::guild_id(ctx)?;
    let settings = ctx.data().guild_settings.for_guild(guild_id.get());
    let bot = util::bot_member(ctx.serenity_context(), guild_id).await;
    let verify_ready = config::current().roblox_verify.is_configured();

    // Tudo daqui ate o embed pronto e sincrono: o cache nao atravessa .await.
    let embed = {
        let guild = ctx.guild().ok_or("servidor fora do cache")?;
        build_view(&guild, settings.as_ref(), bot.as_ref(), verify_ready)
    };
    respond(ctx, embed).await
}

fn build_view(
    guild: &serenity::Guild,
    settings: Option<&GuildSettings>,
    bot: Option<&serenity::Member>,
    verify_ready: bool,
) -> serenity::CreateEmbed {
    let s = settings.cloned().unwrap_or_default();

    let staff_role = match s.ticket_staff_role_id {
        Some(id) if role_exists(guild, id) => role_mention(id),
        Some(id) => format!("⚠️ role `{id}` no longer exists"),
        None => "— *(`/config tickets-role`)*".to_owned(),
    };

    let mut embed = serenity::CreateEmbed::new()
        .title(format!("Configuration for {}", guild.name))
        .colour(colors::BLURPLE)
        .field(
            "Moderation log",
            describe_channel(guild, s.moderation_log_channel_id, "`/config mod-log`"),
            false,
        )
        .field(
            "Auto-softban",
            describe_channel(guild, s.auto_softban_channel_id, "`/config auto-softban`"),
            false,
        )
        .field(
            "Tickets — category",
            describe_channel(guild, s.ticket_category_id, "`/config tickets-category`"),
            true,
        )
        .field("Tickets — staff role", staff_role, true)
        .field(
            "Tickets — log channel",
            describe_channel(guild, s.ticket_log_channel_id, "`/config tickets-log`"),
            true,
        )
        .field(
            "Verification — role",
            describe_role(guild, s.verified_role_id, "`/config verify-role`"),
            true,
        )
        .field(
            "Verification — unverified",
            describe_role(
                guild,
                s.unverified_role_id,
                "`/config verify-unverified-role`",
            ),
            true,
        )
        .field(
            "Verification — nickname",
            nickname_format::describe(s.nickname_format.as_deref()),
            true,
        )
        .field(
            "Verification — minimum age",
            if s.min_account_age_days > 0 {
                format!("{} day(s)", s.min_account_age_days)
            } else {
                "off".to_owned()
            },
            true,
        )
        .field(
            "Group binds",
            if s.group_binds.is_empty() {
                "— *(`/bind add`)*".to_owned()
            } else {
                format!("{} (`/bind list`)", s.group_binds.len())
            },
            true,
        )
        .field(
            "Levels",
            if s.leveling_enabled {
                "on"
            } else {
                "off *(`/config leveling`)*"
            },
            true,
        )
        .field(
            "Economy",
            if s.economy_enabled {
                "on"
            } else {
                "off *(`/config economy`)*"
            },
            true,
        );

    let bot_permissions = bot.map(|b| guild.member_permissions(b));
    let pending = pending(settings, bot_permissions, verify_ready);
    if !pending.is_empty() {
        let list = pending
            .iter()
            .map(|p| format!("• {p}"))
            .collect::<Vec<_>>()
            .join("\n");
        embed = embed.field("Still missing", list, false);
    }

    if let Some(when) = s.updated_at_utc {
        embed = embed.footer(serenity::CreateEmbedFooter::new(format!(
            "Last changed by {} on {} UTC",
            s.updated_by_id.map(|id| id.to_string()).unwrap_or_default(),
            when.format("%Y-%m-%d %H:%M")
        )));
    }
    embed
}

/// Channel that gets a record of every moderation action
#[poise::command(
    slash_command,
    rename = "mod-log",
    guild_only,
    required_permissions = "MANAGE_GUILD"
)]
async fn mod_log(
    ctx: Context<'_>,
    #[description = "Leave empty to turn logging off"]
    #[channel_types("Text", "News")]
    channel: Option<serenity::Channel>,
) -> Result<(), Error> {
    apply_channel(
        ctx,
        channel,
        serenity::Permissions::SEND_MESSAGES | serenity::Permissions::EMBED_LINKS,
        |s, v| s.moderation_log_channel_id = v,
        |c| {
            embeds::ok(
                "Moderation log on",
                format!(
                    "Moderation actions will now be logged in {}.",
                    channel_mention(c.id)
                ),
            )
        },
        || {
            embeds::ok(
                "Moderation log off",
                "Nothing will be logged beyond Discord's own Audit Log.",
            )
        },
    )
    .await
}

/// Trap channel: anyone who posts in it is softbanned automatically
#[poise::command(
    slash_command,
    rename = "auto-softban",
    guild_only,
    required_permissions = "MANAGE_GUILD"
)]
async fn auto_softban(
    ctx: Context<'_>,
    #[description = "Leave empty to disarm the trap"] channel: Option<serenity::Channel>,
) -> Result<(), Error> {
    apply_channel(
        ctx,
        channel,
        serenity::Permissions::VIEW_CHANNEL,
        |s, v| s.auto_softban_channel_id = v,
        // Este comando arma um recurso que bane sozinho. A resposta diz
        // exatamente o que vai acontecer, em vez de um "ok" de uma linha.
        |c| {
            embeds::ok(
                "Trap armed",
                format!(
                    "**Any message in {} now results in an automatic softban** — a ban followed by an unban, which deletes the person's last 7 days of messages and leaves them free to come back with an invite.\n\n\
                     Exempt: bots and webhooks, system messages, anyone with Ban Members, Manage Server or Administrator, the owner, and anyone above my role. Forum posts count, since they are threads inside the channel.\n\n\
                     After more than 5 softbans in 60 seconds, the watcher disarms itself until the bot restarts.",
                    channel_mention(c.id)
                ),
            )
        },
        || {
            embeds::ok(
                "Trap disarmed",
                "No one will be punished automatically for posting in any channel anymore.",
            )
        },
    )
    .await
}

/// Category where ticket channels are created
#[poise::command(
    slash_command,
    rename = "tickets-category",
    guild_only,
    required_permissions = "MANAGE_GUILD"
)]
async fn tickets_category(
    ctx: Context<'_>,
    #[description = "Leave empty to turn tickets off"]
    #[channel_types("Category")]
    category: Option<serenity::Channel>,
) -> Result<(), Error> {
    apply_channel(
        ctx,
        category,
        serenity::Permissions::empty(),
        |s, v| s.ticket_category_id = v,
        |c| {
            embeds::ok(
                "Ticket category set",
                format!(
                    "Ticket channels will be created in **{}**. Check the rest with `/config view`.",
                    c.name
                ),
            )
        },
        || {
            embeds::ok(
                "Tickets off",
                "Without a category there is nowhere to create the channels, so `/ticket` will refuse.",
            )
        },
    )
    .await
}

/// Staff role that can see every ticket
#[poise::command(
    slash_command,
    rename = "tickets-role",
    guild_only,
    required_permissions = "MANAGE_GUILD"
)]
async fn tickets_role(
    ctx: Context<'_>,
    #[description = "Leave empty to turn tickets off"] role: Option<serenity::Role>,
) -> Result<(), Error> {
    util::defer(ctx).await?;
    let guild_id = util::guild_id(ctx)?;

    let role_id = role.as_ref().map(|r| r.id.get());
    ctx.data()
        .guild_settings
        .set(guild_id.get(), ctx.author().id.get(), |s| {
            s.ticket_staff_role_id = role_id;
        })
        .await?;

    let Some(role) = role else {
        return respond(
            ctx,
            embeds::ok(
                "Tickets off",
                "Without a staff role there is no one to give access to, so `/ticket` will refuse.",
            ),
        )
        .await;
    };

    let warning = if role.mentionable {
        ""
    } else {
        "\n\n⚠️ This role **is not mentionable**, so staff won't be notified when a ticket opens — the mention shows up as plain text."
    };

    respond(
        ctx,
        embeds::ok(
            "Staff role set",
            format!(
                "{} can now see every ticket in this server.{warning}",
                role_mention(role.id)
            ),
        ),
    )
    .await
}

/// Channel that receives the transcript when a ticket closes
#[poise::command(
    slash_command,
    rename = "tickets-log",
    guild_only,
    required_permissions = "MANAGE_GUILD"
)]
async fn tickets_log(
    ctx: Context<'_>,
    #[description = "Leave empty to stop archiving"]
    #[channel_types("Text", "News")]
    channel: Option<serenity::Channel>,
) -> Result<(), Error> {
    apply_channel(
        ctx,
        channel,
        // AttachFiles porque o transcript e um anexo: sem ela o fechamento
        // falha e o canal do ticket fica de pe, e descobrir isso no primeiro
        // fechamento e tarde demais.
        serenity::Permissions::SEND_MESSAGES
            | serenity::Permissions::EMBED_LINKS
            | serenity::Permissions::ATTACH_FILES,
        |s, v| s.ticket_log_channel_id = v,
        |c| {
            embeds::ok(
                "Ticket log set",
                format!(
                    "Each closed ticket's transcript goes to {}, and only then is the channel deleted.",
                    channel_mention(c.id)
                ),
            )
        },
        || {
            embeds::ok(
                "Ticket log off",
                "Without a log channel there is nowhere to archive, so closing a ticket will **lock** the channel instead of deleting it — the conversation is never lost without a copy.",
            )
        },
    )
    .await
}

/// Role given to members who verify their Roblox account
#[poise::command(
    slash_command,
    rename = "verify-role",
    guild_only,
    required_permissions = "MANAGE_GUILD"
)]
async fn verify_role(
    ctx: Context<'_>,
    #[description = "Leave empty to give no role"] role: Option<serenity::Role>,
) -> Result<(), Error> {
    apply_role(
        ctx,
        role,
        |s| s.unverified_role_id,
        |s, v| s.verified_role_id = v,
        |r| {
            embeds::ok(
                "Verified role set",
                format!(
                    "Anyone who links their Roblox account gets {} — right away through `/verify`, or on joining if they already verified in another server. Members who were verified before get it on their next `/update`.",
                    role_mention(r.id)
                ),
            )
        },
        || {
            embeds::ok(
                "Verified role off",
                "Verifying an account no longer gives a role. Members who already have it keep it.",
            )
        },
    )
    .await
}

/// Role for members who haven't linked a Roblox account yet
#[poise::command(
    slash_command,
    rename = "verify-unverified-role",
    guild_only,
    required_permissions = "MANAGE_GUILD"
)]
async fn verify_unverified_role(
    ctx: Context<'_>,
    #[description = "Leave empty to turn it off"] role: Option<serenity::Role>,
) -> Result<(), Error> {
    apply_role(
        ctx,
        role,
        |s| s.verified_role_id,
        |s, v| s.unverified_role_id = v,
        // O cargo so e dado em eventos (entrada, /update, /unverify): o bot
        // nao varre o servidor. A resposta diz isso para ninguem esperar ver
        // os membros antigos com o cargo de uma hora para a outra.
        |r| {
            embeds::ok(
                "Unverified role set",
                format!(
                    "Anyone who **joins** without a linked account gets {}, and loses it on verifying. Members already in the server don't get it automatically — only when they run `/update`.",
                    role_mention(r.id)
                ),
            )
        },
        || {
            embeds::ok(
                "Unverified role off",
                "No one gets a role for being unverified anymore. Members who already have it keep it.",
            )
        },
    )
    .await
}

/// As escolhas do formato de apelido. O valor gravado e o de
/// `nickname_format` (em portugues, como no arquivo do C#).
#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum NicknameChoice {
    #[name = "Don't change the nickname"]
    None,
    #[name = "Roblox username"]
    Username,
    #[name = "Roblox display name"]
    Display,
    #[name = "Display name (@username), like BloxLink"]
    Smart,
}

impl NicknameChoice {
    /// None = "nenhum": nada gravado, o bot nao mexe no apelido.
    fn stored(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Username => Some(nickname_format::USERNAME),
            Self::Display => Some(nickname_format::DISPLAY),
            Self::Smart => Some(nickname_format::SMART),
        }
    }
}

/// How the bot sets the nickname of members who verify their Roblox account
#[poise::command(
    slash_command,
    rename = "verify-nickname",
    guild_only,
    required_permissions = "MANAGE_GUILD"
)]
async fn verify_nickname(
    ctx: Context<'_>,
    #[description = "What goes in the nickname"] format: NicknameChoice,
) -> Result<(), Error> {
    util::defer(ctx).await?;
    let guild_id = util::guild_id(ctx)?;
    let value = format.stored();

    ctx.data()
        .guild_settings
        .set(guild_id.get(), ctx.author().id.get(), |s| {
            s.nickname_format = value.map(str::to_owned);
        })
        .await?;

    let Some(value) = value else {
        return respond(
            ctx,
            embeds::ok(
                "Nicknames left alone",
                "The bot no longer changes the nickname of members who verify.",
            ),
        )
        .await;
    };

    let bot = util::bot_member(ctx.serenity_context(), guild_id).await;
    let missing = bot.is_some_and(|bot| {
        ctx.guild().is_some_and(|g| {
            !g.member_permissions(&bot).intersects(
                serenity::Permissions::MANAGE_NICKNAMES | serenity::Permissions::ADMINISTRATOR,
            )
        })
    });
    let warning = if missing {
        "\n\n⚠️ I'm missing **Manage Nicknames**: without it, no nickname will change."
    } else {
        ""
    };

    respond(
        ctx,
        embeds::ok(
            "Nickname format set",
            format!(
                "Members who verify will now be nicknamed with their **{}**. The server owner and anyone above my role are exempt, due to a Discord limitation.{warning}",
                nickname_format::describe(Some(value))
            ),
        ),
    )
    .await
}

/// Rejects Roblox accounts younger than this, in days
#[poise::command(
    slash_command,
    rename = "verify-min-age",
    guild_only,
    required_permissions = "MANAGE_GUILD"
)]
async fn verify_min_age(
    ctx: Context<'_>,
    #[description = "Minimum Roblox account age, in days. 0 turns it off"] days: i64,
) -> Result<(), Error> {
    util::defer(ctx).await?;

    if !(0..=3650).contains(&days) {
        return respond(
            ctx,
            embeds::error(
                "Invalid value",
                "The minimum age goes from 0 (off) to 3650 days.",
            ),
        )
        .await;
    }

    let guild_id = util::guild_id(ctx)?;
    ctx.data()
        .guild_settings
        .set(guild_id.get(), ctx.author().id.get(), |s| {
            s.min_account_age_days = days as i32;
        })
        .await?;

    respond(
        ctx,
        if days == 0 {
            embeds::ok(
                "Anti-alt filter off",
                "Any linked Roblox account now counts in this server.",
            )
        } else {
            embeds::ok(
                "Anti-alt filter on",
                format!(
                    "Roblox accounts younger than **{days} day(s)** can still be linked, but in this server they are treated as unverified: no verified role and no group roles."
                ),
            )
        },
    )
    .await
}

/// Turns XP and levels on or off in this server
#[poise::command(slash_command, guild_only, required_permissions = "MANAGE_GUILD")]
async fn leveling(
    ctx: Context<'_>,
    #[description = "Members earn XP for chatting and can level up"] enabled: bool,
) -> Result<(), Error> {
    util::defer(ctx).await?;
    let guild_id = util::guild_id(ctx)?;
    ctx.data()
        .guild_settings
        .set(guild_id.get(), ctx.author().id.get(), |s| {
            s.leveling_enabled = enabled;
        })
        .await?;

    // Desligar nao apaga nada: o XP fica guardado para quando religar, so
    // para de crescer e sai do ranking global.
    respond(
        ctx,
        if enabled {
            embeds::ok(
                "Levels on",
                "Members now earn XP for chatting (once a minute at most) and get a DM when they level up — they can turn the DM off. Try `/rank`, `/leaderboard` and `/xp give`.",
            )
        } else {
            embeds::ok(
                "Levels off",
                "Nobody earns XP here anymore and this server leaves the global ranking. The XP already earned is kept, so turning levels back on picks up where it left off.",
            )
        },
    )
    .await
}

/// Turns the SOL$ economy on or off in this server
#[poise::command(slash_command, guild_only, required_permissions = "MANAGE_GUILD")]
async fn economy(
    ctx: Context<'_>,
    #[description = "Members can use /daily, /work, /pay and /balance here"] enabled: bool,
) -> Result<(), Error> {
    util::defer(ctx).await?;
    let guild_id = util::guild_id(ctx)?;
    ctx.data()
        .guild_settings
        .set(guild_id.get(), ctx.author().id.get(), |s| {
            s.economy_enabled = enabled;
        })
        .await?;

    // A carteira e global: ligar ou desligar aqui so decide se os comandos
    // funcionam NESTE servidor, nunca mexe em saldo de ninguem.
    respond(
        ctx,
        if enabled {
            embeds::ok(
                "Economy on",
                "Members can now use `/daily`, `/work`, `/pay` and `/balance` here. Wallets are global: the same SOL$ balance follows each person to every server where the economy is on.",
            )
        } else {
            embeds::ok(
                "Economy off",
                "The economy commands no longer work in this server. Nobody's wallet changes — balances are global and stay as they are.",
            )
        },
    )
    .await
}

/// O caminho comum dos subcomandos de canal: confere que o canal e deste
/// servidor, confere as permissoes que aquele recurso vai precisar, grava e
/// responde.
///
/// A conferencia de servidor parece redundante - o seletor do Discord so
/// oferece canais daqui -, mas e barata e a alternativa e confiar num dado
/// que veio de fora. A de permissao e o ponto: sem ela o operador so
/// descobre o problema quando o recurso falha calado, dias depois.
async fn apply_channel(
    ctx: Context<'_>,
    channel: Option<serenity::Channel>,
    needs: serenity::Permissions,
    assign: fn(&mut GuildSettings, Option<u64>),
    on_set: impl FnOnce(&serenity::GuildChannel) -> serenity::CreateEmbed,
    on_clear: impl FnOnce() -> serenity::CreateEmbed,
) -> Result<(), Error> {
    util::defer(ctx).await?;
    let guild_id = util::guild_id(ctx)?;

    // Canal de DM ou de grupo nao tem servidor: vale como "de outro servidor".
    let channel = match channel {
        None => None,
        Some(channel) => match channel.guild() {
            Some(channel) if channel.guild_id == guild_id => Some(channel),
            _ => {
                return respond(
                    ctx,
                    embeds::error(
                        "Channel from another server",
                        "Only channels from this server can be configured.",
                    ),
                )
                .await;
            }
        },
    };

    if let Some(channel) = &channel
        && !needs.is_empty()
        && let Some(bot) = util::bot_member(ctx.serenity_context(), guild_id).await
    {
        let missing = {
            let guild = ctx.guild().ok_or("servidor fora do cache")?;
            needs - guild.user_permissions_in(channel, &bot)
        };
        if !missing.is_empty() {
            return respond(
                ctx,
                embeds::error(
                    "Missing permissions in that channel",
                    format!(
                        "I can't use {} because I'm missing: **{}**.\n\nAdjust the channel's permissions and run the command again.",
                        channel_mention(channel.id),
                        permission_names(missing)
                    ),
                ),
            )
            .await;
        }
    }

    let value = channel.as_ref().map(|c| c.id.get());
    ctx.data()
        .guild_settings
        .set(guild_id.get(), ctx.author().id.get(), |s| assign(s, value))
        .await?;

    respond(
        ctx,
        match &channel {
            Some(channel) => on_set(channel),
            None => on_clear(),
        },
    )
    .await
}

/// O caminho comum dos subcomandos de cargo da verificacao. O cargo passa
/// pela mesma checagem do /bind: o bot vai distribui-lo sozinho, entao ele
/// nao pode estar acima de quem configura nem acima do bot.
async fn apply_role(
    ctx: Context<'_>,
    role: Option<serenity::Role>,
    conflict: fn(&GuildSettings) -> Option<u64>,
    assign: fn(&mut GuildSettings, Option<u64>),
    on_set: impl FnOnce(&serenity::Role) -> serenity::CreateEmbed,
    on_clear: impl FnOnce() -> serenity::CreateEmbed,
) -> Result<(), Error> {
    util::defer(ctx).await?;
    let guild_id = util::guild_id(ctx)?;

    if let Some(role) = &role {
        let actor = util::actor(ctx).await?;
        let bot = util::bot_member(ctx.serenity_context(), guild_id).await;
        let refusal = {
            let guild = ctx.guild().ok_or("servidor fora do cache")?;
            hierarchy::check_assignable_role(&guild, &actor, role, bot.as_ref())
        };
        if let Some(refusal) = refusal {
            return respond(ctx, refusal).await;
        }

        // Verificado e nao verificado no mesmo cargo: o bot daria e tiraria o
        // mesmo cargo a cada evento.
        if let Some(current) = ctx.data().guild_settings.for_guild(guild_id.get())
            && conflict(&current) == Some(role.id.get())
        {
            return respond(
                ctx,
                embeds::error(
                    "Same role",
                    format!(
                        "{} is already the other verification role. Verified and unverified must be different roles.",
                        role_mention(role.id)
                    ),
                ),
            )
            .await;
        }
    }

    let value = role.as_ref().map(|r| r.id.get());
    ctx.data()
        .guild_settings
        .set(guild_id.get(), ctx.author().id.get(), |s| assign(s, value))
        .await?;

    respond(
        ctx,
        match &role {
            Some(role) => on_set(role),
            None => on_clear(),
        },
    )
    .await
}

fn role_exists(guild: &serenity::Guild, id: u64) -> bool {
    guild.roles.contains_key(&serenity::RoleId::new(id))
}

/// O GetChannel do DisCatSharp acha canal e thread ativa.
fn channel_exists(guild: &serenity::Guild, id: u64) -> bool {
    let id = serenity::ChannelId::new(id);
    guild.channels.contains_key(&id) || guild.threads.iter().any(|t| t.id == id)
}

fn describe_role(guild: &serenity::Guild, role_id: Option<u64>, command: &str) -> String {
    match role_id {
        None => format!("— *({command})*"),
        Some(id) if role_exists(guild, id) => role_mention(id),
        Some(id) => format!("⚠️ role `{id}` no longer exists"),
    }
}

fn describe_channel(guild: &serenity::Guild, channel_id: Option<u64>, command: &str) -> String {
    match channel_id {
        None => format!("— *({command})*"),
        Some(id) if channel_exists(guild, id) => channel_mention(id),
        Some(id) => format!("⚠️ channel `{id}` no longer exists"),
    }
}

/// Os nomes das permissoes como o `Permissions.ToString()` do DisCatSharp os
/// escrevia na mensagem: nome do enum, em ordem crescente de bit, separados
/// por virgula. So as que o /config confere.
fn permission_names(perms: serenity::Permissions) -> String {
    const NAMES: [(serenity::Permissions, &str); 4] = [
        (serenity::Permissions::VIEW_CHANNEL, "AccessChannels"),
        (serenity::Permissions::SEND_MESSAGES, "SendMessages"),
        (serenity::Permissions::EMBED_LINKS, "EmbedLinks"),
        (serenity::Permissions::ATTACH_FILES, "AttachFiles"),
    ];
    let mut names: Vec<String> = NAMES
        .iter()
        .filter(|(p, _)| perms.contains(*p))
        .map(|(_, n)| (*n).to_owned())
        .collect();
    // Bit que nao esta na tabela nao some calado.
    let rest = perms
        - NAMES
            .iter()
            .fold(serenity::Permissions::empty(), |a, (p, _)| a | *p);
    if !rest.is_empty() {
        names.push(format!("{rest:?}"));
    }
    names.join(", ")
}

/// O que ainda falta para cada recurso funcionar. E a pergunta que quem
/// configura de fato tem, e que uma lista de valores nao responde.
/// `bot` e a permissao de SERVIDOR do bot, quando ele foi lido.
fn pending(
    settings: Option<&GuildSettings>,
    bot: Option<serenity::Permissions>,
    verify_ready: bool,
) -> Vec<&'static str> {
    use poise::serenity_prelude::Permissions as P;

    let mut pending = Vec::new();
    let positive = |v: Option<u64>| v.is_some_and(|id| id > 0);

    let verification = settings.is_some_and(GuildSettings::verification_configured);
    if verification && !verify_ready {
        pending.push("Roblox verification is set up here, but **whoever runs the bot** hasn't turned it on yet: `/verify` replies that it's unavailable");
    }

    let has_category = positive(settings.and_then(|s| s.ticket_category_id));
    let has_role = positive(settings.and_then(|s| s.ticket_staff_role_id));
    let has_trap = positive(settings.and_then(|s| s.auto_softban_channel_id));

    if has_category ^ has_role {
        pending.push(
            "Tickets need **both a category and a role** — just one of them doesn't turn anything on",
        );
    }

    if has_category && has_role && !positive(settings.and_then(|s| s.ticket_log_channel_id)) {
        pending.push(
            "Without a ticket log channel, closing a ticket **locks** the channel instead of deleting it",
        );
    }

    if has_trap && !positive(settings.and_then(|s| s.moderation_log_channel_id)) {
        pending.push("With the trap armed and no moderation log, the notice that the circuit breaker disarmed it doesn't reach any channel");
    }

    if let Some(bot) = bot {
        if has_category && !bot.contains(P::MANAGE_CHANNELS | P::MANAGE_ROLES) {
            pending.push("I'm missing **Manage Channels** and/or **Manage Roles**, and without them I can't create a ticket channel");
        }

        if has_trap && !bot.contains(P::BAN_MEMBERS) {
            pending
                .push("I'm missing **Ban Members**, and without it every auto-softban will fail");
        }

        let uses_roles = settings.is_some_and(|s| {
            positive(s.verified_role_id)
                || positive(s.unverified_role_id)
                || !s.group_binds.is_empty()
        });
        if uses_roles && !bot.intersects(P::MANAGE_ROLES | P::ADMINISTRATOR) {
            pending.push("I'm missing **Manage Roles**, and without it verification can't give or remove any role");
        }

        let renames = settings
            .and_then(|s| s.nickname_format.as_deref())
            .is_some_and(|f| f != nickname_format::NONE);
        if renames && !bot.intersects(P::MANAGE_NICKNAMES | P::ADMINISTRATOR) {
            pending.push("I'm missing **Manage Nicknames**, and without it the nickname of members who verify won't change");
        }
    }

    pending
}

#[cfg(test)]
mod tests {
    use super::*;
    use poise::serenity_prelude::Permissions as P;

    #[test]
    fn nomes_das_permissoes_como_o_csharp() {
        assert_eq!(
            permission_names(P::EMBED_LINKS | P::SEND_MESSAGES),
            "SendMessages, EmbedLinks"
        );
        assert_eq!(permission_names(P::VIEW_CHANNEL), "AccessChannels");
        assert_eq!(
            permission_names(P::ATTACH_FILES | P::VIEW_CHANNEL),
            "AccessChannels, AttachFiles"
        );
    }

    #[test]
    fn pendencias() {
        assert!(pending(None, Some(P::empty()), false).is_empty());

        let mut s = GuildSettings::new(1);
        s.ticket_category_id = Some(10);
        let list = pending(Some(&s), Some(P::all()), true);
        assert_eq!(list.len(), 1);
        assert!(list[0].starts_with("Tickets need"));

        s.ticket_staff_role_id = Some(11);
        let list = pending(Some(&s), Some(P::all()), true);
        assert_eq!(list.len(), 1);
        assert!(list[0].starts_with("Without a ticket log channel"));

        s.ticket_log_channel_id = Some(12);
        s.auto_softban_channel_id = Some(13);
        s.verified_role_id = Some(14);
        s.nickname_format = Some(nickname_format::SMART.into());
        let list = pending(Some(&s), Some(P::empty()), false);
        assert!(list[0].starts_with("Roblox verification is set up"));
        assert!(list[1].starts_with("With the trap armed"));
        assert!(list.iter().any(|p| p.contains("Manage Channels")));
        assert!(list.iter().any(|p| p.contains("Ban Members")));
        assert!(
            list.iter()
                .any(|p| p.contains("**Manage Roles**, and without it verification"))
        );
        assert!(list.iter().any(|p| p.contains("Manage Nicknames")));

        // Administrador cobre papel e apelido, mas nao Ban Members nem canais.
        let list = pending(Some(&s), Some(P::ADMINISTRATOR), true);
        assert_eq!(list.len(), 3);

        // Sem o membro do bot nao ha o que dizer sobre permissao.
        assert_eq!(pending(Some(&s), None, true).len(), 1);
    }

    #[test]
    fn apelido_nenhum_nao_grava() {
        assert_eq!(NicknameChoice::None.stored(), None);
        assert_eq!(NicknameChoice::Smart.stored(), Some(nickname_format::SMART));
    }

    #[test]
    fn grupo_com_os_doze_subcomandos() {
        let group = config_group();
        assert_eq!(group.name, "config");
        assert_eq!(group.default_member_permissions, P::MANAGE_GUILD);
        let names: Vec<_> = group.subcommands.iter().map(|c| &*c.name).collect();
        assert_eq!(
            names,
            [
                "view",
                "mod-log",
                "auto-softban",
                "tickets-category",
                "tickets-role",
                "tickets-log",
                "verify-role",
                "verify-unverified-role",
                "verify-nickname",
                "verify-min-age",
                "leveling",
                "economy"
            ]
        );
        assert!(
            group
                .subcommands
                .iter()
                .all(|c| c.required_permissions == P::MANAGE_GUILD)
        );
    }
}
