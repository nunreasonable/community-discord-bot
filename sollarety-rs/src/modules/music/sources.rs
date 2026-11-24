//! Transforma o que alguem digitou no /play em faixas, e as faixas ainda sem
//! audio (Spotify, entrada de set) em algo que toca (MusicSources.cs).
//!
//! E o unico lugar que decide que URL segue adiante. So passam hosts do
//! YouTube, do SoundCloud e do Spotify; o resto e recusado antes de chegar ao
//! yt-dlp, cujo extrator generico baixaria qualquer endereco - inclusive
//! interno desta rede.

use std::time::Duration;

use tracing::{info, warn};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};
use url::Url;

use super::{
    format, spotify,
    track::{MusicError, QueuedTrack, TrackInfo, TrackSource},
    ytdlp,
};
use crate::config::MusicSettings;

/// O que um /play vai enfileirar.
#[derive(Debug)]
pub struct Request {
    pub tracks: Vec<TrackInfo>,
    /// Nome do set/album/playlist; None para faixa avulsa.
    pub collection_name: Option<String>,
    pub source: TrackSource,
    /// Quantas ficaram de fora pelo limite por link.
    pub truncated: usize,
}

impl Request {
    fn single(track: TrackInfo, source: TrackSource) -> Self {
        Self {
            tracks: vec![track],
            collection_name: None,
            source,
            truncated: 0,
        }
    }
}

const YOUTUBE_HOSTS: &[&str] = &[
    "youtube.com",
    "www.youtube.com",
    "m.youtube.com",
    "music.youtube.com",
    "youtu.be",
];
const SOUNDCLOUD_HOSTS: &[&str] = &["soundcloud.com", "www.soundcloud.com", "m.soundcloud.com"];
const SPOTIFY_HOSTS: &[&str] = &["open.spotify.com", "play.spotify.com"];

// Links curtos: so redirecionam. O destino final passa pelas mesmas listas.
const SHORT_HOSTS: &[&str] = &["on.soundcloud.com", "spotify.link", "spotify.app.link"];

// Segundo trecho de uma URL do SoundCloud que NAO e faixa: perfil, set,
// curtidas etc. soundcloud.com/artista/faixa e o formato de uma faixa.
const SOUNDCLOUD_NON_TRACK: &[&str] = &[
    "sets",
    "likes",
    "tracks",
    "albums",
    "reposts",
    "popular-tracks",
    "followers",
    "following",
    "comments",
    "spotlight",
    "toptracks",
];

// A API do SoundCloud: sets grandes listam as faixas a partir da sexta como
// api-v2.soundcloud.com/tracks/ID, e o yt-dlp resolve esse formato.
const SOUNDCLOUD_API_HOSTS: &[&str] = &["api.soundcloud.com", "api-v2.soundcloud.com"];

// Versao que nao e a musica: se aparece no SoundCloud e nao no titulo do
// Spotify, o candidato e descartado mesmo com a duracao batendo - um backing
// track tem, por definicao, a duracao do original.
const VARIANT_WORDS: &[&str] = &[
    "backing track",
    "karaoke",
    "instrumental",
    "cover",
    "remix",
    "sped up",
    "slowed",
    "nightcore",
    "reverb",
    "8d",
    "acapella",
    "a cappella",
    "live",
    "acoustic",
    "tribute",
    "bass boosted",
    "mashup",
    "bootleg",
    "type beat",
];

const UNSUPPORTED: &str = "I can play **YouTube**, **SoundCloud** and **Spotify** links. You can also just type what to search for.";

fn host_in(url: &Url, hosts: &[&str]) -> bool {
    url.host_str()
        .is_some_and(|h| hosts.iter().any(|x| x.eq_ignore_ascii_case(h)))
}

// ----------------------------------------------------------------------
// Pedido
// ----------------------------------------------------------------------

/// O texto do /play, ja classificado. Puro: e o que os testes olham.
#[derive(Debug, PartialEq)]
enum Input {
    Empty,
    SpotifyUri(String),
    Link(Url),
    BadLink,
    Search(String),
}

