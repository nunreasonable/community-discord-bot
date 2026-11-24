//! Musica: /play, /skip, /stop, /pause, /resume, /volume, /loop, /shuffle,
//! /remove, /queue, /nowplaying, os botoes "music:" e o VoiceStateUpdate
//! (bot expulso, canal vazio). A voz e o songbird que o main ja registrou no
//! client: `data.songbird` ou `songbird::get(ctx)`.
//!
//! A portar do C# (CommunityBot/):
//! - commands/Music.cs
//! - Services/Music/GuildPlayer.cs
//! - Services/Music/MusicActions.cs
//! - Services/Music/MusicComponents.cs
//! - Services/Music/MusicFormat.cs
//! - Services/Music/MusicService.cs
//! - Services/Music/MusicSources.cs
//! - Services/Music/Spotify.cs
//! - Services/Music/TrackInfo.cs
//! - Services/Music/YtDlp.cs
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
    // Desligamento: sair dos canais de voz e apagar o audio baixado
    // (MusicService.DrainAsync, ate 8 s). Na subida o C# chamava
    // MusicService.CleanTempRoot() antes de conectar: faca isso no primeiro
    // evento ou num LazyLock do modulo.
}
