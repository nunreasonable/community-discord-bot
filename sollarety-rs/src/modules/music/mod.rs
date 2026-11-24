//! Musica: /play, /skip, /stop, /pause, /resume, /volume, /loop, /shuffle,
//! /remove, /queue, /nowplaying, os botoes "music:" e o VoiceStateUpdate
//! (bot expulso, canal vazio). A voz e o songbird que o main ja registrou no
//! client (`data.songbird`); o audio e so de SAIDA - o songbird esta sem o
//! feature "receive", entao nada que os outros falam e decodificado.
//!
//! Mapa (do C#, CommunityBot/):
//! - commands.rs   commands/Music.cs
//! - actions.rs    Services/Music/MusicActions.cs
//! - components.rs Services/Music/MusicComponents.cs
//! - format.rs     Services/Music/MusicFormat.cs
//! - player.rs     Services/Music/GuildPlayer.cs
//! - service.rs    Services/Music/MusicService.cs
//! - sources.rs    Services/Music/MusicSources.cs
//! - spotify.rs    Services/Music/Spotify.cs
//! - track.rs      Services/Music/TrackInfo.cs (+ MusicException)
//! - ytdlp.rs      Services/Music/YtDlp.cs
//! - voice.rs      ffmpeg -> songbird (no C#, o VoiceTransmitSink)
//! - cancel.rs     o CancellationToken que o tokio puro nao tem

mod actions;
mod cancel;
mod commands;
mod components;
mod format;
mod player;
mod service;
mod sources;
mod spotify;
mod track;
mod voice;
mod ytdlp;

use std::{sync::Once, time::Duration};

use poise::serenity_prelude as serenity;

use crate::{Data, Error, scope};

/// Os comandos do modulo, na ordem do C#. Todos so em servidor.
pub fn commands() -> Vec<poise::Command<Data, Error>> {
    vec![
        commands::play(),
        commands::skip(),
        commands::stop(),
        commands::pause(),
        commands::resume(),
        commands::volume(),
        commands::loop_(),
        commands::shuffle(),
        commands::remove(),
        commands::queue(),
        commands::nowplaying(),
    ]
    .into_iter()
    .map(scope::guild_only)
    .collect()
}

pub async fn on_event(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    _data: &Data,
) -> Result<(), Error> {
    // O CleanTempRoot do C# rodava antes de conectar. O primeiro evento do
    // gateway vem antes de qualquer /play, e o Once segura quem chegar junto.
    static CLEAN: Once = Once::new();
    CLEAN.call_once(service::clean_temp_root);

    match event {
        serenity::FullEvent::CacheReady { .. } => {
            tokio::spawn(service::report_status());
        }
        serenity::FullEvent::VoiceStateUpdate { new, .. } => {
            service::on_voice_state_update(ctx, new);
        }
        serenity::FullEvent::InteractionCreate {
            interaction: serenity::Interaction::Component(component),
        } if component.data.custom_id.starts_with(format::PREFIX) => {
            tokio::spawn(components::handle(ctx.clone(), component.clone()));
        }
        _ => {}
    }
    Ok(())
}

/// Desligamento: sai dos canais de voz e apaga o audio baixado (ate 8 s).
pub async fn shutdown(_data: &Data) {
    service::drain(Duration::from_secs(8)).await;
}