fn parse_input(input: &str) -> Input {
    let text = input.trim();
    if text.is_empty() {
        return Input::Empty;
    }
    if text
        .get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case("spotify:"))
    {
        return Input::SpotifyUri(text.to_owned());
    }

    // Sem esquema tambem conta como link: quem cola "youtu.be/abc" nao quer
    // procurar pelo texto "youtu.be/abc".
    let candidate =
        if !text.contains("://") && !text.contains(' ') && text.contains('.') && text.contains('/')
        {
            format!("https://{text}")
        } else {
            text.to_owned()
        };

    if let Ok(url) = Url::parse(&candidate)
        && matches!(url.scheme(), "http" | "https")
    {
        return Input::Link(url);
    }
    if text.contains("://") {
        return Input::BadLink;
    }
    Input::Search(text.to_owned())
}

/// Para onde um link vai, pelo host.
#[derive(Debug, PartialEq)]
enum LinkKind {
    Short,
    YouTube,
    YouTubePlaylist,
    SoundCloud,
    Spotify,
    Unsupported,
}

fn classify(url: &Url) -> LinkKind {
    if host_in(url, SHORT_HOSTS) {
        LinkKind::Short
    } else if host_in(url, YOUTUBE_HOSTS) {
        if url
            .path()
            .get(..9)
            .is_some_and(|p| p.eq_ignore_ascii_case("/playlist"))
        {
            LinkKind::YouTubePlaylist
        } else {
            LinkKind::YouTube
        }
    } else if host_in(url, SOUNDCLOUD_HOSTS) {
        LinkKind::SoundCloud
    } else if host_in(url, SPOTIFY_HOSTS) {
        LinkKind::Spotify
    } else {
        LinkKind::Unsupported
    }
}

/// As faixas de um /play. Link de faixa do YouTube ou do SoundCloud ja sai
/// consultado (e recusado aqui mesmo se nao puder tocar); playlist, album e
/// set saem como lista, e cada faixa so e buscada na vez dela.
pub async fn resolve(
    http: &reqwest::Client,
    input: &str,
    search_source: TrackSource,
    settings: &MusicSettings,
) -> Result<Request, MusicError> {
    let mut url = match parse_input(input) {
        Input::Empty => {
            return Err(MusicError::user(
                "Tell me what to play: a link or what to search for.",
            ));
        }
        Input::SpotifyUri(uri) => return from_spotify(http, &uri, settings).await,
        Input::BadLink => return Err(MusicError::user("That doesn't look like a valid link.")),
        Input::Search(text) => return search(&text, search_source, settings).await,
        Input::Link(url) => url,
    };

    if classify(&url) == LinkKind::Short {
        url = expand_short_link(http, &url).await?;
    }

    match classify(&url) {
        LinkKind::YouTubePlaylist => Err(MusicError::user(
            "YouTube playlists aren't supported. Send a link to a single video.",
        )),
        LinkKind::YouTube => {
            let video = ytdlp::resolve(url.as_str(), TrackSource::YouTube, settings).await?;
            Ok(Request::single(video, TrackSource::YouTube))
        }
        LinkKind::SoundCloud => from_soundcloud(&url, settings).await,
        LinkKind::Spotify => from_spotify(http, url.as_str(), settings).await,
        LinkKind::Short | LinkKind::Unsupported => Err(MusicError::user(UNSUPPORTED)),
    }
}

async fn search(
    text: &str,
    source: TrackSource,
    settings: &MusicSettings,
) -> Result<Request, MusicError> {
    if source == TrackSource::SoundCloud {
        // Tres bastam: cada resultado e uma consulta inteira ao SoundCloud.
        let found = ytdlp::search_soundcloud(text, 3, settings).await?;
        let first = found.into_iter().next().ok_or_else(|| {
            MusicError::user("I couldn't find anything playable on SoundCloud for that.")
        })?;
        return Ok(Request::single(first, TrackSource::SoundCloud));
    }

    let target = format!("ytsearch1:{text}");
    let result = ytdlp::resolve(&target, TrackSource::YouTube, settings).await?;
    Ok(Request::single(result, TrackSource::YouTube))
}

/// scheme://host/caminho, sem query nem fragmento (GetLeftPart(Path)).
fn left_part(url: &Url) -> Url {
    let mut out = url.clone();
    out.set_query(None);
    out.set_fragment(None);
    out
}

