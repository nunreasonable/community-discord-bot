//! Advertencias com historico (commands/Warnings.cs).
//!
//! E o que da memoria a moderacao: sem isto cada punicao e um evento solto e
//! ninguem sabe se a pessoa ja foi avisada tres vezes antes.

use poise::serenity_prelude as serenity;

use super::util::{self, respond, user_mention};
use crate::{
    Context, Error, embeds,
    embeds::colors,
    hierarchy, modlog,
    store::{time, warnings::Warning},
};

/// Quantas advertencias cabem numa pagina do /warnings.
const PAGE_SIZE: usize = 10;

/// Records a warning for a user
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MODERATE_MEMBERS",
    required_permissions = "MODERATE_MEMBERS"
)]
pub async fn warn(
    ctx: Context<'_>,
    #[description = "Who to warn"] user: serenity::User,
    #[description = "Reason for the warning"] reason: String,
) -> Result<(), Error> {
    util::defer(ctx).await?;

    if user.bot {
        return respond(
            ctx,
            embeds::error("Invalid target", "There's no point in warning a bot."),
        )
        .await;
    }

    let guild_id = util::guild_id(ctx)?;

    // Recusa quando o alvo nao esta no servidor, igual a /kick e /timeout.
    // Sem isto, um moderador com apenas ModerateMembers advertia alguem que
    // saiu temporariamente - inclusive um admin acima dele -, e a advertencia
    // (indexada por userId) reaparecia quando a pessoa voltava, pulando a
    // checagem de hierarquia que so da para fazer com o membro presente.
    let Some(member) = hierarchy::try_get_member(ctx, guild_id, user.id).await? else {
        return respond(
            ctx,
            embeds::error(
                "Not in this server",
                format!("{} is not in this server.", user_mention(user.id)),
            ),
        )
        .await;
    };

    if let Some(blocked) = util::blocked(ctx, &member).await? {
        return respond(ctx, blocked).await;
    }

    // O id e gerado dentro do update do store, com o arquivo em maos: garante
    // unicidade sem corrida.
    let (id, total) = ctx
        .data()
        .warnings
        .add(Warning {
            id: String::new(),
            guild_id: guild_id.get(),
            user_id: user.id.get(),
            moderator_id: ctx.author().id.get(),
            moderator_tag: util::tag(ctx.author()),
            reason: embeds::trim(&reason, 500),
            created_at_utc: time::now(),
        })
        .await?;

    respond(
        ctx,
        embeds::ok(
            "Warning recorded",
            format!(
                "{} now has **{total}** warning(s).\nThis warning's ID: `{id}`",
                user_mention(user.id)
            ),
        ),
    )
    .await?;

    let extra = format!("ID `{id}` — {total} in total");
    let entry = modlog::Entry::new("Warning", ctx.author())
        .target(&user)
        .reason(&reason)
        .extra(&extra);
    modlog::record(ctx, &ctx.data().guild_settings, guild_id, entry).await;
    Ok(())
}

