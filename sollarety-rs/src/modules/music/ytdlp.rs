//! A ponte com o yt-dlp, para YouTube e SoundCloud (YtDlp.cs).
//!
//! Duas regras valem para todo processo daqui: argumentos sempre em lista
//! (nada de string montada, que seria injecao de opcao por titulo de video),
//! e sempre um "--" antes do alvo, para que uma busca comecando com "-" nao
//! vire flag. O alvo em si nunca e uma URL qualquer: o `sources` so deixa
//! chegar aqui host do YouTube, do SoundCloud ou uma busca com prefixo fixo.

use std::{
    path::{Path, PathBuf},
    sync::LazyLock,
    time::Duration,
};

use serde_json::Value;
use tokio::sync::Semaphore;
use tracing::warn;

use super::{
    format, sources,
    track::{MusicError, TrackInfo, TrackSource},
};
use crate::{config::MusicSettings, embeds, process};

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(45);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(180);

// Download e banda e disco desta maquina, compartilhados por todos os
// servidores. Dois de cada vez cobrem o prefetch de dois servidores sem
// deixar um /play em massa afogar a conexao.
static DOWNLOADS: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(2));

// ----------------------------------------------------------------------
// Metadados
// ----------------------------------------------------------------------

/// Consulta uma faixa sem baixar nada, e recusa o que nao pode tocar: live,
/// longa demais, previa de 30s do SoundCloud Go+, ou um extrator que nao seja
/// o esperado.
pub async fn resolve(
    target: &str,
    source: TrackSource,
    settings: &MusicSettings,
) -> Result<TrackInfo, MusicError> {
    let result = run(
        settings,
        RESOLVE_TIMEOUT,
        false,
        &["--skip-download", "--dump-json", "--", target],
    )
    .await?;

    let Some(line) = json_lines(&result.stdout).next() else {
        if result.exit_code == 0 && is_search(target) {
            return Err(MusicError::user(
                "I couldn't find anything for that search.",
            ));
        }
        warn!(
            "[musica] aviso: yt-dlp nao resolveu '{target}' (saida {}): {}",
            result.exit_code,
            last_lines(&result.stderr)
        );
        return Err(MusicError::User(explain(&result.stderr, source)));
    };

    validate(line, source, settings).map_err(MusicError::User)
}

/// Busca no SoundCloud e devolve so o que pode tocar, na ordem do resultado.
pub async fn search_soundcloud(
    query: &str,
    count: usize,
    settings: &MusicSettings,
) -> Result<Vec<TrackInfo>, MusicError> {
    let target = format!("scsearch{count}:{query}");
    let result = run(
        settings,
        RESOLVE_TIMEOUT,
        false,
        &["--skip-download", "--dump-json", "--", &target],
    )
    .await?;

    Ok(json_lines(&result.stdout)
        .filter_map(|line| validate(line, TrackSource::SoundCloud, settings).ok())
        .collect())
}

