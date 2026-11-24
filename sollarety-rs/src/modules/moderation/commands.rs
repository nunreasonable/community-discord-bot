//! Comandos de moderacao (commands/Moderation.cs).
//!
//! O portao e a permissao do PROPRIO Discord: quem nao pode banir nem ve o
//! /ban na lista (default_member_permissions), e o `required_permissions`
//! confere de novo na execucao. Isso aproveita os cargos que o servidor ja
//! tem, em vez de exigir um cargo configurado a mao.
//!
//! `required_bot_permissions` existe pelo outro lado: sem ele, faltar
//! permissao ao bot vira um erro cru da API no meio da execucao.

use poise::serenity_prelude as serenity;
use serde_json::json;

use super::{
    softban,
    util::{self, channel_mention, respond, user_mention},
};
use crate::{Context, Error, duration, embeds, hierarchy, modlog};

/// Bans a user from the server
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "BAN_MEMBERS",
    required_permissions = "BAN_MEMBERS",
    required_bot_permissions = "BAN_MEMBERS"
)]
pub async fn ban(
    ctx: Context<'_>,
    #[description = "Who to ban"] user: serenity::User,
    #[description = "Reason for the ban"] reason: Option<String>,
    #[description = "Delete their messages from the last N days (0 to 7)"] delete_days: Option<i64>,
) -> Result<(), Error> {
    util::defer(ctx).await?;
    let delete_days = delete_days.unwrap_or(0);

    if !(0..=7).contains(&delete_days) {
        return respond(
            ctx,
            embeds::error("Invalid value", "`delete_days` must be between 0 and 7."),
        )
        .await;
    }

    let guild_id = util::guild_id(ctx)?;

    // O alvo pode nao ser membro do servidor - banir alguem que ja saiu e
    // legitimo, e por isso a hierarquia so e checada quando ele esta la.
    if let Some(member) = hierarchy::try_get_member(ctx, guild_id, user.id).await?
        && let Some(blocked) = util::blocked(ctx, &member).await?
    {
        return respond(ctx, blocked).await;
    }

    // O serenity recebe DIAS e manda delete_message_seconds para a API. No C#
    // passar `deleteDays` cru apagava 7 SEGUNDOS de mensagens quando o
    // moderador pedia 7 dias; aqui a conversao fica do lado da biblioteca.
    let audit = util::reason(ctx, reason.as_deref());
    if let Err(cause) = guild_id
        .ban_with_reason(ctx.http(), user.id, delete_days as u8, &audit)
        .await
    {
        return respond(
            ctx,
            embeds::error("Failed to ban", embeds::trim(&cause.to_string(), 500)),
        )
        .await;
    }

    respond(
        ctx,
        embeds::ok("Banned", format!("{} was banned.", user_mention(user.id))),
    )
    .await?;

    let extra =
        (delete_days > 0).then(|| format!("Deleted messages from the last {delete_days} day(s)."));
    let mut entry = modlog::Entry::new("Ban", ctx.author()).target(&user);
    if let Some(reason) = reason.as_deref() {
        entry = entry.reason(reason);
    }
    if let Some(extra) = extra.as_deref() {
        entry = entry.extra(extra);
    }
    modlog::record(ctx, &ctx.data().guild_settings, guild_id, entry).await;
    Ok(())
}

/// Kicks a user from the server
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "KICK_MEMBERS",
    required_permissions = "KICK_MEMBERS",
    required_bot_permissions = "KICK_MEMBERS"
)]
pub async fn kick(
    ctx: Context<'_>,
    #[description = "Who to kick"] user: serenity::User,
    #[description = "Reason for the kick"] reason: Option<String>,
) -> Result<(), Error> {
    util::defer(ctx).await?;
    let guild_id = util::guild_id(ctx)?;

    let Some(member) = present_and_allowed(ctx, guild_id, &user).await? else {
        return Ok(());
    };

    let audit = util::reason(ctx, reason.as_deref());
    if let Err(cause) = member.kick_with_reason(ctx, &audit).await {
        return respond(
            ctx,
            embeds::error("Failed to kick", embeds::trim(&cause.to_string(), 500)),
        )
        .await;
    }

    respond(
        ctx,
        embeds::ok("Kicked", format!("{} was kicked.", user_mention(user.id))),
    )
    .await?;

    record(ctx, guild_id, "Kick", Some(&user), reason.as_deref(), None).await;
    Ok(())
}

