//! /role add, /role remove e /role strip (o change_role e o
//! remove_from_regiment do painel do ccore).
//!
//! No ccore o portao era a patente; aqui e a permissao do Discord (Gerenciar
//! Cargos) mais a hierarquia: ninguem usa o bot para mexer em cargo da
//! propria altura para cima, nem em quem esta na propria altura para cima.
//! As regras moram em `gate::role_refusal`.

use std::time::Duration;

use poise::serenity_prelude as serenity;

use super::{
    dm,
    gate::{self, ActorFacts, Change, PREFIX, RoleFacts, respond},
};
use crate::{Context, Error, embeds, hierarchy, modlog};

/// Quanto o botao de confirmar do /role strip espera.
const CONFIRM_WINDOW: Duration = Duration::from_secs(60);

/// Adds or removes a member's roles
#[poise::command(
    slash_command,
    rename = "role",
    guild_only,
    default_member_permissions = "MANAGE_ROLES",
    required_permissions = "MANAGE_ROLES",
    required_bot_permissions = "MANAGE_ROLES",
    subcommand_required,
    subcommands("add", "remove", "strip")
)]
pub async fn role_group(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Gives a role to a member
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_ROLES",
    required_bot_permissions = "MANAGE_ROLES"
)]
async fn add(
    ctx: Context<'_>,
    #[description = "Who gets the role"] member: serenity::User,
    #[description = "The role to give"] role: serenity::Role,
    #[description = "Reason (shows in the audit log)"] reason: Option<String>,
) -> Result<(), Error> {
    change(ctx, member, role.id, reason, Change::Add).await
}

/// Takes a role away from a member
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_ROLES",
    required_bot_permissions = "MANAGE_ROLES"
)]
async fn remove(
    ctx: Context<'_>,
    #[description = "Who loses the role"] member: serenity::User,
    #[description = "The role to take away"] role: serenity::Role,
    #[description = "Reason (shows in the audit log)"] reason: Option<String>,
) -> Result<(), Error> {
    change(ctx, member, role.id, reason, Change::Remove).await
}

/// O que se sabe de quem pede, do bot e do alvo, copiado do cache.
struct Scene {
    guild_id: u64,
    owner_id: u64,
    actor: ActorFacts,
    bot_top: i32,
}

/// Le o cache UMA vez: hierarquia de membro (o `hierarchy::check` de sempre)
/// e os fatos para as regras de cargo. Err e a recusa pronta.
async fn scene(
    ctx: Context<'_>,
    guild_id: serenity::GuildId,
    target: &serenity::Member,
) -> Result<Result<Scene, serenity::CreateEmbed>, Error> {
    let actor = gate::actor(ctx).await?;
    let bot = gate::bot_member(ctx.serenity_context(), guild_id).await;

    let guild = ctx.guild().ok_or("servidor fora do cache")?;
    if let Some(blocked) = hierarchy::check(&guild, &actor, target, bot.as_ref()) {
        return Ok(Err(blocked));
    }
    // O check acima ja recusou sem o bot; o unwrap_or nunca e usado.
    let bot_top = bot.as_ref().map_or(-1, |b| hierarchy::top_role(&guild, b));
    Ok(Ok(Scene {
        guild_id: guild_id.get(),
        owner_id: guild.owner_id.get(),
        actor: ActorFacts {
            id: actor.user.id.get(),
            top: hierarchy::top_role(&guild, &actor),
            permissions: guild.member_permissions(&actor),
        },
        bot_top,
    }))
}