/// Ids de video da busca de MUSICAS do YouTube Music - a secao "songs" traz
/// primeiro a gravacao oficial (os canais "- Topic"), que e o que o casamento
/// com o Spotify quer. So a lista: nada e consultado a fundo.
pub async fn search_youtube_music(
    query: &str,
    count: usize,
    settings: &MusicSettings,
) -> Result<Vec<String>, MusicError> {
    // Host e caminho fixos; o texto do usuario so entra escapado no q=.
    let url = format!(
        "https://music.youtube.com/search?q={}#songs",
        escape_data_string(query)
    );
    let items = format!("1:{count}");
    let result = run(
        settings,
        RESOLVE_TIMEOUT,
        true,
        &[
            "--flat-playlist",
            "--dump-single-json",
            "--playlist-items",
            &items,
            "--",
            &url,
        ],
    )
    .await?;

    let Some(json) = json_lines(&result.stdout).next() else {
        return Ok(Vec::new());
    };
    let json: Value = serde_json::from_str(json)
        .map_err(|e| MusicError::Other(format!("JSON da busca do YouTube Music: {e}")))?;

    Ok(json["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["id"].as_str())
        .filter(|id| !id.trim().is_empty())
        .map(str::to_owned)
        .collect())
}

/// Lista um set (ou perfil) do SoundCloud sem consultar cada faixa: a
/// listagem so traz as URLs, e o resto e buscado quando chegar a vez.
pub async fn list_soundcloud(
    url: &str,
    max: usize,
    settings: &MusicSettings,
) -> Result<(Option<String>, Vec<String>), MusicError> {
    let items = format!("1:{max}");
    let result = run(
        settings,
        RESOLVE_TIMEOUT,
        true,
        &[
            "--flat-playlist",
            "--dump-single-json",
            "--playlist-items",
            &items,
            "--",
            url,
        ],
    )
    .await?;

    let Some(json) = json_lines(&result.stdout).next() else {
        warn!(
            "[musica] aviso: yt-dlp nao listou '{url}' (saida {}): {}",
            result.exit_code,
            last_lines(&result.stderr)
        );
        return Err(MusicError::User(explain(
            &result.stderr,
            TrackSource::SoundCloud,
        )));
    };
    let json: Value = serde_json::from_str(json)
        .map_err(|e| MusicError::Other(format!("JSON da listagem do SoundCloud: {e}")))?;

    Ok(parse_listing(&json))
}

fn parse_listing(json: &Value) -> (Option<String>, Vec<String>) {
    let urls = json["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["url"].as_str())
        .filter(|u| sources::is_soundcloud_track_url(u))
        .map(str::to_owned)
        .collect();
    (json["title"].as_str().map(str::to_owned), urls)
}

// ----------------------------------------------------------------------
// Audio
// ----------------------------------------------------------------------

/// URL do audio de uma faixa do SoundCloud, para o ffmpeg ler direto. Pedida
/// na hora de tocar, e nao guardada: os links de HLS do SoundCloud expiram.
pub async fn stream_url(track: &TrackInfo, settings: &MusicSettings) -> Result<String, MusicError> {
    let result = run(
        settings,
        RESOLVE_TIMEOUT,
        false,
        &[
            "--format",
            "bestaudio/best",
            "--get-url",
            "--",
            &track.webpage_url,
        ],
    )
    .await?;

    let first = result
        .stdout
        .split('\n')
        .map(str::trim)
        .find(|l| !l.is_empty());

    // So https: o ffmpeg vai abrir isto, e com a lista de protocolos restrita
    // a https ele nao aceitaria outra coisa de qualquer forma.
    match first.and_then(|u| url::Url::parse(u).ok()) {
        Some(url) if url.scheme() == "https" => Ok(url.to_string()),
        _ => {
            warn!(
                "[musica] aviso: sem URL de audio para {} (saida {}): {}",
                track.webpage_url,
                result.exit_code,
                last_lines(&result.stderr)
            );
            Err(MusicError::User(explain(
                &result.stderr,
                TrackSource::SoundCloud,
            )))
        }
    }
}

/// Apaga `<stem>.*` ao sair de escopo, a menos que seja desarmado. E o que
/// cobre o download cancelado: aqui cancelar e largar o future, e nao ha
/// catch onde limpar o .part que o yt-dlp deixou.
struct PartialFiles<'a> {
    directory: &'a Path,
    stem: &'a str,
    armed: bool,
}

impl Drop for PartialFiles<'_> {
    fn drop(&mut self) {
        if self.armed {
            delete_matching(self.directory, self.stem);
        }
    }
}

