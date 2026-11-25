//! O texto longo do /send e do /dm vem por modal: opcao de slash comando nao
//! tem quebra de linha e para em 6000 caracteres somados.
//!
//! Nao usa o `execute_modal` do poise: ele responde ao envio do modal com um
//! Acknowledge e deixa as respostas seguintes para o token do COMANDO, cuja
//! resposta inicial foi o proprio modal. Aqui a resposta vai pelo token do
//! envio do modal (defer efemero e depois edicao), que e uma interacao nova
//! e vale 15 minutos a partir dele.

use std::time::Duration;

use poise::serenity_prelude as serenity;

use super::gate::PREFIX;
use crate::{Context, Error, embeds};

/// Quanto o modal espera alguem escrever. Mais que isso e desistencia.
const WAIT: Duration = Duration::from_secs(15 * 60);
const FIELD: &str = "text";

pub struct Ask<'a> {
    /// Titulo do modal (o Discord corta em 45).
    pub title: &'a str,
    /// Rotulo do campo (tambem 45).
    pub label: &'a str,
    pub placeholder: &'a str,
    pub max_length: u16,
}

/// Mostra o modal como PRIMEIRA resposta do comando e espera o envio. None
/// quando a pessoa fecha ou demora demais. O envio ja volta deferido em
/// efemero: o resto responde com `finish`.
pub async fn ask(
    ctx: Context<'_>,
    ask: Ask<'_>,
) -> Result<Option<(serenity::ModalInteraction, String)>, Error> {
    let poise::Context::Application(app) = ctx else {
        return Ok(None);
    };
    let custom_id = format!("{PREFIX}modal:{}", app.interaction.id);

    // CreateInputText::new e (estilo, ROTULO, CUSTOM ID).
    let modal = serenity::CreateModal::new(&custom_id, ask.title).components(vec![
        serenity::CreateActionRow::InputText(
            serenity::CreateInputText::new(serenity::InputTextStyle::Paragraph, ask.label, FIELD)
                .placeholder(ask.placeholder)
                .min_length(1)
                .max_length(ask.max_length)
                .required(true),
        ),
    ]);
    app.interaction
        .create_response(
            ctx.http(),
            serenity::CreateInteractionResponse::Modal(modal),
        )
        .await?;
    // A resposta inicial ja foi: se algo subir como erro daqui em diante, o
    // on_error do poise tenta followup em vez de uma segunda resposta.
    app.has_sent_initial_response
        .store(true, std::sync::atomic::Ordering::SeqCst);

    let author = ctx.author().id;
    let Some(submit) = serenity::ModalInteractionCollector::new(ctx.serenity_context())
        .filter(move |m| m.data.custom_id == custom_id && m.user.id == author)
        .timeout(WAIT)
        .await
    else {
        return Ok(None);
    };

    submit.defer_ephemeral(ctx.http()).await?;
    let text = value(&submit.data.components).unwrap_or_default();
    Ok(Some((submit, text)))
}

/// O valor do campo, dentro das action rows.
fn value(rows: &[serenity::ActionRow]) -> Option<String> {
    rows.iter()
        .flat_map(|row| &row.components)
        .find_map(|component| match component {
            serenity::ActionRowComponent::InputText(input) if input.custom_id == FIELD => {
                input.value.clone()
            }
            _ => None,
        })
}

/// Troca o "pensando..." do envio do modal pelo resultado.
pub async fn finish(
    http: impl AsRef<serenity::Http>,
    submit: &serenity::ModalInteraction,
    embed: serenity::CreateEmbed,
) -> Result<(), Error> {
    submit
        .edit_response(
            http.as_ref(),
            serenity::EditInteractionResponse::new().embed(embed),
        )
        .await?;
    Ok(())
}

/// Roda o trabalho depois do modal. Erro aqui nao pode subir para o
/// on_error: ele responderia pelo token do comando, e o "pensando..." do
/// envio do modal ficaria la para sempre. Entao loga e edita o envio.
pub async fn guard(
    ctx: Context<'_>,
    submit: &serenity::ModalInteraction,
    work: impl std::future::Future<Output = Result<(), Error>>,
) {
    if let Err(cause) = work.await {
        tracing::error!(
            "[erro] /{} de {} falhou: {cause}",
            ctx.command().qualified_name,
            ctx.author().id
        );
        let embed = embeds::error(
            "Command failed",
            "Something went wrong while running this command. The error was logged; let the bot's admin know.",
        );
        if let Err(inner) = finish(ctx.http(), submit, embed).await {
            tracing::error!(
                "[erro] nao consegui avisar o usuario sobre a falha de /{}: {inner}",
                ctx.command().qualified_name
            );
        }
    }
}
