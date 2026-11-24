//! Paginas com botoes, como a paginacao da Interactivity do DisCatSharp
//! (primeira, anterior, parar, proxima, ultima), com os botoes desligados ao
//! expirar em 3 minutos ou ao parar.
//!
//! So quem rodou o comando troca de pagina: o clique de outra pessoa ganha um
//! aviso efemero em vez do "interacao falhou" do Discord. No C# isso
//! precisava do PaginationOwnership e de um handler global; aqui o proprio
//! coletor ve o clique e responde.

use std::time::Duration;

use poise::{CreateReply, serenity_prelude as serenity};

use crate::{Context, Error};

const IDLE_LIMIT: Duration = Duration::from_secs(180);

/// Manda a primeira pagina (como resposta ou, se o comando ja deferiu, como
/// edicao da resposta) e cuida dos botoes ate expirar.
pub async fn send(
    ctx: Context<'_>,
    pages: Vec<serenity::CreateEmbed>,
    ephemeral: bool,
) -> Result<(), Error> {
    if pages.is_empty() {
        return Ok(());
    }

    let prefix = format!("{}:page:", ctx.id());
    let id = |what: &str| format!("{prefix}{what}");

    let buttons = |disabled: bool| {
        vec![serenity::CreateActionRow::Buttons(
            [
                ("first", "⏮️"),
                ("prev", "◀️"),
                ("stop", "⏹️"),
                ("next", "▶️"),
                ("last", "⏭️"),
            ]
            .into_iter()
            .map(|(name, emoji)| {
                serenity::CreateButton::new(id(name))
                    .emoji(serenity::ReactionType::Unicode(emoji.into()))
                    .style(if name == "stop" {
                        serenity::ButtonStyle::Danger
                    } else {
                        serenity::ButtonStyle::Secondary
                    })
                    .disabled(disabled)
            })
            .collect(),
        )]
    };

    let reply = |page: usize, disabled: bool| {
        let mut reply = CreateReply::default()
            .embed(pages[page].clone())
            .ephemeral(ephemeral);
        if pages.len() > 1 {
            reply = reply.components(buttons(disabled));
        }
        reply
    };

    let handle = ctx.send(reply(0, false)).await?;
    if pages.len() == 1 {
        return Ok(());
    }

    let last = pages.len() - 1;
    let mut page = 0;
    while let Some(press) = serenity::ComponentInteractionCollector::new(ctx)
        .filter({
            let prefix = prefix.clone();
            move |press| press.data.custom_id.starts_with(&prefix)
        })
        .timeout(IDLE_LIMIT)
        .await
    {
        if press.user.id != ctx.author().id {
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

        let action = &press.data.custom_id[prefix.len()..];
        if action == "stop" {
            let _ = press
                .create_response(
                    ctx,
                    serenity::CreateInteractionResponse::UpdateMessage(
                        serenity::CreateInteractionResponseMessage::new()
                            .embed(pages[page].clone())
                            .components(buttons(true)),
                    ),
                )
                .await;
            return Ok(());
        }

        page = match action {
            "first" => 0,
            "prev" => page.checked_sub(1).unwrap_or(last),
            "next" => (page + 1) % pages.len(),
            "last" => last,
            _ => page,
        };

        press
            .create_response(
                ctx,
                serenity::CreateInteractionResponse::UpdateMessage(
                    serenity::CreateInteractionResponseMessage::new()
                        .embed(pages[page].clone())
                        .components(buttons(false)),
                ),
            )
            .await?;
    }

    // Expirou: o conteudo fica legivel, so os botoes desligam. Se o token da
    // interacao ja venceu, nao ha o que fazer.
    let _ = handle.edit(ctx, reply(page, true)).await;
    Ok(())
}