/// Baixa so o audio de um video do YouTube para dentro de `directory` e
/// devolve o caminho. Quem chama e dono do arquivo e tem de apaga-lo.
pub async fn download(
    track: &TrackInfo,
    directory: &Path,
    settings: &MusicSettings,
) -> Result<PathBuf, MusicError> {
    tokio::fs::create_dir_all(directory)
        .await
        .map_err(|e| MusicError::Other(format!("nao consegui criar a pasta: {e}")))?;

    // Nome aleatorio, e nao o id do video: a mesma faixa pode estar tocando e
    // sendo pre-baixada ao mesmo tempo (loop de fila com uma faixa so), e um
    // apagaria o arquivo do outro.
    let stem = format!("{:032x}", rand::random::<u128>());
    let mut partial = PartialFiles {
        directory,
        stem: &stem,
        armed: true,
    };

    let _permit = DOWNLOADS
        .acquire()
        .await
        .map_err(|e| MusicError::Other(e.to_string()))?;

    let max_size = format!("{}M", settings.max_file_size_megabytes());
    let filter = format!(
        "duration <= {} & !is_live",
        settings.max_track_length().as_secs()
    );
    let paths = directory.to_string_lossy();
    let output = format!("{stem}.%(ext)s");
    let args = [
        "--no-progress",
        "--format",
        "bestaudio/best",
        "--max-filesize",
        &max_size,
        "--match-filters",
        &filter,
        "--paths",
        &paths,
        "--output",
        &output,
        "--print",
        "after_move:filepath",
        "--no-simulate",
        "--",
        &track.webpage_url,
    ];

    // O prazo de fora vence o do process::run (que e so a rede de seguranca):
    // assim o estouro e reconhecido aqui e vira a frase certa. Largar o future
    // mata o grupo do yt-dlp.
    let result = match tokio::time::timeout(
        DOWNLOAD_TIMEOUT,
        run(
            settings,
            DOWNLOAD_TIMEOUT + Duration::from_secs(30),
            false,
            &args,
        ),
    )
    .await
    {
        Ok(result) => result?,
        Err(_) => {
            return Err(MusicError::user(
                "The download took too long, so I gave up on it.",
            ));
        }
    };

    let printed = result
        .stdout
        .split('\n')
        .map(str::trim)
        .rfind(|l| !l.is_empty());

    // Confere que o caminho impresso e mesmo daqui de dentro antes de
    // entregar a quem vai apaga-lo depois.
    if let Some(printed) = printed
        && let (Ok(file), Ok(root)) = (
            std::fs::canonicalize(printed),
            std::fs::canonicalize(directory),
        )
        && file.is_file()
        && file.starts_with(&root)
    {
        partial.armed = false;
        return Ok(file);
    }

    // Filtro de duracao/tamanho: o yt-dlp PULA o video e sai com 0, sem
    // imprimir caminho. Sobra no maximo um .part, que o guarda apaga.
    warn!(
        "[musica] aviso: download de {} nao gerou arquivo (saida {}): {}",
        track.id,
        result.exit_code,
        last_lines(&result.stderr)
    );
    Err(MusicError::User(
        if result
            .stderr
            .to_lowercase()
            .contains("larger than max-filesize")
        {
            "That audio file is too large for me to download.".into()
        } else {
            explain(&result.stderr, TrackSource::YouTube)
        },
    ))
}

/// Versao do yt-dlp, para o relatorio de subida. Erro se nao houver yt-dlp.
pub async fn probe(settings: &MusicSettings) -> Result<String, crate::Error> {
    let result = process::run(
        &settings.yt_dlp_path,
        ["--version"],
        Duration::from_secs(15),
        &[],
    )
    .await?;
    if result.exit_code != 0 {
        return Err(format!(
            "yt-dlp --version saiu com {}: {}",
            result.exit_code,
            last_lines(&result.stderr)
        )
        .into());
    }
    Ok(result.stdout.trim().to_owned())
}

// ----------------------------------------------------------------------
// Interno
// ----------------------------------------------------------------------

async fn run(
    settings: &MusicSettings,
    timeout: Duration,
    allow_playlist: bool,
    args: &[&str],
) -> Result<process::ProcessResult, MusicError> {
    Ok(process::run(
        &settings.yt_dlp_path,
        build_args(settings, allow_playlist, args),
        timeout,
        &[],
    )
    .await?)
}