async fn from_soundcloud(url: &Url, settings: &MusicSettings) -> Result<Request, MusicError> {
    let url = left_part(url);

    if is_soundcloud_track_url(url.as_str()) {
        let track = ytdlp::resolve(url.as_str(), TrackSource::SoundCloud, settings).await?;
        return Ok(Request::single(track, TrackSource::SoundCloud));
    }

    // Set, perfil, curtidas: a listagem so traz URLs. Pede uma a mais que o
    // limite para saber se houve corte.
    let max = settings.max_playlist();
    let (title, urls) = ytdlp::list_soundcloud(url.as_str(), max + 1, settings).await?;
    if urls.is_empty() {
        return Err(MusicError::user(
            "I couldn't find any tracks in that SoundCloud link.",
        ));
    }

    Ok(Request {
        truncated: urls.len().saturating_sub(max),
        tracks: urls.iter().take(max).map(|u| placeholder(u)).collect(),
        collection_name: Some(title.unwrap_or_else(|| "SoundCloud set".into())),
        source: TrackSource::SoundCloud,
    })
}

async fn from_spotify(
    http: &reqwest::Client,
    link: &str,
    settings: &MusicSettings,
) -> Result<Request, MusicError> {
    let Some((kind, id)) = spotify::try_parse(link) else {
        return Err(MusicError::user(
            "Send a Spotify **track**, **album** or **playlist** link. Artists and podcasts aren't supported.",
        ));
    };

    let collection = spotify::fetch(http, kind, &id).await?;
    if collection.tracks.is_empty() {
        return Err(MusicError::user(
            "That Spotify link has no playable tracks.",
        ));
    }

    let max = settings.max_playlist();
    let truncated = collection.tracks.len().saturating_sub(max);
    let tracks: Vec<TrackInfo> = collection
        .tracks
        .into_iter()
        .take(max)
        .map(|t| TrackInfo {
            webpage_url: spotify::track_url(&t.id),
            id: t.id,
            title: t.title,
            duration: t.duration,
            uploader: (!t.artists.trim().is_empty()).then_some(t.artists),
            thumbnail_url: t.image_url,
            source: TrackSource::Spotify,
        })
        .collect();

    // Faixa avulsa longa demais nao cabe em fonte nenhuma: recusa ja.
    let limit = settings.max_stream_length();
    if kind == spotify::Kind::Track && tracks[0].duration > limit {
        return Err(MusicError::User(format!(
            "That track is {} long; the limit is {}.",
            format::duration(tracks[0].duration),
            format::duration(limit)
        )));
    }

    Ok(Request {
        tracks,
        collection_name: (kind != spotify::Kind::Track).then(|| {
            collection
                .name
                .unwrap_or_else(|| format!("Spotify {}", kind.as_str()))
        }),
        source: TrackSource::Spotify,
        truncated,
    })
}

/// Segue o redirecionamento de um link curto e devolve o destino. Pelo
/// cliente compartilhado, que ja recusa endereco interno - e o destino ainda
/// passa pelas listas de host de quem chamou.
async fn expand_short_link(http: &reqwest::Client, url: &Url) -> Result<Url, MusicError> {
    let response = http
        .get(url.clone())
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|_| {
            MusicError::user("I couldn't open that short link. Paste the full link instead.")
        })?;

    let last = response.url().clone();
    if !response.status().is_success() || host_in(&last, SHORT_HOSTS) {
        return Err(MusicError::user(
            "That short link didn't lead anywhere I can play.",
        ));
    }
    Ok(last)
}

pub fn is_soundcloud_track_url(url: &str) -> bool {
    let Ok(url) = Url::parse(url) else {
        return false;
    };
    if url.scheme() != "https" {
        return false;
    }

    let parts: Vec<&str> = url.path().split('/').filter(|p| !p.is_empty()).collect();

    if host_in(&url, SOUNDCLOUD_API_HOSTS) {
        return parts.len() == 2
            && parts[0] == "tracks"
            && parts[1].bytes().all(|b| b.is_ascii_digit());
    }

    if !host_in(&url, SOUNDCLOUD_HOSTS) {
        return false;
    }

    // /artista/faixa, ou /artista/faixa/s-TOKEN de uma faixa privada
    // compartilhada por link.
    (parts.len() == 2 || (parts.len() == 3 && parts[2].starts_with("s-")))
        && !SOUNDCLOUD_NON_TRACK
            .iter()
            .any(|x| x.eq_ignore_ascii_case(parts[1]))
}

