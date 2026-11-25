//! Administracao: /send, /role add|remove|strip e /dm role|user. As funcoes
//! genericas do painel web do ccore (send_message, change_role,
//! remove_from_regiment, dm_start/dm_status/dm_cancel), como slash comandos
//! e sem site.
//!
//! Portado de (ccore-rs/):
//! - src/dashboard/routes.rs (send_message, split_for_discord) -> send.rs
//! - src/dashboard/routes.rs (change_role, role_refusal, bot_top_position,
//!   remove_from_regiment, punishment_dm) -> roles.rs, gate.rs
//! - src/dashboard/jobs.rs + src/dm.rs (DmLimiter, resolve, send_tracked,
//!   describe_failures) -> dm.rs, report.rs
//!
//! TRAVADO em servidor, como o painel era travado no servidor do regimento:
//! os comandos so sao registrados nos servidores de `admin.guildIds` (ver
//! `modules::guild_scoped` e o registro no lib.rs) e cada comando e botao
//! confere a lista de novo na execucao (`gate::ensure`). No ccore o portao
//! era a patente; aqui e a permissao do proprio Discord mais a hierarquia.
//!
//! Estado proprio (envios em andamento, o limitador de DM) mora em `static`s
//! do dm.rs.

mod dm;
mod gate;
mod modal;
mod report;
mod roles;
mod send;

use poise::serenity_prelude as serenity;

use crate::{Data, Error, scope};

/// Todos so dentro de servidor. O registro so os leva para os servidores de
/// admin.guildIds; fora deles nem aparecem.
pub fn commands() -> Vec<poise::Command<Data, Error>> {
    vec![send::send(), roles::role_group(), dm::dm_group()]
        .into_iter()
        .map(scope::guild_only)
        .collect()
}

/// So o botao de cancelar da DM em massa chega por aqui: os outros botoes e
/// modais do modulo sao lidos pelos coletores dentro dos proprios comandos.
pub async fn on_event(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    data: &Data,
) -> Result<(), Error> {
    if let serenity::FullEvent::InteractionCreate { interaction } = event {
        dm::on_interaction(ctx, interaction, data);
    }
    Ok(())
}

/// Para as DMs em massa em andamento; a mensagem de progresso de cada uma
/// vira o relatorio dizendo que o bot desligou.
pub async fn shutdown(_data: &Data) {
    dm::shutdown().await;
}
