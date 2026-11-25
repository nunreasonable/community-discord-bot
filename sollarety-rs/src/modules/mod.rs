//! Os modulos de funcionalidade. Cada um e um diretorio com `commands()`,
//! `on_event()` e `shutdown()`; o main so os junta. Um lugar so para a lista:
//! esquecer de registrar um modulo e o jeito mais facil de um comando novo
//! simplesmente nao aparecer (RegisterAll do C#).

pub mod admin;
pub mod ai;
pub mod community;
pub mod moderation;
pub mod music;
pub mod roblox;
pub mod tickets;

use poise::serenity_prelude as serenity;

use crate::{Data, Error, config::Config};

/// Todos os comandos, na ordem do RegisterAll do C#. Os do admin entram aqui
/// tambem - o poise so despacha o que esta nesta lista -, mas o registro no
/// Discord os separa (ver `guild_scoped`).
pub fn commands() -> Vec<poise::Command<Data, Error>> {
    let mut all = Vec::new();
    all.extend(moderation::commands());
    all.extend(community::commands());
    all.extend(tickets::commands());
    all.extend(roblox::commands());
    all.extend(music::commands());
    all.extend(ai::commands());
    all.extend(admin::commands());
    all
}

/// Comandos que so existem em servidores escolhidos: nunca entram no registro
/// global nem no de guildIds.
pub struct GuildScoped {
    /// Nomes de primeiro nivel, como em `commands()`.
    pub names: Vec<String>,
    pub guild_ids: Vec<u64>,
    /// A chave do config, para o log dizer onde mexer.
    pub config_key: &'static str,
    /// Nestes servidores os comandos vao sem default_member_permissions
    /// (`open_menu`): quem pode e conferido na execucao, por cargo.
    pub open_menu: bool,
}

/// Os grupos travados, com a lista de servidores do config da subida: o
/// admin inteiro em admin.guildIds e so o /dm em admin.dmGuilds.
pub fn guild_scoped(config: &Config) -> Vec<GuildScoped> {
    vec![
        GuildScoped {
            names: admin::commands()
                .into_iter()
                .map(|c| c.name.into_owned())
                .collect(),
            guild_ids: config.admin.guild_ids.clone(),
            config_key: "admin.guildIds",
            open_menu: false,
        },
        GuildScoped {
            names: admin::DM_ONLY.iter().map(|&n| n.to_owned()).collect(),
            guild_ids: config.admin.dm_guilds.keys().copied().collect(),
            config_key: "admin.dmGuilds",
            open_menu: true,
        },
    ]
}

/// O comando sem default_member_permissions, para um grupo `open_menu`.
pub fn open_menu(name: &str) -> Option<poise::Command<Data, Error>> {
    admin::open_menu(name)
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
        admin::on_event(ctx, event, data),
    );

    let named = [
        ("moderation", results.0),
        ("roblox", results.1),
        ("music", results.2),
        ("community", results.3),
        ("tickets", results.4),
        ("ai", results.5),
        ("admin", results.6),
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

/// Na ordem do C#: o softban em voo, depois a musica, depois o feed. O admin
/// vem logo depois do softban: a DM em massa para e edita a mensagem de
/// progresso enquanto o gateway e o HTTP ainda estao de pe.
pub async fn shutdown(data: &Data) {
    moderation::shutdown(data).await;
    admin::shutdown(data).await;
    music::shutdown(data).await;
    tickets::shutdown(data).await;
    roblox::shutdown(data).await;
    community::shutdown(data).await;
    ai::shutdown(data).await;
}
