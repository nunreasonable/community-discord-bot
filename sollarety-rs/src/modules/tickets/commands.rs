//! Os comandos de ticket (commands/Tickets.cs).
//!
//! O /ticket e aberto a qualquer um de proposito - e a porta de entrada, e
//! trancar a porta com permissao derrotaria o proposito. O resto exige a
//! permissao correspondente do Discord, como no restante do bot.
//!
//! Toda a logica mora em `service`, porque o painel de botoes precisa
//! executar as MESMAS acoes sem passar por comando nenhum.

use poise::serenity_prelude as serenity;
use serenity::{CreateEmbed, EditInteractionResponse};

use super::{
    service::{self, CloseOutcome, PARTICIPANT_PERMISSIONS, PREFIX, Target, TicketSettings},
    types::{self, TypeChoice},
};
use crate::{Context, Error, audit_reason, embeds, store::Ticket};

/// Edita a resposta deferida, como o EditResponseAsync do C#. Explicito, e
/// nao pelo `ctx.send` (que vira followup depois do defer).
pub async fn edit(ctx: Context<'_>, embed: CreateEmbed) -> Result<(), Error> {
    match ctx {
        poise::Context::Application(app) => {
            app.interaction
                .edit_response(ctx, EditInteractionResponse::new().embed(embed))
                .await?;
        }
        poise::Context::Prefix(_) => {
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
        }
    }
    Ok(())
}

fn guild(ctx: Context<'_>) -> Result<serenity::GuildId, Error> {
    ctx.guild_id()
        .ok_or_else(|| "comando de servidor fora de servidor".into())
}

/// Open a ticket with the staff
#[poise::command(
    slash_command,
    rename = "ticket",
    guild_only,
    required_bot_permissions = "MANAGE_CHANNELS | MANAGE_ROLES",
    check = "crate::cooldown::user::<2, 60>"
)]
pub async fn ticket(
    ctx: Context<'_>,
    #[description = "In a few words, what this is about"] subject: String,
    // As escolhas saem de types::ALL, e nao de uma lista escrita a mao: era
    // a segunda copia da lista de tipos.
    #[rename = "type"]
    #[description = "What kind of ticket this is"]
    kind: Option<TypeChoice>,
    #[description = "Tell us what happened"] details: Option<String>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = guild(ctx)?;

    let Some(settings) = service::try_get_settings(ctx.data(), guild_id) else {
        return edit(ctx, service::not_configured()).await;
    };

    let key = kind.map_or("duvida", |k| k.0.key);
    let opened = service::open(
        ctx.serenity_context(),
        ctx.data(),
        guild_id,
        ctx.author(),
        &settings,
        key,
        &subject,
        details.as_deref(),
    )
    .await?;

    edit(
        ctx,
        match opened {
            Ok(channel) => embeds::ok("Ticket opened", format!("Continue in <#{channel}>.")),
            Err(refusal) => refusal,
        },
    )
    .await
}

/// Post the ticket panel in this channel
#[poise::command(
    slash_command,
    rename = "ticket-panel",
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD",
    required_bot_permissions = "SEND_MESSAGES"
)]
pub async fn ticket_panel(
    ctx: Context<'_>,
    #[description = "Panel title"] title: Option<String>,
    #[description = "Panel description text"] text: Option<String>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = guild(ctx)?;

    if service::try_get_settings(ctx.data(), guild_id).is_none() {
        return edit(ctx, service::not_configured()).await;
    }

    let embed = CreateEmbed::new()
        .title(embeds::safe_trim(
            title.as_deref().unwrap_or("Need to talk to the staff?"),
            200,
        ))
        .description(embeds::safe_trim(
            text.as_deref().unwrap_or(
                "Pick the kind of help you need below. A private channel will be created just for you and the staff.",
            ),
            2000,
        ))
        .colour(embeds::colors::BLURPLE);

    // Um botao por tipo. Os custom ids nao carregam nada alem do tipo, e e
    // isso que faz este painel continuar funcionando daqui a meses, inclusive
    // depois de o bot reiniciar.
    let message = serenity::CreateMessage::new()
        .embed(embed)
        .components(panel_rows());

    if let Err(cause) = ctx.channel_id().send_message(ctx, message).await {
        return edit(
            ctx,
            embeds::error(
                "Couldn't post the panel",
                embeds::trim(&cause.to_string(), 500),
            ),
        )
        .await;
    }

    edit(
        ctx,
        embeds::ok(
            "Panel posted",
            format!("The panel is live in <#{}>.", ctx.channel_id()),
        ),
    )
    .await
}