// Cooldown de 3 segundos por usuario - e o numero da documentacao que este
// comando segue. E o unico comando de moderacao com cooldown: os vizinhos nao
// tem, e sem esta nota o (1, 3) pareceria arbitrario.
/// Bans and immediately unbans a user, deleting their messages
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "BAN_MEMBERS",
    required_permissions = "BAN_MEMBERS",
    required_bot_permissions = "BAN_MEMBERS",
    check = "crate::cooldown::user::<1, 3>"
)]
pub async fn softban(
    ctx: Context<'_>,
    #[description = "Who to softban"] user: serenity::User,
    #[description = "Reason for the softban"] reason: Option<String>,
) -> Result<(), Error> {
    util::defer(ctx).await?;
    let guild_id = util::guild_id(ctx)?;

    // Exige o membro presente, como o /kick e ao contrario do /ban. Softban e
    // uma expulsao com limpeza; em quem ja saiu nao ha nada para expulsar, e
    // o unban logo em seguida devolve o servidor ao estado exato de antes.
    // Quem quiser so apagar o historico de alguem que ja foi embora tem o
    // /ban com delete_days, que e um banimento assumido.
    if present_and_allowed(ctx, guild_id, &user).await?.is_none() {
        return Ok(());
    }

    let audit = util::reason(ctx, reason.as_deref());
    match softban::apply(ctx.http(), guild_id, user.id, &audit).await {
        softban::Outcome::BanFailed(error) => {
            respond(
                ctx,
                embeds::error("Failed to apply the softban", embeds::trim(&error, 500)),
            )
            .await
        }
        softban::Outcome::UnbanFailed(error) => {
            // O banimento ficou de pe. Dizer so "falhou" aqui seria mentir
            // sobre o estado do servidor: alguem precisa ir desfazer a mao.
            // Sem crase em volta dos detalhes: a mensagem da API vem de fora,
            // e uma crase dentro dela quebraria o bloco de codigo.
            respond(
                ctx,
                embeds::error(
                    "Softban only half done",
                    format!(
                        "{} was banned and their messages were deleted, but the unban **failed** — they are still banned. Remove the ban manually.\n\nDetails: {}",
                        user_mention(user.id),
                        embeds::trim(&error, 400)
                    ),
                ),
            )
            .await?;

            // Registrado mesmo assim, e de proposito: um estado errado e
            // justamente o que precisa ficar anotado em algum lugar.
            record(
                ctx,
                guild_id,
                "Softban",
                Some(&user),
                reason.as_deref(),
                Some(UNBAN_FAILED_NOTE),
            )
            .await;
            Ok(())
        }
        softban::Outcome::Ok => {
            respond(
                ctx,
                embeds::ok(
                    "Softban applied",
                    format!(
                        "{} was banned and immediately unbanned. Their messages from the last 7 days were deleted, and they can rejoin with an invite.",
                        user_mention(user.id)
                    ),
                ),
            )
            .await?;
            record(
                ctx,
                guild_id,
                "Softban",
                Some(&user),
                reason.as_deref(),
                Some(SOFTBAN_OK_NOTE),
            )
            .await;
            Ok(())
        }
    }
}

/// Detalhe do registro quando o unban do softban falhou. Compartilhado com o
/// vigia de canal.
pub const UNBAN_FAILED_NOTE: &str =
    "⚠️ The unban failed — the user is still BANNED and must be unbanned manually.";

/// Detalhe do registro de um softban completo. Compartilhado com o vigia.
pub const SOFTBAN_OK_NOTE: &str =
    "Deleted messages from the last 7 days. The user can rejoin with an invite.";