async fn change(
    ctx: Context<'_>,
    user: serenity::User,
    role_id: serenity::RoleId,
    reason: Option<String>,
    change: Change,
) -> Result<(), Error> {
    if !gate::ensure(ctx).await? {
        return Ok(());
    }
    ctx.defer_ephemeral().await?;
    let guild_id = ctx
        .guild_id()
        .ok_or("comando de servidor fora de servidor")?;
    let verb = match change {
        Change::Add => "add",
        Change::Remove => "remove",
    };

    let Some(target) = hierarchy::try_get_member(ctx, guild_id, user.id).await? else {
        return respond(
            ctx,
            embeds::error(
                "Not a member",
                format!("<@{}> isn't in this server.", user.id),
            ),
        )
        .await;
    };

    // O cargo vem do cache DESTE servidor, e nao do que a interacao
    // resolveu: posicao e permissoes atuais, e nada de cargo de outro lugar.
    let role = {
        let guild = ctx.guild().ok_or("servidor fora do cache")?;
        guild.roles.get(&role_id).map(RoleFacts::of)
    };
    let Some(role) = role else {
        return respond(
            ctx,
            embeds::error("Invalid role", "That role isn't in this server."),
        )
        .await;
    };

    let refusal = match scene(ctx, guild_id, &target).await? {
        Err(embed) => Some((gate::embed_text(&embed), embed)),
        Ok(s) => gate::role_refusal(s.guild_id, s.owner_id, &s.actor, s.bot_top, &role, change)
            .map(|r| {
                let text = r.sentence(role.id);
                (text.clone(), embeds::error("Role not allowed", text))
            }),
    };
    if let Some((text, embed)) = refusal {
        tracing::warn!(
            "[admin] {} tentou {verb} o cargo {} de {}: recusado - {text}",
            ctx.author().id,
            role.id,
            user.id
        );
        respond(ctx, embed).await?;
        // Tentativa recusada de mexer em cargo e o que mais interessa
        // auditar: e assim que uma escalada aparece.
        let action = match change {
            Change::Add => "Role add refused",
            Change::Remove => "Role remove refused",
        };
        let extra = format!("Role: <@&{}>\n{text}", role.id);
        let mut entry = modlog::Entry::new(action, ctx.author())
            .target(&user)
            .extra(&extra);
        if let Some(reason) = reason.as_deref() {
            entry = entry.reason(reason);
        }
        modlog::record(ctx, &ctx.data().guild_settings, guild_id, entry).await;
        return Ok(());
    }

    let has = target.roles.contains(&role_id);
    match (change, has) {
        (Change::Add, true) => {
            return respond(
                ctx,
                embeds::info(
                    "Nothing to do",
                    format!("<@{}> already has <@&{role_id}>.", user.id),
                ),
            )
            .await;
        }
        (Change::Remove, false) => {
            return respond(
                ctx,
                embeds::info(
                    "Nothing to do",
                    format!("<@{}> doesn't have <@&{role_id}>.", user.id),
                ),
            )
            .await;
        }
        _ => {}
    }

    let audit = gate::audit(ctx, reason.as_deref());
    let result = match change {
        Change::Add => {
            ctx.http()
                .add_member_role(guild_id, user.id, role_id, Some(&audit))
                .await
        }
        Change::Remove => {
            ctx.http()
                .remove_member_role(guild_id, user.id, role_id, Some(&audit))
                .await
        }
    };
    if let Err(cause) = result {
        return respond(
            ctx,
            embeds::error(
                "Failed to change the role",
                embeds::trim(&cause.to_string(), 500),
            ),
        )
        .await;
    }

    let (title, sentence, action) = match change {
        Change::Add => (
            "Role added",
            format!("<@{}> now has <@&{role_id}>.", user.id),
            "Role added",
        ),
        Change::Remove => (
            "Role removed",
            format!("<@&{role_id}> was removed from <@{}>.", user.id),
            "Role removed",
        ),
    };
    respond(ctx, embeds::ok(title, sentence)).await?;

    let extra = format!("Role: <@&{role_id}>");
    let mut entry = modlog::Entry::new(action, ctx.author())
        .target(&user)
        .extra(&extra);
    if let Some(reason) = reason.as_deref() {
        entry = entry.reason(reason);
    }
    modlog::record(ctx, &ctx.data().guild_settings, guild_id, entry).await;
    Ok(())
}

