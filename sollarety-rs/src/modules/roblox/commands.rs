//! Verificacao de conta Roblox, no molde do BloxLink (Verification.cs).
//!
//! Os nomes (/verify, /unverify, /update, /whois) sao os do BloxLink de
//! proposito: e o que quem vem de outro servidor ja sabe digitar.
//!
//! A logica mora no flow/service. Aqui fica so a porta de entrada, porque o
//! botao do painel precisa fazer exatamente o mesmo que o /verify.
//!
//! So servidor por comando, e nao no modulo: o /unverify desfaz um vinculo
//! GLOBAL e funciona em qualquer lugar - inclusive como app pessoal. Os
//! outros quatro mexem em cargo e apelido do servidor e continuam exigindo um.

use std::sync::atomic::Ordering;

use poise::{CreateReply, serenity_prelude as serenity};

use super::{api, flow, service};
use crate::{Context, Error, audit_reason, embeds, hierarchy, scope};

/// A interacao por tras do comando. Todo comando daqui e slash.
fn interaction<'a>(ctx: Context<'a>) -> Result<&'a serenity::CommandInteraction, Error> {
    match ctx {
        poise::Context::Application(app) => Ok(app.interaction),
        poise::Context::Prefix(_) => Err("comando de verificacao fora de slash".into()),
    }
}

/// O `EditResponseAsync` do C#: troca a resposta deferida pelo embed.
async fn edit(ctx: Context<'_>, embed: serenity::CreateEmbed) -> Result<(), Error> {
    interaction(ctx)?
        .edit_response(ctx, serenity::EditInteractionResponse::new().embed(embed))
        .await?;
    Ok(())
}

/// O membro de quem chamou, ja com os cargos.
async fn author_member(ctx: Context<'_>) -> Result<serenity::Member, Error> {
    Ok(ctx
        .author_member()
        .await
        .ok_or("membro de quem chamou indisponivel")?
        .into_owned())
}

/// Links your Roblox account through the official Roblox login
#[poise::command(slash_command, guild_only, check = "crate::cooldown::user::<3, 60>")]
pub async fn verify(ctx: Context<'_>) -> Result<(), Error> {
    let poise::Context::Application(app) = ctx else {
        return Ok(());
    };
    let guild_id = ctx.guild_id().ok_or("sem servidor")?;
    let member = author_member(ctx).await?;

    let reply = flow::Reply::new(app.interaction.id, &app.interaction.token);
    let result = flow::begin(
        ctx.serenity_context(),
        ctx.data(),
        &reply,
        guild_id,
        &member,
    )
    .await;
    // O flow responde pela interacao direto; o poise precisa saber, senao a
    // mensagem de erro tentaria uma segunda resposta inicial.
    if reply.responded() {
        app.has_sent_initial_response.store(true, Ordering::SeqCst);
    }
    result
}

/// Unlinks your Roblox account, across all servers
#[poise::command(slash_command)]
pub async fn unverify(ctx: Context<'_>) -> Result<(), Error> {
    let Some(link) = ctx.data().links.for_user(ctx.author().id.get()) else {
        ctx.send(CreateReply::default().ephemeral(true).embed(embeds::info(
            "Nothing to undo",
            "Your Discord account isn't linked to any Roblox account.",
        )))
        .await?;
        return Ok(());
    };

    let here = if scope::bot_guild(ctx).is_some() {
        "Here, verification roles are removed right away; in other servers, on the next `/update`. "
    } else {
        "Verification roles are removed in each server on the next `/update`. "
    };

    // Confirmacao por botao: o vinculo e global, e desfaze-lo tira cargo de
    // verificacao em todo servidor na proxima atualizacao.
    let embed = serenity::CreateEmbed::new()
        .title("Unlink your Roblox account?")
        .colour(embeds::colors::BLURPLE)
        .description(format!(
            "You're currently linked to {}.\n\nUnlinking applies to **every server** that uses Sollarety. {here}To link again, run `/verify` in a server.",
            flow::describe(&link)
        ));
    let button = serenity::CreateButton::new(flow::UNLINK_ID)
        .style(serenity::ButtonStyle::Danger)
        .label("Unlink");

    ctx.send(
        CreateReply::default()
            .ephemeral(true)
            .embed(embed)
            .components(vec![serenity::CreateActionRow::Buttons(vec![button])]),
    )
    .await?;
    Ok(())
}

