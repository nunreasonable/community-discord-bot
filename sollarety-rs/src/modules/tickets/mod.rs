//! Tickets e utilidades: /ticket, /ticket-panel, /ticket-close, /ticket-add,
//! /ticket-remove, os botoes e modais de ticket, o /logs (BotLogs), /ping,
//! /userinfo, /serverinfo, /poll, /help e o feed de commits (GitFeed).
//!
//! Portado do C# (CommunityBot/): commands/Tickets.cs, commands/BotLogs.cs,
//! commands/Utility.cs, Services/TicketService.cs, TicketComponents.cs,
//! TicketTypes.cs e GitFeed.cs.

mod commands;
mod components;
mod git_feed;
mod logs;
mod service;
mod types;
mod utility;

use poise::serenity_prelude as serenity;

use crate::{Data, Error, scope};

/// Os comandos do modulo, na ordem do RegisterAll do C# (Utility, BotLogs,
/// Tickets). So /ping, /userinfo e /help valem como app pessoal; o resto e
/// so de servidor.
pub fn commands() -> Vec<poise::Command<Data, Error>> {
    vec![
        scope::anywhere(utility::ping()),
        scope::anywhere(utility::userinfo()),
        scope::guild_only(utility::serverinfo()),
        scope::guild_only(utility::poll()),
        scope::anywhere(utility::help()),
        scope::guild_only(logs::logs()),
        scope::guild_only(commands::ticket()),
        scope::guild_only(commands::ticket_panel()),
        scope::guild_only(commands::ticket_close()),
        scope::guild_only(commands::ticket_add()),
        scope::guild_only(commands::ticket_remove()),
    ]
}

/// Botoes e modais "ticket:", e a subida: o relatorio da configuracao dos
/// tickets (com a limpeza de registros orfaos) e o feed de commits, que liga
/// na primeira carga dos servidores porque o canal precisa estar no cache.
pub async fn on_event(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    data: &Data,
) -> Result<(), Error> {
    match event {
        serenity::FullEvent::InteractionCreate { interaction } => {
            components::on_interaction(ctx, interaction, data);
        }
        serenity::FullEvent::CacheReady { .. } => {
            let (ctx_owned, data_owned) = (ctx.clone(), data.clone());
            tokio::spawn(async move {
                service::report_status(&ctx_owned, &data_owned).await;
            });
            git_feed::start(ctx);
        }
        _ => {}
    }
    Ok(())
}

/// Desligamento: para a varredura do feed de commits (GitFeed.Stop).
pub async fn shutdown(_data: &Data) {
    git_feed::stop();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contextos_como_no_csharp() {
        let commands = commands();
        let names: Vec<_> = commands.iter().map(|c| &*c.name).collect();
        assert_eq!(
            names,
            [
                "ping",
                "userinfo",
                "serverinfo",
                "poll",
                "help",
                "logs",
                "ticket",
                "ticket-panel",
                "ticket-close",
                "ticket-add",
                "ticket-remove"
            ]
        );
        assert_eq!(
            commands[5].description.as_deref(),
            Some("Show the bot's latest logs")
        );
        for command in &commands {
            let user_install = command
                .install_context
                .as_ref()
                .is_some_and(|c| c.contains(&serenity::InstallationContext::User));
            let anywhere = matches!(&*command.name, "ping" | "userinfo" | "help");
            assert_eq!(user_install, anywhere, "/{}", command.name);
            // A descricao e a primeira linha do doc comment; texto longo ali
            // estouraria o limite de 100 do Discord e derrubaria o registro.
            let description = command.description.as_deref().unwrap_or_default();
            assert!(
                !description.is_empty() && description.len() <= 100,
                "/{}: {description:?}",
                command.name
            );
        }
    }
}