/// Entrada de set antes da consulta: o nome sai do endereco
/// ("sleepy-fish-cable-knit-sweater-1" vira "Sleepy fish cable knit sweater
/// 1") ate a vez dela chegar.
fn placeholder(url: &str) -> TrackInfo {
    let parsed = Url::parse(url).ok();
    let slug = parsed
        .as_ref()
        .and_then(|u| u.path().split('/').filter(|p| !p.is_empty()).nth(1))
        .unwrap_or(url)
        .to_owned();

    // Pela API nao ha nome no endereco, so o numero.
    let raw = if parsed
        .as_ref()
        .is_some_and(|u| host_in(u, SOUNDCLOUD_API_HOSTS))
    {
        "SoundCloud track".to_owned()
    } else {
        slug.replace(['-', '_'], " ")
    };
    let mut chars = raw.chars();
    let title = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => raw,
    };

    TrackInfo {
        id: slug,
        title,
        webpage_url: url.to_owned(),
        duration: Duration::ZERO,
        uploader: None,
        thumbnail_url: None,
        source: TrackSource::SoundCloud,
    }
}

// ----------------------------------------------------------------------
// Na vez da faixa
// ----------------------------------------------------------------------

/// A faixa do YouTube ou do SoundCloud que vai tocar no lugar desta entrada.
/// Consulta a entrada de set, e casa a faixa do Spotify.
pub async fn resolve_playable(
    entry: &QueuedTrack,
    settings: &MusicSettings,
) -> Result<TrackInfo, MusicError> {
    if let Some(ready) = entry.playable() {
        return Ok(ready);
    }

    let track = &entry.track;
    if track.source == TrackSource::Spotify {
        match_spotify(track, settings).await
    } else {
        ytdlp::resolve(&track.webpage_url, track.source, settings).await
    }
}

/// O que se procura de uma faixa do Spotify, ja normalizado.
struct Wanted {
    query: String,
    title: String,
    artist: String,
    spotify_title: String,
    duration: Duration,
    tolerance: Duration,
}

impl Wanted {
    fn new(spotify: &TrackInfo) -> Self {
        let artist = spotify
            .uploader
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned();
        let title = base_title(&spotify.title);
        let query = format!("{artist} {title}").trim().to_owned();
        let tolerance =
            Duration::from_secs_f64(f64::max(3.0, spotify.duration.as_secs_f64() * 0.03));

        Self {
            query,
            title: normalize(&title),
            artist: normalize(&artist),
            spotify_title: normalize(&spotify.title),
            duration: spotify.duration,
            tolerance,
        }
    }

    /// "Bater" no SoundCloud e exigente de proposito - la ha cover, remix e
    /// backing track de tudo. Tem de valer tudo junto: a duracao coincide (3s
    /// ou 3%), o titulo contem o nome da musica, o artista e quem subiu (ou
    /// esta no titulo, num reupload "Artista - Musica"), e nada de palavra de
    /// versao que o Spotify nao tenha.
    fn accepts(&self, candidate: &TrackInfo) -> bool {
        if candidate.duration.abs_diff(self.duration) > self.tolerance || self.title.is_empty() {
            return false;
        }

        let title = normalize(&candidate.title);
        let uploader = normalize(candidate.uploader.as_deref().unwrap_or_default());
        let all = format!("{title} {uploader}");

        if VARIANT_WORDS
            .iter()
            .any(|w| has_word(&all, w) && !has_word(&self.spotify_title, w))
        {
            return false;
        }

        if !title.contains(&self.title) {
            return false;
        }

        !self.artist.is_empty()
            && (uploader == self.artist
                || uploader.starts_with(&format!("{} ", self.artist))
                || title.contains(&self.artist))
    }
}