/// Os botoes do painel em linhas de cinco: uma action row do Discord
/// comporta cinco botoes, e um quinto tipo acrescentado em `types::ALL`
/// derrubaria o comando em vez de simplesmente quebrar a linha.
fn panel_rows() -> Vec<serenity::CreateActionRow> {
    types::ALL
        .chunks(5)
        .map(|row| {
            serenity::CreateActionRow::Buttons(
                row.iter()
                    .map(|t| {
                        serenity::CreateButton::new(format!("{PREFIX}open:{}", t.key))
                            .style(serenity::ButtonStyle::Primary)
                            .label(t.label)
                            .emoji(serenity::ReactionType::Unicode(t.emoji.into()))
                    })
                    .collect(),
            )
        })
        .collect()
}

/// Close the ticket in this channel
#[poise::command(
    slash_command,
    rename = "ticket-close",
    guild_only,
    required_bot_permissions = "MANAGE_CHANNELS"
)]
pub async fn ticket_close(
    ctx: Context<'_>,
    #[description = "How it was resolved"] reason: Option<String>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = guild(ctx)?;

    let (settings, ticket) = match resolve(ctx, guild_id).await? {
        Ok(found) => found,
        Err(refusal) => return edit(ctx, refusal).await,
    };

    let member = ctx
        .author_member()
        .await
        .ok_or("nao consegui ler o membro de quem chamou")?
        .into_owned();
    if ticket.opener_id != ctx.author().id.get()
        && !service::is_staff(
            ctx.serenity_context().cache.as_ref(),
            guild_id,
            &member,
            &settings,
        )
    {
        return edit(
            ctx,
            embeds::error(
                "No permission",
                "Only the person who opened the ticket or the staff can close it.",
            ),
        )
        .await;
    }

    let outcome = service::close(
        ctx.serenity_context(),
        ctx.data(),
        guild_id,
        ctx.channel_id(),
        &ticket,
        ctx.author(),
        reason.as_deref(),
        &settings,
    )
    .await?;

    match outcome {
        CloseOutcome::KeptChannel(detail) => {
            edit(ctx, embeds::error("Closed, but not archived", detail)).await
        }
        CloseOutcome::Archived => {
            // O canal deixou de existir; a edicao da resposta vai falhar e
            // tudo bem.
            let _ = edit(
                ctx,
                embeds::ok(
                    "Ticket closed",
                    "The conversation was archived in the log channel.",
                ),
            )
            .await;
            Ok(())
        }
    }
}

/// Add someone to the ticket in this channel
#[poise::command(
    slash_command,
    rename = "ticket-add",
    guild_only,
    required_bot_permissions = "MANAGE_ROLES"
)]
pub async fn ticket_add(
    ctx: Context<'_>,
    #[description = "Who to add to the ticket"] user: serenity::User,
) -> Result<(), Error> {
    set_participant(ctx, user, true).await
}

/// Remove someone from the ticket in this channel
#[poise::command(
    slash_command,
    rename = "ticket-remove",
    guild_only,
    required_bot_permissions = "MANAGE_ROLES"
)]
pub async fn ticket_remove(
    ctx: Context<'_>,
    #[description = "Who to remove from the ticket"] user: serenity::User,
) -> Result<(), Error> {
    set_participant(ctx, user, false).await
}

/// Entra e sai pelo mesmo caminho: a unica diferenca e o lado para onde o
/// overwrite vai.
async fn set_participant(ctx: Context<'_>, user: serenity::User, add: bool) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = guild(ctx)?;

    let (settings, ticket) = match resolve(ctx, guild_id).await? {
        Ok(found) => found,
        Err(refusal) => return edit(ctx, refusal).await,
    };

    let member = ctx
        .author_member()
        .await
        .ok_or("nao consegui ler o membro de quem chamou")?
        .into_owned();
    if !service::is_staff(
        ctx.serenity_context().cache.as_ref(),
        guild_id,
        &member,
        &settings,
    ) {
        return edit(
            ctx,
            embeds::error(
                "Staff only",
                "Only staff can add people to or remove them from a ticket.",
            ),
        )
        .await;
    }

    if user.id.get() == ticket.opener_id && !add {
        return edit(
            ctx,
            embeds::error(
                "Invalid target",
                "You can't remove the person who opened the ticket. Close the ticket instead.",
            ),
        )
        .await;
    }

    let target_member =
        crate::hierarchy::try_get_member(ctx.serenity_context(), guild_id, user.id).await?;
    if target_member.is_none() {
        return edit(
            ctx,
            embeds::error(
                "Not in this server",
                format!("<@{}> isn't in this server.", user.id),
            ),
        )
        .await;
    }

    if let Err(cause) = write_participant(ctx, &user, &ticket, add).await {
        return edit(
            ctx,
            embeds::error(
                "Couldn't change access",
                embeds::trim(&cause.to_string(), 500),
            ),
        )
        .await;
    }

    edit(
        ctx,
        if add {
            embeds::ok("Added", format!("<@{}> can now see this ticket.", user.id))
        } else {
            embeds::ok(
                "Removed",
                format!("<@{}> can no longer see this ticket.", user.id),
            )
        },
    )
    .await
}