/// Times out a user for a while
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MODERATE_MEMBERS",
    required_permissions = "MODERATE_MEMBERS",
    required_bot_permissions = "MODERATE_MEMBERS"
)]
pub async fn timeout(
    ctx: Context<'_>,
    #[description = "Who to time out"] user: serenity::User,
    #[description = "E.g. 10m, 2h30m, 1d (max 28 days)"] duration: String,
    #[description = "Reason for the timeout"] reason: Option<String>,
) -> Result<(), Error> {
    util::defer(ctx).await?;

    let span = match duration::parse(&duration) {
        Ok(span) => span,
        Err(error) => return respond(ctx, embeds::error("Invalid duration", error)).await,
    };

    let guild_id = util::guild_id(ctx)?;
    if present_and_allowed(ctx, guild_id, &user).await?.is_none() {
        return Ok(());
    }

    let until = serenity::Timestamp::from_unix_timestamp(
        chrono::Utc::now().timestamp() + span.as_secs() as i64,
    )?;
    let audit = util::reason(ctx, reason.as_deref());
    let edit = serenity::EditMember::new()
        .disable_communication_until_datetime(until)
        .audit_log_reason(&audit);
    if let Err(cause) = guild_id.edit_member(ctx, user.id, edit).await {
        return respond(
            ctx,
            embeds::error("Failed to time out", embeds::trim(&cause.to_string(), 500)),
        )
        .await;
    }

    let describe = duration::describe(span);
    respond(
        ctx,
        embeds::ok(
            "Timed out",
            format!("{} is timed out for {describe}.", user_mention(user.id)),
        ),
    )
    .await?;

    let extra = format!("Duration: {describe}");
    record(
        ctx,
        guild_id,
        "Timeout",
        Some(&user),
        reason.as_deref(),
        Some(&extra),
    )
    .await;
    Ok(())
}

/// Removes a user's timeout
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MODERATE_MEMBERS",
    required_permissions = "MODERATE_MEMBERS",
    required_bot_permissions = "MODERATE_MEMBERS"
)]
pub async fn untimeout(
    ctx: Context<'_>,
    #[description = "Whose timeout to remove"] user: serenity::User,
    #[description = "Reason"] reason: Option<String>,
) -> Result<(), Error> {
    util::defer(ctx).await?;
    let guild_id = util::guild_id(ctx)?;

    // Tirar um silenciamento e ato de moderacao como qualquer outro: sem a
    // checagem de hierarquia, um mod com ModerateMembers desfazia o
    // silenciamento que um admin acima dele tinha aplicado.
    if present_and_allowed(ctx, guild_id, &user).await?.is_none() {
        return Ok(());
    }

    let audit = util::reason(ctx, reason.as_deref());
    let edit = serenity::EditMember::new()
        .enable_communication()
        .audit_log_reason(&audit);
    if let Err(cause) = guild_id.edit_member(ctx, user.id, edit).await {
        return respond(
            ctx,
            embeds::error(
                "Failed to remove the timeout",
                embeds::trim(&cause.to_string(), 500),
            ),
        )
        .await;
    }

    respond(
        ctx,
        embeds::ok(
            "Timeout removed",
            format!("{} is no longer timed out.", user_mention(user.id)),
        ),
    )
    .await?;

    record(
        ctx,
        guild_id,
        "Timeout removed",
        Some(&user),
        reason.as_deref(),
        None,
    )
    .await;
    Ok(())
}