/// Procura a faixa do Spotify primeiro no SoundCloud (toca por streaming) e,
/// se nada bater, no YouTube Music (baixa antes, como todo YouTube). No
/// YouTube Music a secao de musicas ja traz a gravacao oficial primeiro; ali
/// a duracao so desempata entre os tres primeiros.
async fn match_spotify(
    spotify: &TrackInfo,
    settings: &MusicSettings,
) -> Result<TrackInfo, MusicError> {
    let wanted = Wanted::new(spotify);

    // Tres: cada resultado e uma consulta inteira, e a gravacao oficial,
    // quando esta no SoundCloud, vem no topo.
    match ytdlp::search_soundcloud(&wanted.query, 3, settings).await {
        Ok(candidates) => {
            if let Some(found) = candidates.into_iter().find(|c| wanted.accepts(c)) {
                info!("[musica] spotify {} -> soundcloud {}", spotify.id, found.id);
                return Ok(found);
            }
        }
        // SoundCloud fora do ar nao impede o YouTube de tentar.
        Err(e) => warn!(
            "[musica] aviso: busca no SoundCloud falhou para spotify {}: {e}",
            spotify.id
        ),
    }

    let ids = ytdlp::search_youtube_music(&wanted.query, 3, settings).await?;
    let mut first = None;

    for id in ids {
        let url = format!("https://www.youtube.com/watch?v={id}");
        let video = match ytdlp::resolve(&url, TrackSource::YouTube, settings).await {
            Ok(video) => video,
            Err(MusicError::User(_)) => continue,
            Err(other) => return Err(other),
        };

        if video.duration.abs_diff(spotify.duration) <= Duration::from_secs(15) {
            info!("[musica] spotify {} -> youtube {}", spotify.id, video.id);
            return Ok(video);
        }
        first.get_or_insert(video);
    }

    if let Some(first) = first {
        info!(
            "[musica] spotify {} -> youtube {} (sem duracao igual; o primeiro resultado)",
            spotify.id, first.id
        );
        return Ok(first);
    }

    Err(MusicError::user(
        "I couldn't find this song on SoundCloud or YouTube.",
    ))
}

/// "Money - 2011 Remaster" e "Song (feat. X)" viram so o nome da musica.
fn base_title(title: &str) -> String {
    let mut title = title;
    if let Some(cut) = title.find(" - ")
        && cut > 0
    {
        title = &title[..cut];
    }
    if let Some(paren) = title.find(" (")
        && paren > 0
    {
        title = &title[..paren];
    }
    title.trim().to_owned()
}

/// Palavra (ou expressao) inteira, no singular ou no plural: "live" nao casa
/// com "olive", nem "cover" com "discover" - mas "backing track" casa com o
/// canal "Pink Floyd Backing Tracks".
fn has_word(normalized: &str, word: &str) -> bool {
    let padded = format!(" {normalized} ");
    padded.contains(&format!(" {word} ")) || padded.contains(&format!(" {word}s "))
}

