//! Tickets e utilidades: /ticket, /ticket-panel, /ticket-close, /ticket-add,
//! /ticket-remove, os botoes e modais de ticket, o /logs (BotLogs), /ping,
//! /userinfo, /serverinfo, /poll, /help e o feed de commits (GitFeed).
//!
//! A portar do C# (CommunityBot/):
//! - commands/Tickets.cs
//! - commands/BotLogs.cs
//! - commands/Utility.cs
//! - Services/TicketService.cs
//! - Services/TicketComponents.cs
//! - Services/TicketTypes.cs
//! - Services/GitFeed.cs
//!
//! Tudo deste modulo fica DENTRO deste diretorio. Estado proprio vai num
//! `static` (LazyLock/OnceLock) daqui, e nao em campo novo no `Data`.

use poise::serenity_prelude as serenity;

use crate::{Data, Error};

/// Os comandos do modulo. O main junta todos e registra.
pub fn commands() -> Vec<poise::Command<Data, Error>> {
    vec![]
}

/// Recebe TODO evento do gateway. Interacao de componente que nao comeca com
/// o prefixo deste modulo nao e daqui: ignore e devolva Ok. Trabalho longo
/// vai num `tokio::spawn`, para nao segurar os outros modulos.
pub async fn on_event(
    _ctx: &serenity::Context,
    _event: &serenity::FullEvent,
    _data: &Data,
) -> Result<(), Error> {
    Ok(())
}

/// Chamado uma vez no desligamento, antes de largar o gateway.
pub async fn shutdown(_data: &Data) {
    // Desligamento: parar a varredura do feed de commits (GitFeed.Stop).
}
