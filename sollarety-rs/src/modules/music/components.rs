//! Botoes do aviso de "Now playing" (MusicComponents.cs). Passam pelas MESMAS
//! acoes dos comandos, com as mesmas regras: o aviso fica no canal a vista de
//! todos, e um botao que pulasse a votacao seria o atalho obvio para
//! contorna-la.

use poise::serenity_prelude as serenity;
use tracing::{error, info, warn};

use super::{
    actions::{self, MusicReply},
    format, service,
};
use crate::{Error, embeds};

/// Trata um clique com custom id "music:". Roda numa task propria.
pub async fn handle(ctx: serenity::Context, interaction: serenity::ComponentInteraction) {
    let id = interaction.data.custom_id.clone();
    if let Err(e) = route(&ctx, &interaction, &id).await {
        error!(
            "[musica] falha ao tratar '{id}' de {}: {e}",
            interaction.user.id
        );
        try_fail(
            &ctx,
            &interaction,
            "Something broke while handling that button.",
        )
        .await;
    }
}

async fn route(
    ctx: &serenity::Context,
    interaction: &serenity::ComponentInteraction,
    id: &str,
) -> Result<(), Error> {
    let (Some(guild), Some(member)) = (interaction.guild_id, interaction.member.as_ref()) else {
        try_fail(ctx, interaction, "This only works inside a server.").await;
        return Ok(());
    };

    // O ultimo aviso de uma faixa que ja acabou perde os botoes, mas um
    // clique pode chegar no intervalo - ou o bot pode ter reiniciado.
    if service::get(guild).is_none() {
        respond(
            ctx,
            interaction,
            MusicReply::private(embeds::info(
                "Nothing is playing",
                "These buttons belong to a song that already ended.",
            )),
        )
        .await?;
        return Ok(());
    }

    let cache = &ctx.cache;
    let reply = match id {
        format::PAUSE_ID => actions::set_paused(cache, member, guild, None),
        format::SKIP_ID => actions::skip(cache, member, guild),
        format::STOP_ID => actions::stop(cache, member, guild),
        // Consulta: so quem clicou precisa ver.
        format::QUEUE_ID => MusicReply {
            ephemeral: true,
            ..actions::queue(guild, 1)
        },
        _ => {
            info!("[musica] custom id desconhecido: '{id}'");
            MusicReply::private(embeds::error(
                "Unknown button",
                "This button is from an older version of the bot.",
            ))
        }
    };

    respond(ctx, interaction, reply).await?;
    Ok(())
}

async fn respond(
    ctx: &serenity::Context,
    interaction: &serenity::ComponentInteraction,
    reply: MusicReply,
) -> Result<(), serenity::Error> {
    let message = serenity::CreateInteractionResponseMessage::new()
        .embed(reply.embed)
        .allowed_mentions(serenity::CreateAllowedMentions::new())
        .ephemeral(reply.ephemeral);
    interaction
        .create_response(
            &ctx.http,
            serenity::CreateInteractionResponse::Message(message),
        )
        .await
}

async fn try_fail(
    ctx: &serenity::Context,
    interaction: &serenity::ComponentInteraction,
    message: &str,
) {
    let response = serenity::CreateInteractionResponseMessage::new()
        .embed(embeds::error("Error", message))
        .ephemeral(true);
    if let Err(e) = interaction
        .create_response(
            &ctx.http,
            serenity::CreateInteractionResponse::Message(response),
        )
        .await
    {
        warn!("[musica] nao consegui avisar a falha do botao: {e}");
    }
}
