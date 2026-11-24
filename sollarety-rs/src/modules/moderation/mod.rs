//! Moderacao: /ban, /kick, /softban, /timeout, /untimeout, /purge, /slowmode,
//! /lock, /unlock, advertencias (/warn, /warnings, /delwarn), o /config, o
//! vigia de canal (auto-softban), a migracao unica das chaves legadas do
//! config.jsonc e a faxina de comandos de servidor (GuildCommandSweeper).
//!
//! Portado de (CommunityBot/):
//! - commands/Moderation.cs -> commands.rs
//! - commands/Warnings.cs -> warnings.rs
//! - commands/Config.cs -> config_cmd.rs
//! - Services/AutoSoftban.cs -> auto_softban.rs
//! - Services/Softban.cs -> softban.rs
//! - Services/LegacyConfigMigration.cs -> legacy_migration.rs
//! - Services/GuildCommandSweeper.cs -> sweeper.rs
//!
//! Estado proprio (marcas do vigia, disjuntores, servidores varridos) mora
//! em `static`s desses arquivos, e nao em campo novo no `Data`.

mod auto_softban;
mod commands;
mod config_cmd;
mod legacy_migration;
mod softban;
mod sweeper;
mod util;
mod warnings;

use std::time::Duration;

use poise::serenity_prelude as serenity;

use crate::{Data, Error, scope};

/// Quanto o desligamento espera pelo softban em voo.
const DRAIN_BUDGET: Duration = Duration::from_secs(10);

/// Os comandos do modulo, na ordem do RegisterAll do C# (Moderation,
/// Warnings, ... Config). Todos so dentro de servidor: somem da DM com o
/// bot, onde apareceriam so para recusar.
pub fn commands() -> Vec<poise::Command<Data, Error>> {
    vec![
        commands::ban(),
        commands::kick(),
        commands::softban(),
        commands::timeout(),
        commands::untimeout(),
        commands::purge(),
        commands::slowmode(),
        commands::lock(),
        commands::unlock(),
        warnings::warn(),
        warnings::warnings(),
        warnings::delwarn(),
        config_cmd::config_group(),
    ]
    .into_iter()
    .map(scope::guild_only)
    .collect()
}

/// Recebe TODO evento do gateway. Nao ha componente nem modal deste modulo;
/// o que importa sao mensagens (vigia), o cache pronto (relatorio do vigia,
/// migracao, varredura) e a entrada em servidor novo.
pub async fn on_event(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    data: &Data,
) -> Result<(), Error> {
    match event {
        serenity::FullEvent::Message { new_message } => {
            auto_softban::on_message(ctx, new_message, data);
        }
        serenity::FullEvent::CacheReady { guilds } => {
            // Diz na subida se o vigia esta ligado e, se nao estiver
            // funcionando, por que. Sincrono: so le o cache.
            auto_softban::report_status(ctx, data);

            // Migracao e varredura vao para segundo plano: tocam disco e
            // REST, e nao podem segurar o despacho dos outros modulos.
            let (migration_ctx, migration_data) = (ctx.clone(), data.clone());
            tokio::spawn(async move {
                legacy_migration::run(&migration_ctx, &migration_data).await;
            });
            sweeper::on_cache_ready(ctx, data, guilds.clone());
        }
        // `is_new` so e Some(true) para servidor em que o bot ENTROU com o
        // processo no ar; os que chegam no READY sao da varredura de subida.
        serenity::FullEvent::GuildCreate {
            guild,
            is_new: Some(true),
        } => sweeper::on_guild_joined(ctx, data, guild),
        _ => {}
    }
    Ok(())
}

/// Desligamento: esperar o softban em voo - um ban aplicado com o unban
/// pendente vira banimento permanente se o processo morrer no meio, e sem
/// nenhuma linha de log dizendo isso.
pub async fn shutdown(_data: &Data) {
    auto_softban::drain(DRAIN_BUDGET).await;
}