// ReadMessageHistory alem de ManageMessages: a leitura das mensagens precisa
// dela, e sem declara-la a falta de permissao virava erro cru da API no meio
// da execucao.
/// Deletes recent messages in the channel
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_MESSAGES",
    required_permissions = "MANAGE_MESSAGES",
    required_bot_permissions = "MANAGE_MESSAGES | READ_MESSAGE_HISTORY"
)]
pub async fn purge(
    ctx: Context<'_>,
    #[description = "How many messages to check (1 to 100)"] amount: i64,
    #[description = "Only delete messages from this user"] user: Option<serenity::User>,
) -> Result<(), Error> {
    util::defer(ctx).await?;

    if !(1..=100).contains(&amount) {
        return respond(
            ctx,
            embeds::error("Invalid value", "The amount must be between 1 and 100."),
        )
        .await;
    }

    let guild_id = util::guild_id(ctx)?;

    // Com filtro de usuario, o /purge e uma acao CONTRA alguem, e passa pela
    // mesma hierarquia do resto. Sem filtro e limpeza de canal, que nao tem
    // alvo - por isso a checagem so acontece quando ha um.
    if let Some(user) = &user
        && let Some(member) = hierarchy::try_get_member(ctx, guild_id, user.id).await?
        && let Some(blocked) = util::blocked(ctx, &member).await?
    {
        return respond(ctx, blocked).await;
    }

    let channel = ctx.channel_id();
    let result: Result<usize, serenity::Error> = async {
        let messages = channel
            .messages(ctx, serenity::GetMessages::new().limit(amount as u8))
            .await?;

        // A API recusa apagar em lote mensagem com mais de 14 dias, e uma
        // unica dessas no meio derruba a chamada inteira - por isso ela e
        // filtrada aqui em vez de descobrirmos no erro.
        let cutoff = chrono::Utc::now().timestamp() - 14 * 86_400;
        let target: Vec<serenity::MessageId> = messages
            .iter()
            .filter(|m| m.id.created_at().unix_timestamp() > cutoff)
            .filter(|m| user.as_ref().is_none_or(|u| m.author.id == u.id))
            .map(|m| m.id)
            .collect();

        let audit = util::reason(ctx, None);
        match target.as_slice() {
            [] => {}
            // Uma mensagem so nao pode ir pelo bulk delete: o endpoint exige
            // de 2 a 100 ids e responde 400 com um unico, o que virava
            // "Failed to delete" para o caso mais banal.
            [one] => {
                ctx.http()
                    .delete_message(channel, *one, Some(&audit))
                    .await?;
            }
            many => {
                ctx.http()
                    .delete_messages(channel, &json!({ "messages": many }), Some(&audit))
                    .await?;
            }
        }
        Ok(target.len())
    }
    .await;

    let count = match result {
        Ok(0) => {
            return respond(
                ctx,
                embeds::info(
                    "Nothing to delete",
                    "No messages from the last 14 days matched the filter. Discord doesn't allow bulk-deleting messages older than that.",
                ),
            )
            .await;
        }
        Ok(count) => count,
        Err(cause) => {
            return respond(
                ctx,
                embeds::error("Failed to delete", embeds::trim(&cause.to_string(), 500)),
            )
            .await;
        }
    };

    let suffix = match &user {
        None => ".".to_owned(),
        Some(user) => format!(" from {}.", user_mention(user.id)),
    };
    respond(
        ctx,
        embeds::ok(
            "Purge complete",
            format!("{count} message(s) deleted{suffix}"),
        ),
    )
    .await?;

    // Sem filtro de usuario nao existe "alvo": passar o moderador aqui fazia
    // o embed dizer "User: @mod / Moderator: @mod", como se o moderador
    // tivesse feito uma limpeza contra si mesmo.
    let extra = format!("{count} message(s) in {}", channel_mention(channel));
    record(
        ctx,
        guild_id,
        "Message purge",
        user.as_ref(),
        None,
        Some(&extra),
    )
    .await;
    Ok(())
}

/// Sets the channel's slowmode
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_CHANNELS",
    required_permissions = "MANAGE_CHANNELS",
    required_bot_permissions = "MANAGE_CHANNELS"
)]
pub async fn slowmode(
    ctx: Context<'_>,
    #[description = "Delay between messages (0 turns it off, max 21600)"] seconds: i64,
) -> Result<(), Error> {
    util::defer(ctx).await?;

    if !(0..=21600).contains(&seconds) {
        return respond(
            ctx,
            embeds::error(
                "Invalid value",
                "The delay must be between 0 and 21600 seconds (6 hours).",
            ),
        )
        .await;
    }

    let channel = ctx.channel_id();
    if let Err(cause) = channel
        .edit(
            ctx,
            serenity::EditChannel::new().rate_limit_per_user(seconds as u16),
        )
        .await
    {
        return respond(
            ctx,
            embeds::error(
                "Failed to apply slowmode",
                embeds::trim(&cause.to_string(), 500),
            ),
        )
        .await;
    }

    let mention = channel_mention(channel);
    respond(
        ctx,
        if seconds == 0 {
            embeds::ok("Slowmode off", format!("{mention} is back to normal."))
        } else {
            embeds::ok(
                "Slowmode on",
                format!("One message every {seconds} second(s) in {mention}."),
            )
        },
    )
    .await
}

