//! Botoes do /quiz ("quiz:") (AiQuizComponents.cs). Mesmo esquema dos outros
//! modulos: so entra aqui quem ja bateu o prefixo, sincrono ate o ponto de
//! responder "recebido", e o resto em segundo plano.

use poise::serenity_prelude as serenity;

use super::quiz::{self, QuizClick};
use crate::{Error, embeds};

pub async fn on_component(
    ctx: &serenity::Context,
    component: &serenity::ComponentInteraction,
) -> Result<(), Error> {
    let ctx = ctx.clone();
    let component = component.clone();
    tokio::spawn(async move {
        if let Err(cause) = click(&ctx, &component).await {
            tracing::error!(
                "[quiz] falha ao tratar '{}' de {}: {cause}",
                component.data.custom_id,
                component.user.id
            );
            try_fail(
                &ctx,
                &component,
                "Something broke while handling that button.",
            )
            .await;
        }
    });
    Ok(())
}

async fn click(
    ctx: &serenity::Context,
    component: &serenity::ComponentInteraction,
) -> Result<(), Error> {
    let id = &component.data.custom_id;
    let parts: Vec<&str> = id.split(':').collect();

    let parsed = if parts.len() == 4 {
        match (parts[2].parse::<usize>(), parts[3].parse::<usize>()) {
            (Ok(q), Ok(o)) => Some((parts[1], q, o)),
            _ => None,
        }
    } else {
        None
    };
    let Some((game_id, question_index, option_index)) = parsed else {
        tracing::warn!("[quiz] custom id fora do formato: '{id}'");
        try_fail(
            ctx,
            component,
            "This button is from an older version of the bot.",
        )
        .await;
        return Ok(());
    };

    // So gente de verdade joga. Nao deveria chegar aqui vindo da UI do
    // Discord, mas uma conta de bot que tentasse clicar via API nao leva
    // premio nenhum.
    if component.user.bot {
        ephemeral(
            ctx,
            component,
            embeds::info("Can't play", "Bots don't play the quiz."),
        )
        .await?;
        return Ok(());
    }

    let Some(game) = quiz::find(game_id) else {
        ephemeral(
            ctx,
            component,
            embeds::info("Quiz over", "This quiz already ended."),
        )
        .await?;
        return Ok(());
    };

    let outcome = quiz::resolve(&game, question_index, option_index, component.user.id.get());
    match outcome {
        QuizClick::Stale => {
            ephemeral(
                ctx,
                component,
                embeds::info("Too late", "This question's time is up."),
            )
            .await
        }
        QuizClick::AlreadyDecided => {
            ephemeral(
                ctx,
                component,
                embeds::info(
                    "Already answered",
                    "Someone already got this one — wait for the next question.",
                ),
            )
            .await
        }
        QuizClick::LockedOut => {
            ephemeral(
                ctx,
                component,
                embeds::info(
                    "Already tried",
                    "You already picked wrong on this question — sit this one out.",
                ),
            )
            .await
        }
        QuizClick::Wrong => {
            ephemeral(
                ctx,
                component,
                embeds::error(
                    "Wrong answer",
                    "That's not it. You're out for this question, but the next one is fresh.",
                ),
            )
            .await
        }
        // So um "parabens" efemero pra quem clicou: a revelacao publica
        // (resposta certa, quem ganhou) e o `run` que posta, depois que o
        // laco da pergunta acordar.
        QuizClick::Won => {
            ephemeral(
                ctx,
                component,
                embeds::ok(
                    "Correct!",
                    "Nice — you got it first. The result shows up in the channel in a moment.",
                ),
            )
            .await
        }
    }
}

async fn ephemeral(
    ctx: &serenity::Context,
    component: &serenity::ComponentInteraction,
    embed: serenity::CreateEmbed,
) -> Result<(), Error> {
    component
        .create_response(
            ctx,
            serenity::CreateInteractionResponse::Message(
                serenity::CreateInteractionResponseMessage::new()
                    .embed(embed)
                    .ephemeral(true),
            ),
        )
        .await?;
    Ok(())
}

async fn try_fail(
    ctx: &serenity::Context,
    component: &serenity::ComponentInteraction,
    message: &str,
) {
    let response = serenity::CreateInteractionResponse::Message(
        serenity::CreateInteractionResponseMessage::new()
            .embed(embeds::error("Error", message))
            .ephemeral(true),
    );
    if let Err(cause) = component.create_response(ctx, response).await {
        tracing::error!("[quiz] nao consegui avisar a falha do botao: {cause}");
    }
}