fn build_args(settings: &MusicSettings, allow_playlist: bool, args: &[&str]) -> Vec<String> {
    // --ignore-config: um ~/.config/yt-dlp/config desta conta nao pode mudar o
    // formato, a pasta ou ligar o download de playlist por baixo do bot.
    let mut all: Vec<String> = [
        "--ignore-config",
        "--no-warnings",
        "--no-color",
        "--socket-timeout",
        "20",
    ]
    .map(String::from)
    .into();

    // Listagem de set e busca do YouTube Music SAO playlists; o resto nunca
    // pode virar uma (um link de video com &list= baixaria a lista).
    if !allow_playlist {
        all.push("--no-playlist".into());
    }

    if !settings.js_runtimes.trim().is_empty() {
        all.push("--js-runtimes".into());
        all.push(settings.js_runtimes.clone());
    }

    all.extend(args.iter().map(|a| (*a).to_owned()));
    all
}

/// O JSON de uma faixa virou TrackInfo, ou a frase de por que ela nao toca.
fn validate(
    line: &str,
    source: TrackSource,
    settings: &MusicSettings,
) -> Result<TrackInfo, String> {
    let json: Value = match serde_json::from_str(line) {
        Ok(json) => json,
        Err(e) => {
            warn!("[musica] aviso: JSON do yt-dlp ilegivel: {e}");
            return Err("I got back something I couldn't read. Try again.".into());
        }
    };

    // Uma URL que redireciona para outro site cairia em outro extrator; e o
    // que o allowlist de host nao ve.
    let expected = if source == TrackSource::SoundCloud {
        "Soundcloud"
    } else {
        "Youtube"
    };
    if json["extractor_key"].as_str() != Some(expected) {
        return Err(if source == TrackSource::SoundCloud {
            "That link isn't a SoundCloud track.".into()
        } else {
            "That link isn't a YouTube video.".into()
        });
    }

    let live_status = json["live_status"].as_str();
    if json["is_live"].as_bool() == Some(true)
        || matches!(live_status, Some("is_live" | "is_upcoming"))
    {
        return Err("Live streams and premieres can't be played.".into());
    }

    // SoundCloud Go+: sem assinatura, so vem uma previa de 30s. Tocar isso
    // como se fosse a musica seria pior que recusar.
    if source == TrackSource::SoundCloud
        && let Some(formats) = json["formats"].as_array()
        && !formats.is_empty()
        && formats.iter().all(|f| {
            f["format_id"]
                .as_str()
                .is_some_and(|id| id.to_lowercase().contains("preview"))
        })
    {
        return Err(
            "That SoundCloud track is Go+ only — SoundCloud just gives me a 30-second preview."
                .into(),
        );
    }

    let id = json["id"].as_str().filter(|s| !s.trim().is_empty());
    let url = json["webpage_url"]
        .as_str()
        .filter(|s| !s.trim().is_empty());
    let seconds = json["duration"].as_f64().filter(|s| *s > 0.0);
    let (Some(id), Some(url), Some(seconds)) = (id, url, seconds) else {
        return Err("That track is missing information I need. Try another one.".into());
    };

    let duration = Duration::try_from_secs_f64(seconds).unwrap_or(Duration::MAX);
    let limit = if source == TrackSource::YouTube {
        settings.max_track_length()
    } else {
        settings.max_stream_length()
    };
    if duration > limit {
        return Err(format!(
            "That track is {} long; the limit is {}.",
            format::duration(duration),
            format::duration(limit)
        ));
    }

    Ok(TrackInfo {
        id: id.to_owned(),
        title: json["title"].as_str().unwrap_or(id).to_owned(),
        webpage_url: url.to_owned(),
        duration,
        uploader: json["channel"]
            .as_str()
            .or(json["uploader"].as_str())
            .map(str::to_owned),
        thumbnail_url: json["thumbnail"].as_str().map(str::to_owned),
        source,
    })
}

fn is_search(target: &str) -> bool {
    target.starts_with("ytsearch") || target.starts_with("scsearch")
}

fn json_lines(stdout: &str) -> impl Iterator<Item = &str> {
    stdout
        .split('\n')
        .map(str::trim)
        .filter(|l| l.starts_with('{'))
}