/// Lists a user's warnings
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MODERATE_MEMBERS",
    required_permissions = "MODERATE_MEMBERS"
)]
pub async fn warnings(
    ctx: Context<'_>,
    #[description = "Whose history to view"] user: serenity::User,
    #[description = "Page of the list, starting at 1"] page: Option<i64>,
) -> Result<(), Error> {
    util::defer(ctx).await?;
    let guild_id = util::guild_id(ctx)?;

    let all = ctx
        .data()
        .warnings
        .list(guild_id.get(), user.id.get())
        .await?;

    if all.is_empty() {
        return respond(
            ctx,
            embeds::info(
                "No warnings",
                format!("{} has no recorded warnings.", user_mention(user.id)),
            ),
        )
        .await;
    }

    let (current, page_count) = page_bounds(all.len(), page.unwrap_or(1));
    let description = all
        .iter()
        .skip((current - 1) * PAGE_SIZE)
        .take(PAGE_SIZE)
        .map(|w| {
            format!(
                "`{}` — <t:{}:d> by <@{}>\n> {}",
                w.id,
                w.created_at_utc.timestamp(),
                w.moderator_id,
                embeds::trim(&w.reason, 200)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    let embed = serenity::CreateEmbed::new()
        .title(format!("Warnings for {}", util::tag(&user)))
        .description(description)
        .colour(colors::ORANGE)
        .thumbnail(user.face())
        .footer(serenity::CreateEmbedFooter::new(format!(
            "Page {current}/{page_count} — {} warning(s) in total",
            all.len()
        )));

    respond(ctx, embed).await
}

/// (pagina atual, total de paginas), com a pedida presa entre 1 e o total,
/// como o Math.Clamp do C#. `count` nunca e zero aqui.
fn page_bounds(count: usize, wanted: i64) -> (usize, usize) {
    let pages = count.div_ceil(PAGE_SIZE);
    let current = wanted.clamp(1, pages as i64) as usize;
    (current, pages)
}

/// Removes a warning by its ID
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MODERATE_MEMBERS",
    required_permissions = "MODERATE_MEMBERS"
)]
pub async fn delwarn(
    ctx: Context<'_>,
    #[description = "The ID shown in /warnings"] id: String,
) -> Result<(), Error> {
    util::defer(ctx).await?;

    let wanted = id.trim();
    let guild_id = util::guild_id(ctx)?;
    let data = ctx.data();

    // Hierarquia ANTES de apagar. Sem ela, qualquer um com ModerateMembers
    // podia apagar qualquer advertencia do servidor - inclusive uma que um
    // administrador acima dele escreveu, e inclusive as escritas contra ele
    // mesmo. Apagar o registro de uma punicao e um ato de moderacao como
    // tirar a punicao.
    //
    // O que se compara e o MODERADOR QUE ESCREVEU, nao o advertido: apagar
    // uma advertencia e um ato contra o registro de quem a aplicou. Mexer no
    // que um superior escreveu exige estar acima dele; a propria advertencia
    // continua livre para o autor apagar.
    if let Some(existing) = data.warnings.find(guild_id.get(), wanted).await?
        && existing.moderator_id != ctx.author().id.get()
    {
        // Quem aplicou saiu do servidor: RECUSA, e nao libera. Sem o cargo
        // dele nao da para dizer quem estava acima de quem, e liberando
        // bastava um admin sair para as advertencias dele virarem apagaveis
        // por qualquer moderador - lavagem de ficha exatamente na hora em que
        // ninguem esta olhando.
        let issuer_id = serenity::UserId::new(existing.moderator_id);
        let Some(issuer) = hierarchy::try_get_member(ctx, guild_id, issuer_id).await? else {
            return respond(
                ctx,
                embeds::error(
                    "Role hierarchy unavailable",
                    "Whoever issued this warning is no longer in the server, so I can't check whether you outrank them. Only someone with an administrator role can sort this out manually.",
                ),
            )
            .await;
        };

        let actor = util::actor(ctx).await?;
        let outranks = {
            let guild = ctx.guild().ok_or("servidor fora do cache")?;
            hierarchy::outranks(&guild, &actor, &issuer)
        };
        if !outranks {
            return respond(
                ctx,
                embeds::error(
                    "Role hierarchy",
                    format!(
                        "This warning was issued by {}, whose role is equal to or higher than yours. Only someone above them — or they themselves — can delete it.",
                        user_mention(issuer_id)
                    ),
                ),
            )
            .await;
        }
    }

    // Preso ao servidor de proposito: um id de outro servidor nao pode ser
    // apagado a partir daqui.
    let Some(removed) = data.warnings.delete(guild_id.get(), wanted).await? else {
        return respond(
            ctx,
            embeds::error(
                "ID not found",
                format!(
                    "No warning with ID `{}` in this server. Check `/warnings`.",
                    embeds::trim(wanted, 40)
                ),
            ),
        )
        .await;
    };

    respond(
        ctx,
        embeds::ok(
            "Warning removed",
            format!(
                "Warning `{}` for <@{}> was deleted.",
                removed.id, removed.user_id
            ),
        ),
    )
    .await?;

    // Tudo o que vem DEPOIS do "sucesso" acima e best-effort. A busca do
    // usuario em especial: a conta pode ter sido apagada, e um erro subindo
    // daqui viraria "Command failed" numa advertencia que de fato foi
    // removida.
    match serenity::UserId::new(removed.user_id).to_user(ctx).await {
        Ok(target) => {
            let extra = format!(
                "ID `{}`, originally issued by <@{}>",
                removed.id, removed.moderator_id
            );
            let entry = modlog::Entry::new("Warning removed", ctx.author())
                .target(&target)
                .reason(&removed.reason)
                .extra(&extra);
            modlog::record(ctx, &data.guild_settings, guild_id, entry).await;
        }
        Err(cause) => {
            tracing::error!(
                "[warnings] falha ao registrar a remocao de {}: {cause}",
                removed.id
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagina_presa_entre_um_e_o_total() {
        assert_eq!(page_bounds(1, 1), (1, 1));
        assert_eq!(page_bounds(10, 2), (1, 1));
        assert_eq!(page_bounds(11, 2), (2, 2));
        assert_eq!(page_bounds(25, 99), (3, 3));
        assert_eq!(page_bounds(25, -4), (1, 3));
        assert_eq!(page_bounds(25, i64::MAX), (3, 3));
    }
}