/// Removes every role you and the bot are allowed to remove
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_ROLES",
    required_bot_permissions = "MANAGE_ROLES"
)]
async fn strip(
    ctx: Context<'_>,
    #[description = "Whose roles to remove"] member: serenity::User,
    #[description = "Reason (shows in the audit log and in the DM)"] reason: Option<String>,
    #[description = "DM them about it (default: yes)"] notify: Option<bool>,
) -> Result<(), Error> {
    if !gate::ensure(ctx).await? {
        return Ok(());
    }
    ctx.defer_ephemeral().await?;
    let guild_id = ctx
        .guild_id()
        .ok_or("comando de servidor fora de servidor")?;
    let notify = notify.unwrap_or(true);
    let user = member;

    let Some((plan, skipped)) = plan_roles(ctx, guild_id, &user).await? else {
        return Ok(());
    };
    if plan.is_empty() {
        let mut text = format!("I can't remove any of <@{}>'s roles.", user.id);
        if !skipped.is_empty() {
            text.push_str("\n\n");
            text.push_str(&skipped_list(&skipped));
        }
        return respond(
            ctx,
            embeds::info("Nothing to remove", embeds::trim(&text, 4000)),
        )
        .await;
    }

    // Confirmacao: tirar todos os cargos de alguem de uma vez e o tipo de
    // clique errado que custa caro desfazer.
    let prefix = format!("{PREFIX}strip:{}:", ctx.id());
    let buttons = vec![serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new(format!("{prefix}yes"))
            .label("Remove them")
            .style(serenity::ButtonStyle::Danger),
        serenity::CreateButton::new(format!("{prefix}no"))
            .label("Cancel")
            .style(serenity::ButtonStyle::Secondary),
    ])];
    let mut text = format!(
        "These roles will be removed from <@{}>:\n{}",
        user.id,
        mentions(&plan)
    );
    if !skipped.is_empty() {
        text.push_str("\n\n");
        text.push_str(&skipped_list(&skipped));
    }
    text.push_str(if notify {
        "\n\nThey'll get a DM saying their roles in this server were removed."
    } else {
        "\n\nThey won't be notified."
    });
    let confirm = embeds::info(
        format!("Remove {} role(s)?", plan.len()),
        embeds::trim(&text, 4000),
    )
    .footer(serenity::CreateEmbedFooter::new(
        "This expires in 60 seconds.",
    ));
    if let poise::Context::Application(app) = ctx {
        app.interaction
            .edit_response(
                ctx.http(),
                serenity::EditInteractionResponse::new()
                    .embed(confirm)
                    .components(buttons),
            )
            .await?;
    }

    let confirmed = loop {
        let Some(press) = serenity::ComponentInteractionCollector::new(ctx)
            .filter({
                let prefix = prefix.clone();
                move |press| press.data.custom_id.starts_with(&prefix)
            })
            .timeout(CONFIRM_WINDOW)
            .await
        else {
            respond(ctx, embeds::info("Timed out", "Nothing was changed.")).await?;
            return Ok(());
        };

        if press.user.id != ctx.author().id || !gate::guild_allowed(press.guild_id) {
            let _ = press
                .create_response(
                    ctx,
                    serenity::CreateInteractionResponse::Message(
                        serenity::CreateInteractionResponseMessage::new()
                            .content("Only the person who ran this command can use these buttons.")
                            .ephemeral(true),
                    ),
                )
                .await;
            continue;
        }

        let yes = press.data.custom_id.ends_with(":yes");
        let embed = if yes {
            embeds::info("Removing roles…", "One moment.")
        } else {
            embeds::info("Cancelled", "Nothing was changed.")
        };
        press
            .create_response(
                ctx,
                serenity::CreateInteractionResponse::UpdateMessage(
                    serenity::CreateInteractionResponseMessage::new()
                        .embed(embed)
                        .components(Vec::new()),
                ),
            )
            .await?;
        break yes;
    };
    if !confirmed {
        return Ok(());
    }

    // De novo, com o estado de agora: em 60 s alguem pode ter mexido nos
    // cargos ou na hierarquia. Sai so o que foi confirmado E ainda passa.
    let confirmed_ids: Vec<u64> = plan.iter().map(|r| r.id).collect();
    let Some((fresh, _)) = plan_roles(ctx, guild_id, &user).await? else {
        return Ok(());
    };
    let audit = gate::audit(ctx, reason.as_deref());
    let mut removed = Vec::new();
    let mut failed = Vec::new();
    for role in fresh.iter().filter(|r| confirmed_ids.contains(&r.id)) {
        match ctx
            .http()
            .remove_member_role(
                guild_id,
                user.id,
                serenity::RoleId::new(role.id),
                Some(&audit),
            )
            .await
        {
            Ok(()) => removed.push(role.clone()),
            Err(cause) => {
                tracing::warn!(
                    "[admin] /role strip: falha ao tirar o cargo {} de {}: {cause}",
                    role.id,
                    user.id
                );
                failed.push(role.clone());
            }
        }
    }

    let dm_note = if !notify {
        "Not notified (notify was off)."
    } else if removed.is_empty() {
        "Not notified (nothing was removed)."
    } else if dm::notify_roles_removed(ctx.serenity_context(), guild_id, &user, reason.as_deref())
        .await
    {
        "Notified by DM."
    } else {
        "Couldn't DM them (DMs closed or bot blocked)."
    };

    let mut text = String::new();
    if !removed.is_empty() {
        text.push_str(&format!(
            "Removed from <@{}>:\n{}",
            user.id,
            mentions(&removed)
        ));
    } else {
        text.push_str(&format!("No roles were removed from <@{}>.", user.id));
    }
    if !failed.is_empty() {
        text.push_str(&format!("\n\nFailed to remove: {}", mentions(&failed)));
    }
    let dropped = confirmed_ids.len() - removed.len() - failed.len();
    if dropped > 0 {
        text.push_str(&format!(
            "\n\n{dropped} role(s) were skipped because things changed since you confirmed."
        ));
    }
    text.push_str(&format!("\n\n{dm_note}"));
    let embed = if failed.is_empty() && !removed.is_empty() {
        embeds::ok("Roles removed", embeds::trim(&text, 4000))
    } else {
        embeds::error("Roles partly removed", embeds::trim(&text, 4000))
    };
    respond(ctx, embed).await?;

    if !removed.is_empty() || !failed.is_empty() {
        let mut extra = format!("Removed: {}", or_none(&removed));
        if !failed.is_empty() {
            extra.push_str(&format!("\nFailed: {}", mentions(&failed)));
        }
        extra.push_str(&format!("\n{dm_note}"));
        let mut entry = modlog::Entry::new("Roles stripped", ctx.author())
            .target(&user)
            .extra(&extra);
        if let Some(reason) = reason.as_deref() {
            entry = entry.reason(reason);
        }
        modlog::record(ctx, &ctx.data().guild_settings, guild_id, entry).await;
    }
    Ok(())
}