/// Re-reads the Roblox account and reapplies roles and nickname
#[poise::command(slash_command, guild_only, check = "crate::cooldown::user::<5, 60>")]
pub async fn update(
    ctx: Context<'_>,
    #[description = "Who to update. Empty updates you; someone else requires Manage Roles"]
    user: Option<serenity::User>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let data = ctx.data();
    let guild_id = ctx.guild_id().ok_or("sem servidor")?;

    let target = user.unwrap_or_else(|| ctx.author().clone());
    let is_self = target.id == ctx.author().id;

    // Atualizar outra pessoa so reaplica o estado dela - nao da cargo que ela
    // nao mereca. Mesmo assim mexe em cargo alheio, entao pede a permissao de
    // quem mexe em cargo.
    if !is_self {
        let actor = author_member(ctx).await?;
        let permissions = match service::GuildView::cached(&ctx.serenity_context().cache, guild_id)
        {
            Some((guild, _)) => guild.permissions(actor.user.id.get(), &actor.roles),
            None => actor.permissions.unwrap_or_default(),
        };
        if !permissions
            .intersects(serenity::Permissions::MANAGE_ROLES | serenity::Permissions::ADMINISTRATOR)
        {
            return edit(ctx, embeds::error(
                "No permission",
                "Updating someone else requires **Manage Roles**. Without the `user` option, you update yourself.",
            ))
            .await;
        }
    }

    if target.bot {
        return edit(
            ctx,
            embeds::error("Invalid target", "Bots don't have Roblox accounts."),
        )
        .await;
    }

    if !service::is_configured(data.guild_settings.for_guild(guild_id.get()).as_ref()) {
        return edit(ctx, embeds::info(
            "Nothing configured",
            "This server hasn't set up Roblox verification yet, so there are no roles to apply. \
             Someone with Manage Server can set it up with `/config verify-role` and `/bind add`.",
        ))
        .await;
    }

    let Some(member) = hierarchy::try_get_member(ctx, guild_id, target.id).await? else {
        return edit(
            ctx,
            embeds::error(
                "Not in this server",
                format!("<@{}> isn't in this server.", target.id),
            ),
        )
        .await;
    };

    let mut link = data.links.for_user(target.id.get());
    if let Some(current) = &link {
        // Relido do zero: o /update existe justamente para pegar troca de
        // nome ou promocao no grupo que o cache de cinco minutos esconderia.
        api::forget(current.roblox_id);
        if let api::Lookup::Found(user) = api::get_user(&data.http, current.roblox_id, true).await
            && let Some(refreshed) = data
                .links
                .refresh(
                    target.id.get(),
                    user.id,
                    &user.name,
                    &user.display_name,
                    user.created,
                )
                .await?
        {
            link = Some(refreshed);
        }
    }

    let reason = audit_reason::for_actor(&ctx.author().tag(), Some("Roblox verification /update"));
    let report = service::apply(
        ctx.serenity_context(),
        data,
        guild_id,
        &member,
        link.as_ref(),
        &reason,
        true,
    )
    .await;

    let mention = format!("<@{}>", target.id);
    let description = match &link {
        None => format!(
            "{mention} has no linked Roblox account.{}",
            if is_self {
                " Use `/verify` to link one."
            } else {
                ""
            }
        ),
        Some(link) => format!("{mention} → {}", flow::describe(link)),
    };

    let mut embed = serenity::CreateEmbed::new()
        .title(if is_self {
            "You've been updated"
        } else {
            "Member updated"
        })
        .colour(embeds::colors::SPRING_GREEN)
        .description(description);

    if let Some(refusal) = &report.refusal {
        embed = embed.field(
            "Not approved in this server",
            embeds::trim(&format!("The account is linked, but {refusal}"), 1024),
            false,
        );
    }
    embed = embed.field("Changes", report.describe(), false);

    edit(ctx, embed).await
}

