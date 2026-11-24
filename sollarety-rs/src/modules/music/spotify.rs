//! O que um link do Spotify diz sobre a musica - nome, artistas, duracao -,
//! lido da pagina PUBLICA de embed (open.spotify.com/embed/...) (Spotify.cs).
//!
//! Sem Web API de proposito: ela exige registrar uma aplicacao e guardar um
//! client secret, e para faixa, album e playlist publica o embed ja entrega
//! tudo o que o casamento precisa. O preco e depender do formato da pagina: se
//! o Spotify mudar o __NEXT_DATA__, e aqui que quebra, com uma linha clara no
//! log.
//!
//! Audio o Spotify nao entrega a ninguem (DRM). Quem toca e o SoundCloud ou o
//! YouTube - ver `sources::match_spotify`.

use std::time::Duration;

use serde_json::Value;
use tracing::{error, warn};

use super::track::MusicError;

const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Track,
    Album,
    Playlist,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Track => "track",
            Self::Album => "album",
            Self::Playlist => "playlist",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub artists: String,
    pub duration: Duration,
    pub image_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Collection {
    pub name: Option<String>,
    pub tracks: Vec<Item>,
}

/// Os ids do Spotify sao base62 de 22 caracteres. Validar aqui e o que
/// garante que a URL do embed montada abaixo nao carrega nada alheio.
fn is_id(id: &str) -> bool {
    id.len() == 22 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// open.spotify.com/track/ID, /intl-pt/album/ID, /embed/playlist/ID ou
/// spotify:track:ID. Artista, podcast e episodio ficam de fora.
pub fn try_parse(input: &str) -> Option<(Kind, String)> {
    let parts: Vec<String> = if input
        .get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case("spotify:"))
    {
        input
            .split(':')
            .filter(|p| !p.is_empty())
            .skip(1)
            .map(str::to_owned)
            .collect()
    } else {
        let url = url::Url::parse(input).ok()?;
        url.path()
            .split('/')
            .filter(|p| !p.is_empty())
            .filter(|p| {
                !p.get(..5).is_some_and(|x| x.eq_ignore_ascii_case("intl-")) && *p != "embed"
            })
            .map(str::to_owned)
            .collect()
    };

    let kind = match parts.first()?.as_str() {
        "track" => Kind::Track,
        "album" => Kind::Album,
        "playlist" => Kind::Playlist,
        _ => return None,
    };
    let id = parts.get(1)?;
    is_id(id).then(|| (kind, id.clone()))
}

pub fn track_url(id: &str) -> String {
    format!("https://open.spotify.com/track/{id}")
}

pub async fn fetch(http: &reqwest::Client, kind: Kind, id: &str) -> Result<Collection, MusicError> {
    let kind_name = kind.as_str();
    let unavailable = || MusicError::user("Spotify didn't answer. Try again in a moment.");

    let response = http
        .get(format!("https://open.spotify.com/embed/{kind_name}/{id}"))
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|e| {
            warn!("[musica] aviso: embed do Spotify falhou para {kind_name}/{id}: {e}");
            unavailable()
        })?;

    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::BAD_REQUEST {
        return Err(MusicError::user(
            "That Spotify link doesn't exist or isn't public.",
        ));
    }
    if !status.is_success() {
        warn!(
            "[musica] aviso: embed do Spotify respondeu {} para {kind_name}/{id}",
            status.as_u16()
        );
        return Err(unavailable());
    }

    let html = response.text().await.map_err(|e| {
        warn!("[musica] aviso: embed do Spotify falhou para {kind_name}/{id}: {e}");
        unavailable()
    })?;

    parse_embed(kind, id, &html).map_err(|reason| {
        // A pagina mudou de formato. E o ponto fragil documentado acima.
        error!("[musica] falha: o embed do Spotify mudou de formato ({kind_name}/{id}): {reason}");
        MusicError::user(
            "I couldn't read that Spotify link. Try a YouTube or SoundCloud link instead.",
        )
    })
}

/// O que interessa da pagina de embed. Erro com o motivo (para o log) se o
/// formato nao for o esperado.
pub fn parse_embed(kind: Kind, id: &str, html: &str) -> Result<Collection, String> {
    const OPEN: &str = "<script id=\"__NEXT_DATA__\" type=\"application/json\">";
    let start = html.find(OPEN).ok_or("sem __NEXT_DATA__")? + OPEN.len();
    let end = html[start..].find("</script>").ok_or("sem __NEXT_DATA__")?;
    let data: Value = serde_json::from_str(&html[start..start + end]).map_err(|e| e.to_string())?;
    let entity = data
        .pointer("/props/pageProps/state/data/entity")
        .filter(|v| !v.is_null())
        .ok_or("sem props.pageProps.state.data.entity")?;

    let image = largest_image(entity);
    let millis = |v: &Value| {
        Duration::try_from_secs_f64(v.as_f64().unwrap_or(0.0) / 1000.0).unwrap_or_default()
    };
    let name = entity["name"]
        .as_str()
        .or(entity["title"].as_str())
        .map(str::to_owned);

    if kind == Kind::Track {
        let artists = entity["artists"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a["name"].as_str())
            .filter(|n| !n.trim().is_empty())
            .collect::<Vec<_>>()
            .join(", ");

        let track = Item {
            id: id.to_owned(),
            title: name.unwrap_or_else(|| "Unknown track".into()),
            artists,
            duration: millis(&entity["duration"]),
            image_url: image,
        };
        return Ok(Collection {
            name: None,
            tracks: vec![track],
        });
    }

    let tracks = entity["trackList"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["isPlayable"].as_bool() != Some(false))
        .map(|t| Item {
            id: t["uri"]
                .as_str()
                .and_then(|u| u.split(':').next_back())
                .unwrap_or_default()
                .to_owned(),
            title: t["title"].as_str().unwrap_or("Unknown track").to_owned(),
            // "A, B": o embed separa os artistas com espaco rigido.
            artists: t["subtitle"]
                .as_str()
                .unwrap_or_default()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
            duration: millis(&t["duration"]),
            image_url: image.clone(),
        })
        .filter(|t| is_id(&t.id))
        .collect();

    Ok(Collection { name, tracks })
}