type Plan = (Vec<RoleFacts>, Vec<(RoleFacts, gate::RoleRefusal)>);

/// O plano do strip, ja respondendo a recusa (membro fora do servidor ou
/// acima na hierarquia). None quando ja respondeu.
async fn plan_roles(
    ctx: Context<'_>,
    guild_id: serenity::GuildId,
    user: &serenity::User,
) -> Result<Option<Plan>, Error> {
    let Some(target) = hierarchy::try_get_member(ctx, guild_id, user.id).await? else {
        respond(
            ctx,
            embeds::error(
                "Not a member",
                format!("<@{}> isn't in this server.", user.id),
            ),
        )
        .await?;
        return Ok(None);
    };
    let scene = match scene(ctx, guild_id, &target).await? {
        Ok(scene) => scene,
        Err(embed) => {
            tracing::warn!(
                "[admin] {} tentou /role strip em {}: recusado - {}",
                ctx.author().id,
                user.id,
                gate::embed_text(&embed)
            );
            respond(ctx, embed.clone()).await?;
            let extra = gate::embed_text(&embed);
            modlog::record(
                ctx,
                &ctx.data().guild_settings,
                guild_id,
                modlog::Entry::new("Role strip refused", ctx.author())
                    .target(user)
                    .extra(&extra),
            )
            .await;
            return Ok(None);
        }
    };
    let roles: Vec<RoleFacts> = {
        let guild = ctx.guild().ok_or("servidor fora do cache")?;
        target
            .roles
            .iter()
            .filter_map(|id| guild.roles.get(id))
            .map(RoleFacts::of)
            .collect()
    };
    Ok(Some(gate::plan_strip(
        scene.guild_id,
        scene.owner_id,
        &scene.actor,
        scene.bot_top,
        &roles,
    )))
}

fn mentions(roles: &[RoleFacts]) -> String {
    roles
        .iter()
        .map(|r| format!("<@&{}>", r.id))
        .collect::<Vec<_>>()
        .join(" ")
}

fn or_none(roles: &[RoleFacts]) -> String {
    if roles.is_empty() {
        "none".to_owned()
    } else {
        mentions(roles)
    }
}

/// "Kept (I can't remove these): <@&1> — at or above your role ..."
fn skipped_list(skipped: &[(RoleFacts, gate::RoleRefusal)]) -> String {
    let lines: Vec<String> = skipped
        .iter()
        .map(|(role, why)| format!("• <@&{}> — {}", role.id, why.short()))
        .collect();
    format!("Kept (can't be removed here):\n{}", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(id: u64) -> RoleFacts {
        RoleFacts {
            id,
            position: 1,
            managed: false,
            permissions: serenity::Permissions::empty(),
        }
    }

    #[test]
    fn listas_de_cargo() {
        assert_eq!(mentions(&[facts(1), facts(2)]), "<@&1> <@&2>");
        assert_eq!(or_none(&[]), "none");
        let text = skipped_list(&[(facts(3), gate::RoleRefusal::AboveBot)]);
        assert!(text.contains("<@&3> — at or above my role"));
    }
}