// Exige Manage Roles de quem usa, e nao so Manage Channels: o que o comando
// faz e escrever um permission overwrite, e o Discord pede Manage Roles
// (Gerenciar Permissoes) para isso. Pedindo so Manage Channels, o bot
// emprestava a PROPRIA permissao de cargos para alguem executar uma acao que
// aquela pessoa nao pode fazer a mao.
/// Stops @everyone from sending messages in the channel
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_CHANNELS | MANAGE_ROLES",
    required_permissions = "MANAGE_CHANNELS | MANAGE_ROLES",
    required_bot_permissions = "MANAGE_ROLES"
)]
pub async fn lock(
    ctx: Context<'_>,
    #[description = "Reason for locking"] reason: Option<String>,
) -> Result<(), Error> {
    set_lock(ctx, true, reason).await
}

/// Lets @everyone send messages in the channel again
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_CHANNELS | MANAGE_ROLES",
    required_permissions = "MANAGE_CHANNELS | MANAGE_ROLES",
    required_bot_permissions = "MANAGE_ROLES"
)]
pub async fn unlock(
    ctx: Context<'_>,
    #[description = "Reason for unlocking"] reason: Option<String>,
) -> Result<(), Error> {
    set_lock(ctx, false, reason).await
}

/// Novo (allow, deny) do @everyone. Preserva o resto do overwrite: mexer so
/// no bit de SendMessages evita apagar permissoes que alguem configurou a mao.
///
/// LIMITACAO CONHECIDA, deliberada. O /lock tira o allow explicito e poe o
/// deny; o /unlock desfaz so a segunda. Num canal onde o @everyone tinha Send
/// Messages explicitamente CONCEDIDO, o ciclo lock/unlock deixa o overwrite
/// em "neutro". Restaurar o allow exigiria lembrar o estado anterior entre
/// duas invocacoes, e adivinhar - sempre repor o allow - ABRIRIA canais que
/// estavam fechados de proposito. Entre errar concedendo e errar negando,
/// negar e o lado certo. E nao da para manter allow e deny juntos: o allow
/// venceria e o /lock nao trancaria nada.
fn lock_overwrite(
    allow: serenity::Permissions,
    deny: serenity::Permissions,
    locked: bool,
) -> (serenity::Permissions, serenity::Permissions) {
    let send = serenity::Permissions::SEND_MESSAGES;
    if locked {
        (allow - send, deny | send)
    } else {
        (allow, deny - send)
    }
}

/// Fecha e reabre o canal pelo mesmo caminho: a unica diferenca e o lado
/// para onde o SendMessages vai.
async fn set_lock(ctx: Context<'_>, locked: bool, reason: Option<String>) -> Result<(), Error> {
    util::defer(ctx).await?;
    let guild_id = util::guild_id(ctx)?;
    let channel_id = ctx.channel_id();

    let result: Result<bool, Error> = async {
        // O id do @everyone e o id do servidor.
        let everyone = serenity::RoleId::new(guild_id.get());
        let everyone_known = ctx.guild().is_some_and(|g| g.roles.contains_key(&everyone));
        if !everyone_known {
            return Ok(false);
        }

        let existing = ctx.guild_channel().await.and_then(|channel| {
            channel
                .permission_overwrites
                .into_iter()
                .find(|o| o.kind == serenity::PermissionOverwriteType::Role(everyone))
        });
        let (allow, deny) = existing.map_or(
            (
                serenity::Permissions::empty(),
                serenity::Permissions::empty(),
            ),
            |o| (o.allow, o.deny),
        );
        let (allow, deny) = lock_overwrite(allow, deny, locked);

        let audit = util::reason(ctx, reason.as_deref());
        let body = json!({
            "allow": allow.bits().to_string(),
            "deny": deny.bits().to_string(),
            "type": 0,
        });
        ctx.http()
            .create_permission(
                channel_id,
                serenity::TargetId::new(everyone.get()),
                &body,
                Some(&audit),
            )
            .await?;
        Ok(true)
    }
    .await;

    match result {
        Ok(true) => {}
        Ok(false) => {
            return respond(
                ctx,
                embeds::error(
                    "Role not found",
                    "I couldn't read this server's @everyone role.",
                ),
            )
            .await;
        }
        Err(cause) => {
            return respond(
                ctx,
                embeds::error(
                    "Failed to update the channel",
                    embeds::trim(&cause.to_string(), 500),
                ),
            )
            .await;
        }
    }

    let mention = channel_mention(channel_id);
    respond(
        ctx,
        if locked {
            embeds::ok(
                "Channel locked",
                format!("@everyone can no longer send messages in {mention}."),
            )
        } else {
            embeds::ok(
                "Channel unlocked",
                format!(
                    "The block on sending messages in {mention} was lifted. If @everyone had Send Messages explicitly granted in this channel before /lock, check the permissions: it has to be restored manually."
                ),
            )
        },
    )
    .await?;

    // Sem usuario-alvo: um lock/unlock nao age contra ninguem. Passar o
    // moderador fazia o embed sair com "User: @mod / Moderator: @mod". O
    // canal vai nos detalhes, e o rotulo padrao ("*entire channel*") e do
    // /purge.
    let mut entry = modlog::Entry::new(
        if locked {
            "Channel locked"
        } else {
            "Channel unlocked"
        },
        ctx.author(),
    )
    .extra(&mention)
    .no_target_label("—");
    if let Some(reason) = reason.as_deref() {
        entry = entry.reason(reason);
    }
    modlog::record(ctx, &ctx.data().guild_settings, guild_id, entry).await;
    Ok(())
}