/// Preserva o resto do overwrite e mexe so nos bits que interessam, como o
/// /lock faz. O PUT SUBSTITUI a linha inteira da pessoa: escrevendo allow e
/// deny do zero, um /ticket-add no autor de um ticket ja trancado apagava o
/// `deny SendMessages` e destrancava a conversa sem ninguem pedir - fora
/// qualquer ajuste que um admin tivesse feito a mao no canal.
async fn write_participant(
    ctx: Context<'_>,
    user: &serenity::User,
    ticket: &Ticket,
    add: bool,
) -> Result<(), Error> {
    let overwrites = service::channel_overwrites(ctx.serenity_context(), ctx.channel_id()).await?;
    let target = Target::Member(user.id);
    let current = service::current_bits(&overwrites, target);

    let bits = if add {
        // A lista vem do servico, e nao copiada a mao: a copia que existia no
        // C# ja tinha divergido, sem AddReactions.
        service::grant(current, PARTICIPANT_PERMISSIONS)
    } else {
        // Nega o acesso em vez de apagar o overwrite: quem tem o cargo da
        // equipe enxerga o canal pelo cargo, e simplesmente remover a linha
        // dessa pessoa nao a tiraria de lugar nenhum.
        service::revoke(current, serenity::Permissions::VIEW_CHANNEL)
    };

    let what = if add {
        format!("added to ticket {}", ticket.id)
    } else {
        format!("removed from ticket {}", ticket.id)
    };
    let reason = audit_reason::for_actor(&ctx.author().tag(), Some(&what));
    service::put_overwrite(
        ctx.serenity_context(),
        ctx.channel_id(),
        target,
        bits,
        &reason,
    )
    .await?;
    Ok(())
}

/// Config + ticket do canal atual, ou o embed de recusa. Os tres comandos
/// que agem dentro de um ticket comecam exatamente igual.
async fn resolve(
    ctx: Context<'_>,
    guild_id: serenity::GuildId,
) -> Result<Result<(TicketSettings, Ticket), CreateEmbed>, Error> {
    let Some(settings) = service::try_get_settings(ctx.data(), guild_id) else {
        return Ok(Err(service::not_configured()));
    };

    let Some(ticket) = ctx
        .data()
        .tickets
        .by_channel(ctx.channel_id().get())
        .await?
    else {
        return Ok(Err(embeds::error(
            "Not a ticket",
            "Use this command inside a ticket channel.",
        )));
    };

    if !ticket.is_open() {
        return Ok(Err(embeds::error(
            "Ticket closed",
            "This ticket has already been closed.",
        )));
    }

    Ok(Ok((settings, ticket)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn painel_quebra_em_linhas_de_cinco() {
        assert_eq!(panel_rows().len(), types::ALL.len().div_ceil(5));
    }

    #[test]
    fn comandos_com_os_nomes_e_opcoes_do_csharp() {
        let t = ticket();
        assert_eq!(t.name, "ticket");
        let names: Vec<_> = t.parameters.iter().map(|p| &*p.name).collect();
        assert_eq!(names, ["subject", "type", "details"]);
        assert_eq!(t.parameters[1].choices.len(), types::ALL.len());
        assert_eq!(t.checks.len(), 1);

        assert_eq!(ticket_panel().name, "ticket-panel");
        assert!(ticket_panel().default_member_permissions.manage_guild());
        assert_eq!(ticket_close().name, "ticket-close");
        assert_eq!(ticket_add().name, "ticket-add");
        assert_eq!(ticket_remove().name, "ticket-remove");
        assert!(ticket_remove().required_bot_permissions.manage_roles());
    }
}
