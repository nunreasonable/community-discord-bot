//! Os modulos de funcionalidade. Cada um e um diretorio com `commands()`,
//! `on_event()` e `shutdown()`; o main so os junta. Um lugar so para a lista:
//! esquecer de registrar um modulo e o jeito mais facil de um comando novo
//! simplesmente nao aparecer (RegisterAll do C#).

pub mod ai;
pub mod community;
pub mod moderation;
pub mod music;
pub mod roblox;
pub mod tickets;

use poise::serenity_prelude as serenity;

use crate::{Data, Error};

/// Todos os comandos, na ordem do RegisterAll do C#.
pub fn commands() -> Vec<poise::Command<Data, Error>> {
    let mut all = Vec::new();
    all.extend(moderation::commands());
    all.extend(community::commands());
    all.extend(tickets::commands());
    all.extend(roblox::commands());
    all.extend(music::commands());
    all.extend(ai::commands());
    all
}

/// Entrega o evento a todos os modulos, ao mesmo tempo. A falha de um vira
/// log e nao impede os outros de verem o evento.
pub async fn dispatch(ctx: &serenity::Context, event: &serenity::FullEvent, data: &Data) {
    let results = tokio::join!(
        moderation::on_event(ctx, event, data),
        roblox::on_event(ctx, event, data),
        music::on_event(ctx, event, data),
        community::on_event(ctx, event, data),
        tickets::on_event(ctx, event, data),
        ai::on_event(ctx, event, data),
    );

    let named = [
        ("moderation", results.0),
        ("roblox", results.1),
        ("music", results.2),
        ("community", results.3),
        ("tickets", results.4),
        ("ai", results.5),
    ];
    for (module, result) in named {
        if let Err(cause) = result {
            tracing::error!(
                "[{module}] falha ao tratar {}: {cause}",
                event.snake_case_name()
            );
        }
    }
}

/// Na ordem do C#: o softban em voo, depois a musica, depois o feed.
pub async fn shutdown(data: &Data) {
    moderation::shutdown(data).await;
    music::shutdown(data).await;
    tickets::shutdown(data).await;
    roblox::shutdown(data).await;
    community::shutdown(data).await;
    ai::shutdown(data).await;
}