/// Shows the Roblox account linked to someone
#[poise::command(slash_command, guild_only)]
pub async fn whois(
    ctx: Context<'_>,
    #[description = "Who to look up"] user: serenity::User,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().ok_or("sem servidor")?;

    // So membros DESTE servidor. A opcao de usuario aceita qualquer id
    // colado, e sem isto o /whois viraria uma busca do vinculo Roblox de
    // qualquer pessoa do Discord - a politica de privacidade promete que so
    // quem divide um servidor com ela ve.
    if hierarchy::try_get_member(ctx, guild_id, user.id)
        .await?
        .is_none()
    {
        return edit(
            ctx,
            embeds::error(
                "Not in this server",
                format!("<@{}> isn't in this server.", user.id),
            ),
        )
        .await;
    }

    let embed = match ctx.data().links.for_user(user.id.get()) {
        None => embeds::info(
            "Not linked",
            format!("<@{}> has no linked Roblox account.", user.id),
        ),
        Some(link) => {
            let mut embed = serenity::CreateEmbed::new()
                .title("Linked Roblox account")
                .colour(embeds::colors::BLURPLE)
                .description(format!("<@{}> → {}", user.id, flow::describe(&link)))
                .field("Roblox ID", format!("`{}`", link.roblox_id), true)
                .field(
                    "Linked on",
                    format!("<t:{}:D>", link.verified_at_utc.timestamp()),
                    true,
                );
            if let Some(created) = link.roblox_created_utc {
                embed = embed.field(
                    "Account created on",
                    format!("<t:{}:D>", created.timestamp()),
                    true,
                );
            }
            embed
        }
    };

    edit(ctx, embed).await
}

/// Posts the panel with the verification button in this channel
#[poise::command(
    slash_command,
    rename = "verify-panel",
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD",
    required_bot_permissions = "SEND_MESSAGES"
)]
pub async fn verify_panel(
    ctx: Context<'_>,
    #[description = "Panel title"] title: Option<String>,
    #[description = "Panel description text"] text: Option<String>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().ok_or("sem servidor")?;

    let embed = serenity::CreateEmbed::new()
        .title(embeds::safe_trim(
            title.as_deref().unwrap_or("Verify your Roblox account"),
            200,
        ))
        .description(embeds::safe_trim(
            text.as_deref().unwrap_or(
                "Click the button below to link your Roblox account through the official Roblox login. \
                 If you've already verified in another server with Sollarety, you get your roles right away.",
            ),
            2000,
        ))
        .colour(embeds::colors::BLURPLE);

    // O botao nao carrega nada alem da acao: quem clicou e o que importa, e
    // isso vem do proprio clique. O painel sobrevive a reinicios.
    let button = serenity::CreateButton::new(flow::START_ID)
        .style(serenity::ButtonStyle::Success)
        .label("Verify")
        .emoji('✅');
    let message = serenity::CreateMessage::new()
        .embed(embed)
        .components(vec![serenity::CreateActionRow::Buttons(vec![button])]);

    if let Err(cause) = ctx.channel_id().send_message(ctx, message).await {
        return edit(
            ctx,
            embeds::error(
                "Failed to post the panel",
                embeds::trim(&cause.to_string(), 500),
            ),
        )
        .await;
    }

    let note = if service::is_configured(
        ctx.data().guild_settings.for_guild(guild_id.get()).as_ref(),
    ) {
        ""
    } else {
        "\n\n⚠️ This server hasn't set up **any** verification role yet: the button links the account, \
         but grants nothing. See `/config verify-role` and `/bind add`."
    };

    edit(
        ctx,
        embeds::ok(
            "Panel posted",
            format!("The panel is up in <#{}>.{note}", ctx.channel_id()),
        ),
    )
    .await
}

pub fn commands() -> Vec<scope::Command> {
    vec![
        scope::guild_only(verify()),
        scope::anywhere(unverify()),
        scope::guild_only(update()),
        scope::guild_only(whois()),
        scope::guild_only(verify_panel()),
    ]
}