/// A maior capa do embed; so https do CDN de imagens do Spotify.
fn largest_image(entity: &Value) -> Option<String> {
    let mut images: Vec<&Value> = entity
        .pointer("/visualIdentity/image")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    // Estavel, como o OrderByDescending: empate fica na ordem do embed.
    images.sort_by_key(|i| std::cmp::Reverse(i["maxWidth"].as_i64().unwrap_or(0)));
    let url = images.iter().find_map(|i| i["url"].as_str())?;

    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    (parsed.scheme() == "https"
        && (host.ends_with(".spotifycdn.com") || host.ends_with(".scdn.co")))
    .then(|| parsed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "4cOdK2wGLETKBW3PvgPWqT";

    #[test]
    fn le_links_e_uris() {
        let track = Some((Kind::Track, ID.to_owned()));
        assert_eq!(
            try_parse(&format!("https://open.spotify.com/track/{ID}")),
            track
        );
        assert_eq!(
            try_parse(&format!("https://open.spotify.com/intl-pt/track/{ID}?si=x")),
            track
        );
        assert_eq!(try_parse(&format!("spotify:track:{ID}")), track);
        assert_eq!(try_parse(&format!("SPOTIFY:track:{ID}")), track);
        assert_eq!(
            try_parse(&format!("https://open.spotify.com/embed/playlist/{ID}")),
            Some((Kind::Playlist, ID.to_owned()))
        );
        assert_eq!(
            try_parse(&format!("https://open.spotify.com/artist/{ID}")),
            None
        );
        assert_eq!(try_parse("https://open.spotify.com/track/curto"), None);
        assert_eq!(try_parse("https://open.spotify.com/"), None);
        assert_eq!(try_parse("texto qualquer"), None);
    }

    fn page(entity: serde_json::Value) -> String {
        let data =
            serde_json::json!({"props": {"pageProps": {"state": {"data": {"entity": entity}}}}});
        format!(
            "<html><script id=\"__NEXT_DATA__\" type=\"application/json\">{data}</script></html>"
        )
    }

    #[test]
    fn le_a_faixa_do_embed() {
        let html = page(serde_json::json!({
            "name": "Song",
            "duration": 213573,
            "artists": [{"name": "A"}, {"name": " "}, {"name": "B"}],
            "visualIdentity": {"image": [
                {"url": "https://image-cdn-ak.spotifycdn.com/image/small", "maxWidth": 64},
                {"url": "https://image-cdn-ak.spotifycdn.com/image/big", "maxWidth": 640},
            ]}
        }));
        let collection = parse_embed(Kind::Track, ID, &html).unwrap();
        assert_eq!(collection.name, None);
        let track = &collection.tracks[0];
        assert_eq!(track.title, "Song");
        assert_eq!(track.artists, "A, B");
        assert_eq!(track.duration, Duration::from_millis(213573));
        assert_eq!(
            track.image_url.as_deref(),
            Some("https://image-cdn-ak.spotifycdn.com/image/big")
        );
    }

    #[test]
    fn le_a_lista_do_embed() {
        let html = page(serde_json::json!({
            "title": "Album",
            "visualIdentity": {"image": [{"url": "https://evil.example/x.jpg", "maxWidth": 640}]},
            "trackList": [
                {"uri": format!("spotify:track:{ID}"), "title": "One", "subtitle": "A,\u{a0}B", "duration": 1000},
                {"uri": "spotify:track:ruim", "title": "Two"},
                {"uri": format!("spotify:track:{ID}"), "title": "Three", "isPlayable": false},
            ]
        }));
        let collection = parse_embed(Kind::Album, ID, &html).unwrap();
        assert_eq!(collection.name.as_deref(), Some("Album"));
        assert_eq!(collection.tracks.len(), 1);
        assert_eq!(collection.tracks[0].artists, "A, B");
        assert_eq!(collection.tracks[0].image_url, None);
    }

    #[test]
    fn formato_mudado_e_erro() {
        assert!(parse_embed(Kind::Track, ID, "<html></html>").is_err());
        assert!(parse_embed(Kind::Track, ID, &page(serde_json::Value::Null)).is_err());
    }
}
