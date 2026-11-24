//! O SlashCommandErrored do Program.cs: toda falha de comando vira uma
//! resposta para quem usou e uma linha no log. Sem isso, a excecao depois do
//! defer sumia e o usuario ficava no "pensando..." para sempre.

use poise::{CreateReply, FrameworkError, serenity_prelude as serenity};

use crate::{Data, Error, cooldown::CooldownHit, embeds};

pub async fn on_error(err: FrameworkError<'_, Data, Error>) {
    match err {
        FrameworkError::Command { error, ctx, .. } => {
            tracing::error!(
                "[erro] /{} de {} falhou: {error}",
                ctx.command().qualified_name,
                ctx.author().id
            );
            // O comando quase sempre ja deferiu; o send do poise vira a
            // edicao da resposta nesse caso.
            let reply = CreateReply::default()
                .embed(embeds::error(
                    "Command failed",
                    "Something went wrong while running this command. The error was logged; let the bot's admin know.",
                ))
                .ephemeral(true);
            if let Err(inner) = ctx.send(reply).await {
                tracing::error!(
                    "[erro] nao consegui avisar o usuario sobre a falha de /{}: {inner}",
                    ctx.command().qualified_name
                );
            }
        }

        // O cooldown de N usos (crate::cooldown) chega como erro de check.
        FrameworkError::CommandCheckFailed {
            error: Some(error),
            ctx,
            ..
        } if error.is::<CooldownHit>() => {
            if let Some(hit) = error.downcast_ref::<CooldownHit>() {
                ratelimit(ctx, hit.resets_at.timestamp()).await;
            }
        }

        // O cooldown nativo do poise, se algum comando usar.
        FrameworkError::CooldownHit {
            remaining_cooldown,
            ctx,
            ..
        } => {
            let at = chrono::Utc::now() + remaining_cooldown;
            ratelimit(ctx, at.timestamp()).await;
        }

        // Uma checagem que recusou o comando nao e falha - e o comportamento
        // pretendido -, mas so o cooldown respondia sozinho no C#. As outras
        // ganham uma explicacao, senao o /ban sem a permissao Ban Members no
        // bot morria em "A aplicacao nao respondeu".
        FrameworkError::MissingBotPermissions { ctx, .. } => {
            denied(
                ctx,
                "**I** don't have the permission this needs in this server — check the bot's role",
            )
            .await;
        }
        FrameworkError::MissingUserPermissions { ctx, .. } => {
            denied(ctx, "you don't have the permission this command requires").await;
        }
        FrameworkError::GuildOnly { ctx, .. } => {
            denied(ctx, "this command only works inside a server").await;
        }
        FrameworkError::CommandCheckFailed { error, ctx, .. } => {
            if let Some(error) = error {
                tracing::error!(
                    "[erro] checagem de /{} falhou: {error}",
                    ctx.command().qualified_name
                );
            }
            denied(ctx, "a permission check blocked this command").await;
        }

        other => {
            if let Err(cause) = poise::builtins::on_error(other).await {
                tracing::error!("[erro] falha ao tratar erro: {cause}");
            }
        }
    }
}

/// A frase do DisCatSharp para o cooldown, em efemero.
async fn ratelimit(ctx: crate::Context<'_>, resets_at: i64) {
    let reply = CreateReply::default()
        .content(format!("Error: Ratelimit hit\nTry again <t:{resets_at}:R>"))
        .ephemeral(true);
    if let Err(cause) = ctx.send(reply).await {
        tracing::warn!(
            "[erro] nao consegui avisar o cooldown de /{}: {cause}",
            ctx.command().qualified_name
        );
    }
}

async fn denied(ctx: crate::Context<'_>, reason: &str) {
    let name = &ctx.command().qualified_name;
    tracing::info!(
        "[comando] /{name} de {} recusado: {reason}",
        ctx.author().id
    );
    let reply = CreateReply::default()
        .embed(embeds::error("Command denied", format!("• {reason}")))
        .ephemeral(true);
    if let Err(cause) = ctx.send(reply).await {
        tracing::error!("[erro] nao consegui explicar a recusa de /{name}: {cause}");
    }
}

/// Para quem quer saber se um erro do serenity e um 404 (membro que saiu,
/// canal apagado) sem repetir o match.
pub fn is_not_found(error: &serenity::Error) -> bool {
    matches!(
        error,
        serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(r))
            if r.status_code == serenity::StatusCode::NOT_FOUND
    )
}