/// A frase para o usuario a partir do stderr do yt-dlp.
fn explain(stderr: &str, source: TrackSource) -> String {
    let lower = stderr.to_lowercase();
    let has = |s: &str| lower.contains(&s.to_lowercase());
    let site = if source == TrackSource::SoundCloud {
        "SoundCloud"
    } else {
        "YouTube"
    };

    if has("confirm your age") || has("age-restricted") || has("inappropriate for some users") {
        return "That video is age-restricted, so I can't play it.".into();
    }
    if has("Private video") || has("private") {
        return "That track is private.".into();
    }
    if has("not a bot") || has("HTTP Error 429") {
        return format!("{site} is blocking me right now. Try again later.");
    }
    if has("not available in your country")
        || has("blocked it in your country")
        || has("geo restrict")
    {
        return "That track isn't available where I'm hosted.".into();
    }
    if has("members-only") || has("Join this channel") {
        return "That video is for channel members only.".into();
    }
    if has("Video unavailable")
        || has("This video is unavailable")
        || has("has been removed")
        || has("HTTP Error 404")
    {
        return "That track is unavailable.".into();
    }
    "I couldn't load that. Try another link or search.".into()
}

/// As tres ultimas linhas do stderr, numa linha so, para o log.
pub fn last_lines(text: &str) -> String {
    let lines: Vec<&str> = text
        .split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let tail = &lines[lines.len().saturating_sub(3)..];
    embeds::trim(&tail.join(" | "), 600)
}

fn delete_matching(directory: &Path, stem: &str) {
    let prefix = format!("{stem}.");
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            warn!("[musica] aviso: nao consegui limpar {stem}.*: {e}");
            return;
        }
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&prefix)
            && let Err(e) = std::fs::remove_file(entry.path())
        {
            warn!("[musica] aviso: nao consegui limpar {stem}.*: {e}");
        }
    }
}