/// Minusculo, sem acento e so letra, digito e espaco simples.
fn normalize(text: &str) -> String {
    let mapped: String = text
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(|c| {
            let keep = c.is_alphanumeric();
            let lower: Vec<char> = if keep {
                c.to_lowercase().collect()
            } else {
                vec![' ']
            };
            lower
        })
        .collect();
    mapped.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::music::track::sample;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn classifica_o_texto_do_play() {
        assert_eq!(parse_input("   "), Input::Empty);
        assert_eq!(
            parse_input("spotify:track:x"),
            Input::SpotifyUri("spotify:track:x".into())
        );
        assert_eq!(
            parse_input("youtu.be/abc"),
            Input::Link(url("https://youtu.be/abc"))
        );
        assert_eq!(
            parse_input(" https://soundcloud.com/a/b "),
            Input::Link(url("https://soundcloud.com/a/b"))
        );
        assert_eq!(parse_input("ftp://x.com/a"), Input::BadLink);
        assert_eq!(
            parse_input("never gonna"),
            Input::Search("never gonna".into())
        );
        assert_eq!(parse_input("a.b"), Input::Search("a.b".into()));
    }

    #[test]
    fn classifica_o_host() {
        assert_eq!(
            classify(&url("https://WWW.YouTube.com/watch?v=x")),
            LinkKind::YouTube
        );
        assert_eq!(
            classify(&url("https://www.youtube.com/playlist?list=x")),
            LinkKind::YouTubePlaylist
        );
        assert_eq!(
            classify(&url("https://on.soundcloud.com/abc")),
            LinkKind::Short
        );
        assert_eq!(
            classify(&url("https://m.soundcloud.com/a/b")),
            LinkKind::SoundCloud
        );
        assert_eq!(
            classify(&url("https://open.spotify.com/track/x")),
            LinkKind::Spotify
        );
        assert_eq!(
            classify(&url("https://example.com/a")),
            LinkKind::Unsupported
        );
        assert_eq!(classify(&url("http://127.0.0.1/a")), LinkKind::Unsupported);
    }

    #[test]
    fn reconhece_faixa_do_soundcloud() {
        assert!(is_soundcloud_track_url(
            "https://soundcloud.com/artista/faixa"
        ));
        assert!(is_soundcloud_track_url(
            "https://soundcloud.com/artista/faixa/s-AbC123"
        ));
        assert!(is_soundcloud_track_url(
            "https://api-v2.soundcloud.com/tracks/123"
        ));
        assert!(!is_soundcloud_track_url(
            "https://api-v2.soundcloud.com/tracks/12a"
        ));
        assert!(!is_soundcloud_track_url(
            "https://soundcloud.com/artista/sets/x"
        ));
        assert!(!is_soundcloud_track_url(
            "https://soundcloud.com/artista/Likes"
        ));
        assert!(!is_soundcloud_track_url("https://soundcloud.com/artista"));
        assert!(!is_soundcloud_track_url(
            "http://soundcloud.com/artista/faixa"
        ));
        assert!(!is_soundcloud_track_url(
            "https://example.com/artista/faixa"
        ));
    }

    #[test]
    fn nome_provisorio_sai_do_endereco() {
        let track = placeholder("https://soundcloud.com/a/sleepy-fish_cable-1");
        assert_eq!(track.title, "Sleepy fish cable 1");
        assert_eq!(track.id, "sleepy-fish_cable-1");
        assert!(track.is_placeholder());
        assert_eq!(
            placeholder("https://api-v2.soundcloud.com/tracks/123").title,
            "SoundCloud track"
        );
    }

    #[test]
    fn sem_query_nem_fragmento() {
        assert_eq!(
            left_part(&url("https://soundcloud.com/a/b?si=1#t=2")).as_str(),
            "https://soundcloud.com/a/b"
        );
    }

    #[test]
    fn titulo_base_palavras_e_normalizacao() {
        assert_eq!(base_title("Money - 2011 Remaster"), "Money");
        assert_eq!(base_title("Song (feat. X) - Live"), "Song");
        assert_eq!(base_title("- abc"), "- abc");
        assert_eq!(normalize("  Ação, Já!  Écran "), "acao ja ecran");
        assert!(has_word("pink floyd backing tracks", "backing track"));
        assert!(!has_word("olive tree", "live"));
        assert!(!has_word("discover", "cover"));
        assert!(has_word("song live", "live"));
    }

    fn spotify(title: &str, artists: &str, seconds: u64) -> TrackInfo {
        let mut track = sample(TrackSource::Spotify, seconds);
        track.title = title.into();
        track.uploader = Some(artists.into());
        track
    }

    fn candidate(title: &str, uploader: &str, seconds: u64) -> TrackInfo {
        let mut track = sample(TrackSource::SoundCloud, seconds);
        track.title = title.into();
        track.uploader = Some(uploader.into());
        track
    }

    #[test]
    fn casamento_exigente_no_soundcloud() {
        let wanted = Wanted::new(&spotify("Time - 2011 Remaster", "Pink Floyd, X", 413));
        assert_eq!(wanted.query, "Pink Floyd Time");
        // 3% de 413 = 12,39 s de tolerancia.
        assert!(wanted.accepts(&candidate("Time", "Pink Floyd", 420)));
        assert!(wanted.accepts(&candidate("Pink Floyd - Time", "fulano", 413)));
        assert!(wanted.accepts(&candidate("Time", "Pink Floyd Official", 413)));
        assert!(!wanted.accepts(&candidate("Time", "Pink Floyd", 430)));
        assert!(!wanted.accepts(&candidate("Time (Backing Track)", "Pink Floyd", 413)));
        assert!(!wanted.accepts(&candidate("Time", "Pink Floyd Backing Tracks", 413)));
        assert!(!wanted.accepts(&candidate("Other", "Pink Floyd", 413)));
        assert!(!wanted.accepts(&candidate("Time", "fulano", 413)));

        // A palavra de versao que o proprio Spotify tem nao derruba.
        let live = Wanted::new(&spotify("Time - Live", "Pink Floyd", 413));
        assert!(live.accepts(&candidate("Time (Live)", "Pink Floyd", 413)));
        // Sem artista, nada bate.
        let anonymous = Wanted::new(&spotify("Time", "", 413));
        assert!(!anonymous.accepts(&candidate("Time", "Pink Floyd", 413)));
        // Faixa curta: o piso e 3 s.
        let short = Wanted::new(&spotify("Hi", "A", 30));
        assert!(short.accepts(&candidate("Hi", "A", 33)));
        assert!(!short.accepts(&candidate("Hi", "A", 34)));
    }
}
