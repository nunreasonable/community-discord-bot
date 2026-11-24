//! Moderacao: /ban, /kick, /softban, /timeout, /untimeout, /purge, /slowmode,
//! /lock, /unlock, advertencias (/warn, /warnings, /delwarn), o /config, o
//! vigia de canal (auto-softban), a migracao unica das chaves legadas do
//! config.jsonc e a faxina de comandos de servidor (GuildCommandSweeper).
//!
//! A portar do C# (CommunityBot/):
//! - commands/Moderation.cs
//! - commands/Warnings.cs
//! - commands/Config.cs
//! - Services/AutoSoftban.cs
//! - Services/Softban.cs
//! - Services/LegacyConfigMigration.cs
//! - Services/GuildCommandSweeper.cs
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
    // Desligamento: esperar o softban em voo (AutoSoftban.DrainAsync, ate 10 s) -
    // um ban aplicado com o unban pendente vira banimento permanente se o
    // processo morrer no meio.
}