/// O Uri.EscapeDataString do C#: so os nao reservados do RFC 3986 passam,
/// o resto vira %XX dos bytes UTF-8.
fn escape_data_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> MusicSettings {
        MusicSettings::default()
    }

    #[test]
    fn valida_o_json_do_yt_dlp() {
        let line = r#"{"id":"jNQXAC9IVRw","title":"Me at the zoo","webpage_url":"https://www.youtube.com/watch?v=jNQXAC9IVRw","duration":19,"channel":"jawed","uploader":"x","thumbnail":"https://i.ytimg.com/vi/a.jpg","extractor_key":"Youtube","is_live":false,"live_status":"not_live"}"#;
        let track = validate(line, TrackSource::YouTube, &settings()).unwrap();
        assert_eq!(track.id, "jNQXAC9IVRw");
        assert_eq!(track.duration, Duration::from_secs(19));
        assert_eq!(track.uploader.as_deref(), Some("jawed"));
        assert_eq!(track.source, TrackSource::YouTube);

        // Extrator errado, live, sem duracao, longo demais.
        assert_eq!(
            validate(line, TrackSource::SoundCloud, &settings()).unwrap_err(),
            "That link isn't a SoundCloud track."
        );
        let live = line.replace("\"not_live\"", "\"is_upcoming\"");
        assert_eq!(
            validate(&live, TrackSource::YouTube, &settings()).unwrap_err(),
            "Live streams and premieres can't be played."
        );
        let no_duration = line.replace("\"duration\":19", "\"duration\":null");
        assert_eq!(
            validate(&no_duration, TrackSource::YouTube, &settings()).unwrap_err(),
            "That track is missing information I need. Try another one."
        );
        let long = line.replace("\"duration\":19", "\"duration\":901");
        assert_eq!(
            validate(&long, TrackSource::YouTube, &settings()).unwrap_err(),
            "That track is 15:01 long; the limit is 15:00."
        );
        assert_eq!(
            validate("{nada", TrackSource::YouTube, &settings()).unwrap_err(),
            "I got back something I couldn't read. Try again."
        );
    }

    #[test]
    fn recusa_previa_do_go_plus() {
        let line = r#"{"id":"1","webpage_url":"https://soundcloud.com/a/b","duration":30,"extractor_key":"Soundcloud","formats":[{"format_id":"hls_mp3_1_0_preview"},{"format_id":"http_mp3_preview"}]}"#;
        assert!(
            validate(line, TrackSource::SoundCloud, &settings())
                .unwrap_err()
                .contains("Go+")
        );
        let full = line.replace("http_mp3_preview", "http_mp3_128");
        let track = validate(&full, TrackSource::SoundCloud, &settings()).unwrap();
        // Sem titulo, vale o id; e o SoundCloud tem o teto do streaming.
        assert_eq!(track.title, "1");
    }

    #[test]
    fn explica_o_stderr() {
        let yt = TrackSource::YouTube;
        assert_eq!(
            explain("ERROR: Sign in to confirm your age", yt),
            "That video is age-restricted, so I can't play it."
        );
        assert_eq!(
            explain("ERROR: Sign in to confirm you're not a bot", yt),
            "YouTube is blocking me right now. Try again later."
        );
        assert_eq!(
            explain("HTTP Error 429", TrackSource::SoundCloud),
            "SoundCloud is blocking me right now. Try again later."
        );
        assert_eq!(
            explain("ERROR: Video unavailable", yt),
            "That track is unavailable."
        );
        assert_eq!(
            explain("", yt),
            "I couldn't load that. Try another link or search."
        );
    }

    #[test]
    fn linhas_de_json_e_fim_do_stderr() {
        let out = "aviso\n  {\"a\":1}\n\n{\"b\":2}\n";
        assert_eq!(
            json_lines(out).collect::<Vec<_>>(),
            vec!["{\"a\":1}", "{\"b\":2}"]
        );
        assert_eq!(last_lines("a\nb\n\nc\nd\n"), "b | c | d");
        assert_eq!(last_lines(""), "");
    }

    #[test]
    fn argumentos_fixos_e_sem_playlist() {
        let args = build_args(&settings(), false, &["--", "x"]);
        assert_eq!(
            args,
            [
                "--ignore-config",
                "--no-warnings",
                "--no-color",
                "--socket-timeout",
                "20",
                "--no-playlist",
                "--js-runtimes",
                "node",
                "--",
                "x"
            ]
        );
        let mut no_js = settings();
        no_js.js_runtimes = " ".into();
        let args = build_args(&no_js, true, &[]);
        assert!(
            !args
                .iter()
                .any(|a| a == "--no-playlist" || a == "--js-runtimes")
        );
    }

    #[test]
    fn escapa_como_o_escape_data_string() {
        assert_eq!(escape_data_string("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(escape_data_string("ação-~_."), "a%C3%A7%C3%A3o-~_.");
    }

    #[test]
    fn listagem_filtra_o_que_nao_e_faixa() {
        let json: Value = serde_json::json!({
            "title": "Set",
            "entries": [
                {"url": "https://soundcloud.com/a/b"},
                {"url": "https://soundcloud.com/a/sets/c"},
                {"url": "https://api-v2.soundcloud.com/tracks/123"},
                {"url": null},
            ]
        });
        let (title, urls) = parse_listing(&json);
        assert_eq!(title.as_deref(), Some("Set"));
        assert_eq!(
            urls,
            [
                "https://soundcloud.com/a/b",
                "https://api-v2.soundcloud.com/tracks/123"
            ]
        );
    }

    #[test]
    fn guarda_apaga_o_parcial() {
        let dir = std::env::temp_dir().join(format!("sollarety-ytdlp-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("abc.webm.part"), b"x").unwrap();
        std::fs::write(dir.join("abd.webm"), b"x").unwrap();
        drop(PartialFiles {
            directory: &dir,
            stem: "abc",
            armed: true,
        });
        assert!(!dir.join("abc.webm.part").exists());
        assert!(dir.join("abd.webm").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