/// O alvo precisa estar no servidor e abaixo de quem usa e do bot. Ja
/// responde a recusa; devolve o membro so quando pode agir.
async fn present_and_allowed(
    ctx: Context<'_>,
    guild_id: serenity::GuildId,
    user: &serenity::User,
) -> Result<Option<serenity::Member>, Error> {
    let Some(member) = hierarchy::try_get_member(ctx, guild_id, user.id).await? else {
        respond(
            ctx,
            embeds::error(
                "Not in this server",
                format!("{} is not in this server.", user_mention(user.id)),
            ),
        )
        .await?;
        return Ok(None);
    };

    if let Some(blocked) = util::blocked(ctx, &member).await? {
        respond(ctx, blocked).await?;
        return Ok(None);
    }
    Ok(Some(member))
}

/// `ModerationLog.RecordAsync` com o moderador sendo quem usou o comando.
async fn record(
    ctx: Context<'_>,
    guild_id: serenity::GuildId,
    action: &str,
    target: Option<&serenity::User>,
    reason: Option<&str>,
    extra: Option<&str>,
) {
    let mut entry = modlog::Entry::new(action, ctx.author());
    if let Some(target) = target {
        entry = entry.target(target);
    }
    if let Some(reason) = reason {
        entry = entry.reason(reason);
    }
    if let Some(extra) = extra {
        entry = entry.extra(extra);
    }
    modlog::record(ctx, &ctx.data().guild_settings, guild_id, entry).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    use poise::serenity_prelude::Permissions as P;

    #[test]
    fn lock_tira_o_allow_e_poe_o_deny_sem_mexer_no_resto() {
        let (allow, deny) =
            lock_overwrite(P::SEND_MESSAGES | P::ADD_REACTIONS, P::ATTACH_FILES, true);
        assert_eq!(allow, P::ADD_REACTIONS);
        assert_eq!(deny, P::ATTACH_FILES | P::SEND_MESSAGES);
    }

    #[test]
    fn unlock_so_tira_o_deny() {
        let (allow, deny) =
            lock_overwrite(P::ADD_REACTIONS, P::SEND_MESSAGES | P::EMBED_LINKS, false);
        assert_eq!(allow, P::ADD_REACTIONS);
        assert_eq!(deny, P::EMBED_LINKS);
    }

    #[test]
    fn comandos_com_as_permissoes_do_csharp() {
        let ban = ban();
        assert_eq!(ban.default_member_permissions, P::BAN_MEMBERS);
        assert_eq!(ban.required_permissions, P::BAN_MEMBERS);
        assert_eq!(ban.required_bot_permissions, P::BAN_MEMBERS);
        assert_eq!(ban.parameters.len(), 3);
        assert_eq!(ban.parameters[2].name, "delete_days");

        let purge = purge();
        assert_eq!(
            purge.required_bot_permissions,
            P::MANAGE_MESSAGES | P::READ_MESSAGE_HISTORY
        );
        assert_eq!(
            lock().default_member_permissions,
            P::MANAGE_CHANNELS | P::MANAGE_ROLES
        );
        assert_eq!(unlock().required_bot_permissions, P::MANAGE_ROLES);
        assert_eq!(softban().checks.len(), 1);
        assert_eq!(
            timeout().description.as_deref(),
            Some("Times out a user for a while")
        );
    }
}
