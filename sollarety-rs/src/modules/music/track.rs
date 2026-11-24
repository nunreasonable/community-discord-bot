//! O que se sabe de uma faixa, sem o audio (TrackInfo.cs), e a falha com
//! frase pronta para o usuario (MusicException do YtDlp.cs).

use std::{
    fmt,
    sync::{Arc, Mutex},
    time::Duration,
};

/// De onde a faixa vem, e portanto como ela toca.
///
/// YouTube e o caso especial: o audio e BAIXADO inteiro antes de tocar e
/// apagado depois, e nunca transmitido direto - e o que separa este bot dos
/// que o Discord e o Google derrubaram por tocar YouTube em streaming.
/// SoundCloud toca por streaming. Spotify nao entrega audio (DRM): o link so
/// da o nome, e a faixa equivalente e procurada no SoundCloud e, se nao
/// bater, no YouTube.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackSource {
    YouTube,
    SoundCloud,
    Spotify,
}

/// O que fica na fila: o arquivo de um video do YouTube so existe entre o
/// download e o fim dele.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackInfo {
    pub id: String,
    pub title: String,
    pub webpage_url: String,
    pub duration: Duration,
    pub uploader: Option<String>,
    pub thumbnail_url: Option<String>,
    pub source: TrackSource,
}

impl TrackInfo {
    /// Entrada de set do SoundCloud antes de ser consultada: a listagem de um
    /// set so traz a URL de cada faixa, e o resto chega quando for a vez dela.
    pub fn is_placeholder(&self) -> bool {
        self.duration.is_zero() && self.source != TrackSource::Spotify
    }
}

/// Uma entrada da fila. Vive num `Arc` e e comparada por ponteiro, de
/// proposito: a mesma pessoa pode enfileirar a mesma faixa duas vezes, e com
/// igualdade por valor o /remove e o prefetch nao teriam como distinguir uma
/// entrada da outra.
#[derive(Debug)]
pub struct QueuedTrack {
    /// O que a pessoa pediu - e o que aparece na fila e no aviso.
    pub track: TrackInfo,
    pub requester_id: u64,
    /// A faixa do YouTube ou do SoundCloud que vai de fato tocar. Vazia ate
    /// ser resolvida (Spotify, entrada de set). So o laco do player escreve.
    playable: Mutex<Option<TrackInfo>>,
}

impl QueuedTrack {
    pub fn new(track: TrackInfo, requester_id: u64) -> Arc<Self> {
        // Faixa ja consultada do YouTube ou do SoundCloud toca como esta.
        let playable = (track.source != TrackSource::Spotify && !track.is_placeholder())
            .then(|| track.clone());
        Arc::new(Self {
            track,
            requester_id,
            playable: Mutex::new(playable),
        })
    }

    pub fn playable(&self) -> Option<TrackInfo> {
        self.playable
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn set_playable(&self, track: TrackInfo) {
        *self.playable.lock().unwrap_or_else(|e| e.into_inner()) = Some(track);
    }

    /// O que mostrar: a faixa resolvida quando ha, senao a pedida. O Spotify
    /// mostra sempre o que foi pedido - a gravacao casada aparece no Source.
    pub fn display(&self) -> TrackInfo {
        if self.track.source == TrackSource::Spotify {
            return self.track.clone();
        }
        self.playable().unwrap_or_else(|| self.track.clone())
    }

    /// A duracao que vale e a do que TOCA: a gravacao casada com um link do
    /// Spotify nem sempre tem o mesmo tamanho.
    pub fn playing_duration(&self) -> Duration {
        self.playable()
            .map_or_else(|| self.display().duration, |p| p.duration)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoopMode {
    #[default]
    Off,
    Track,
    Queue,
}

/// Falha de musica. `User` tem uma frase pronta para quem usou o comando: vai
/// direto para o Discord, entao e sempre em ingles e nunca carrega caminho de
/// arquivo nem saida crua de processo. `Other` e o resto (processo que nao
/// abriu, prazo estourado, JSON torto), que so vai para o log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MusicError {
    User(String),
    Other(String),
}

impl MusicError {
    pub fn user(message: impl Into<String>) -> Self {
        Self::User(message.into())
    }
}

impl fmt::Display for MusicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User(message) | Self::Other(message) => f.write_str(message),
        }
    }
}

impl From<crate::Error> for MusicError {
    fn from(cause: crate::Error) -> Self {
        Self::Other(cause.to_string())
    }
}

#[cfg(test)]
pub(crate) fn sample(source: TrackSource, seconds: u64) -> TrackInfo {
    TrackInfo {
        id: "id".into(),
        title: "Song".into(),
        webpage_url: "https://example.com/x".into(),
        duration: Duration::from_secs(seconds),
        uploader: None,
        thumbnail_url: None,
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spotify_e_set_esperam_resolver() {
        assert!(
            QueuedTrack::new(sample(TrackSource::Spotify, 200), 1)
                .playable()
                .is_none()
        );
        assert!(
            QueuedTrack::new(sample(TrackSource::SoundCloud, 0), 1)
                .playable()
                .is_none()
        );
        assert!(
            QueuedTrack::new(sample(TrackSource::YouTube, 10), 1)
                .playable()
                .is_some()
        );
    }

    #[test]
    fn display_do_spotify_e_o_pedido() {
        let entry = QueuedTrack::new(sample(TrackSource::Spotify, 200), 1);
        let mut found = sample(TrackSource::SoundCloud, 205);
        found.title = "Outra".into();
        entry.set_playable(found);
        assert_eq!(entry.display().title, "Song");
        assert_eq!(entry.playing_duration(), Duration::from_secs(205));
    }
}
