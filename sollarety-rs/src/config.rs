//! config/config.jsonc, com as mesmas chaves e padroes do JSONReader.cs.
//!
//! O arquivo e relido quando muda (carimbo pela data de modificacao), entao
//! editar o config vale na hora, sem reiniciar - o gitFeed, a musica, a IA e
//! a verificacao dependem disso. Use `config::current()` sempre que for ler;
//! nao guarde o Arc por muito tempo.

use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, SystemTime},
};

use json_comments::StripComments;
use serde::{Deserialize, Deserializer};

use crate::{Error, store::null_default};

/// As chaves que o C# le. O serde ignora as outras.
#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    #[serde(deserialize_with = "null_default")]
    pub token: String,
    /// Vazio registra os comandos globalmente.
    #[serde(deserialize_with = "null_default")]
    pub guild_ids: Vec<u64>,
    #[serde(deserialize_with = "null_default")]
    pub roblox_verify: RobloxVerifySettings,
    #[serde(deserialize_with = "null_default")]
    pub music: MusicSettings,
    #[serde(deserialize_with = "null_default")]
    pub git_feed: GitFeedSettings,
    #[serde(deserialize_with = "null_default")]
    pub local_ai: LocalAiSettings,
    #[serde(deserialize_with = "null_default")]
    pub admin: AdminSettings,

    // LEGADO. Estas seis chaves viraram configuracao por servidor (/config).
    // Continuam sendo lidas so para a migracao unica do LegacyConfigMigration.
    #[serde(deserialize_with = "id")]
    pub moderation_log_channel_id: Option<u64>,
    #[serde(deserialize_with = "id")]
    pub auto_softban_guild_id: Option<u64>,
    #[serde(deserialize_with = "id")]
    pub auto_softban_channel_id: Option<u64>,
    #[serde(deserialize_with = "id")]
    pub ticket_category_id: Option<u64>,
    #[serde(deserialize_with = "id")]
    pub ticket_staff_role_id: Option<u64>,
    #[serde(deserialize_with = "id")]
    pub ticket_log_channel_id: Option<u64>,
}

/// Nada de token nem segredo em log: o Debug esconde os dois.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("token", &redacted(&self.token))
            .field("guild_ids", &self.guild_ids)
            .field("roblox_verify", &self.roblox_verify)
            .field("music", &self.music)
            .field("git_feed", &self.git_feed)
            .field("local_ai", &self.local_ai)
            .field("admin", &self.admin)
            .finish_non_exhaustive()
    }
}

fn redacted(secret: &str) -> &'static str {
    if secret.is_empty() {
        "(vazio)"
    } else {
        "(oculto)"
    }
}

/// O Newtonsoft aceita um id como numero ou como texto; aqui tambem.
fn id<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    Ok(match Option::<serde_json::Value>::deserialize(d)? {
        Some(serde_json::Value::Number(n)) => n.as_u64(),
        Some(serde_json::Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    })
}

/// Onde fica o Worker de verificacao Roblox e o segredo que o bot usa para
/// buscar resultados nele. O segredo e da INSTALACAO (o BOT_API_SECRET do
/// Worker), por isso mora aqui, fora do git, como o token.
#[derive(Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RobloxVerifySettings {
    /// Pagina que a pessoa abre. Estatica: nao carrega identidade nenhuma.
    #[serde(deserialize_with = "null_default")]
    pub start_url: String,
    /// Endpoint que o bot consulta, com o segredo no Authorization.
    #[serde(deserialize_with = "null_default")]
    pub result_url: String,
    pub api_secret: Option<String>,
}

impl Default for RobloxVerifySettings {
    fn default() -> Self {
        Self {
            start_url: "https://daeese.me/oauth/roblox/start".into(),
            result_url: "https://daeese.me/oauth/roblox/result".into(),
            api_secret: None,
        }
    }
}

impl fmt::Debug for RobloxVerifySettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RobloxVerifySettings")
            .field("start_url", &self.start_url)
            .field("result_url", &self.result_url)
            .field(
                "api_secret",
                &redacted(self.api_secret.as_deref().unwrap_or("")),
            )
            .finish()
    }
}

impl RobloxVerifySettings {
    pub fn is_configured(&self) -> bool {
        let https = |u: &str| url::Url::parse(u).is_ok_and(|u| u.scheme() == "https");
        self.api_secret
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
            && https(&self.start_url)
            && https(&self.result_url)
    }
}

/// Musica: onde esta o yt-dlp e ate onde o bot aceita ir. Os limites sao do
/// disco e da banda DESTA maquina, por isso moram aqui e nao por servidor.
#[derive(Clone, Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MusicSettings {
    /// Absoluto na pratica: o PATH do systemd de usuario nao tem ~/.local/bin.
    #[serde(deserialize_with = "null_default")]
    pub yt_dlp_path: String,
    #[serde(deserialize_with = "null_default")]
    pub ffmpeg_path: String,
    /// Valor do --js-runtimes do yt-dlp.
    #[serde(deserialize_with = "null_default")]
    pub js_runtimes: String,
    pub max_track_minutes: i32,
    pub max_stream_minutes: i32,
    pub max_queue_length: i32,
    pub max_playlist_tracks: i32,
    #[serde(rename = "maxFileSizeMB")]
    pub max_file_size_mb: i32,
    pub idle_leave_minutes: i32,
}

impl Default for MusicSettings {
    fn default() -> Self {
        Self {
            yt_dlp_path: "yt-dlp".into(),
            ffmpeg_path: "ffmpeg".into(),
            js_runtimes: "node".into(),
            max_track_minutes: 15,
            max_stream_minutes: 180,
            max_queue_length: 50,
            max_playlist_tracks: 50,
            max_file_size_mb: 60,
            idle_leave_minutes: 2,
        }
    }
}

// Piso e teto aqui, e nao onde sao usados: um zero digitado no config viraria
// "nenhuma faixa cabe" ou "sai do canal na hora", sem nada no log.
impl MusicSettings {
    pub fn max_track_length(&self) -> Duration {
        minutes(self.max_track_minutes.clamp(1, 180))
    }

    pub fn max_stream_length(&self) -> Duration {
        minutes(self.max_stream_minutes.clamp(1, 720))
    }

    pub fn max_queue(&self) -> usize {
        self.max_queue_length.clamp(1, 500) as usize
    }

    pub fn max_playlist(&self) -> usize {
        self.max_playlist_tracks.clamp(1, 200) as usize
    }

    pub fn max_file_size_megabytes(&self) -> u64 {
        self.max_file_size_mb.clamp(5, 1024) as u64
    }

    pub fn idle_leave(&self) -> Duration {
        minutes(self.idle_leave_minutes.clamp(1, 60))
    }
}

fn minutes(n: i32) -> Duration {
    Duration::from_secs(n as u64 * 60)
}

/// Feed de commits: um repositorio git DESTA maquina e o canal que recebe o
/// resumo de cada commit novo.
#[derive(Clone, Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GitFeedSettings {
    pub repo_path: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub branch: String,
    #[serde(deserialize_with = "id")]
    pub guild_id: Option<u64>,
    #[serde(deserialize_with = "id")]
    pub channel_id: Option<u64>,
    /// Nome que aparece no topo de cada resumo.
    #[serde(deserialize_with = "null_default")]
    pub title: String,
    pub poll_seconds: i32,
    #[serde(deserialize_with = "null_default")]
    pub git_path: String,
}

impl Default for GitFeedSettings {
    fn default() -> Self {
        Self {
            repo_path: None,
            branch: "main".into(),
            guild_id: None,
            channel_id: None,
            title: "Commits".into(),
            poll_seconds: 30,
            git_path: "git".into(),
        }
    }
}

impl GitFeedSettings {
    pub fn is_configured(&self) -> bool {
        self.repo_path
            .as_deref()
            .is_some_and(|p| !p.trim().is_empty())
            && self.guild_id.is_some_and(|id| id > 0)
            && self.channel_id.is_some_and(|id| id > 0)
            && !self.branch.trim().is_empty()
    }

    /// Piso de 10s: cada volta roda um git.
    pub fn poll_interval(&self) -> Duration {
        Duration::from_secs(self.poll_seconds.clamp(10, 3600) as u64)
    }
}

/// IA local dividida com o ccore (llama.cpp).
#[derive(Clone, Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LocalAiSettings {
    #[serde(deserialize_with = "null_default")]
    pub base_url: String,
    /// Unicos servidores onde a IA responde.
    #[serde(deserialize_with = "null_default")]
    pub guild_ids: Vec<u64>,
    /// Quem escapa da trava de spam e ordens (AiGuard) e do intervalo.
    #[serde(deserialize_with = "null_default")]
    pub operator_user_ids: Vec<u64>,
    /// Vazio usa a persona padrao.
    pub system_prompt: Option<String>,
    pub cooldown_seconds: i32,
    pub max_tokens: i32,
}

impl Default for LocalAiSettings {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:9931".into(),
            guild_ids: Vec::new(),
            operator_user_ids: Vec::new(),
            system_prompt: None,
            cooldown_seconds: 20,
            max_tokens: 400,
        }
    }
}

impl LocalAiSettings {
    /// A base sem barra no fim.
    pub fn base_url(&self) -> &str {
        self.base_url.trim_end_matches('/')
    }
}

/// Modulo admin (/send, /role, /dm, /deployment, /dmdeployment): mandar
/// mensagem como o bot, mexer em cargo, DM em massa e convocacao. Como o painel do ccore, fica TRAVADO nos servidores
/// listados: os comandos so sao registrados la, e cada um confere de novo na
/// execucao (o arquivo muda sem novo registro). Vazio desliga em todo lugar.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AdminSettings {
    #[serde(deserialize_with = "null_default")]
    pub guild_ids: Vec<u64>,
    /// /deployment e /dmdeployment (o deployment do ccore).
    #[serde(deserialize_with = "null_default")]
    pub deployment: DeploymentSettings,
    /// Servidores que usam SO o /dm, liberado por cargo: id do servidor ->
    /// cargos que podem usar. O dono e Administrator sempre podem.
    #[serde(deserialize_with = "dm_guilds")]
    pub dm_guilds: BTreeMap<u64, DmGuild>,
}

/// Um servidor de admin.dmGuilds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DmGuild {
    /// Quem tem um destes cargos usa o /dm (e cancela uma DM em massa).
    pub role_ids: Vec<u64>,
}

/// Objeto { "<id do servidor>": { "roleIds": [..] } }. Chave que nao e id,
/// valor que nao e objeto (null inclusive) e chave desconhecida ficam de
/// fora; a mesma chave escrita de dois jeitos junta os cargos.
fn dm_guilds<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeMap<u64, DmGuild>, D::Error> {
    #[derive(Default, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    struct Entry {
        #[serde(deserialize_with = "ids")]
        role_ids: Vec<u64>,
    }
    let mut out: BTreeMap<u64, DmGuild> = BTreeMap::new();
    let Some(serde_json::Value::Object(map)) = Option::<serde_json::Value>::deserialize(d)? else {
        return Ok(out);
    };
    for (key, value) in map {
        let Some(guild) = key.trim().parse::<u64>().ok().filter(|&id| id != 0) else {
            continue;
        };
        if !value.is_object() {
            continue;
        }
        let Ok(entry) = serde_json::from_value::<Entry>(value) else {
            continue;
        };
        let roles = &mut out.entry(guild).or_default().role_ids;
        for id in entry.role_ids {
            if !roles.contains(&id) {
                roles.push(id);
            }
        }
    }
    Ok(out)
}

/// A convocacao para uma partida (o deployment do ccore). Tudo opcional e
/// lido com tolerancia: um valor de tipo errado vira "nao configurado" em vez
/// de derrubar o config inteiro, que e relido a quente.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DeploymentSettings {
    /// Canal onde o /deployment posta.
    #[serde(deserialize_with = "id")]
    pub channel_id: Option<u64>,
    /// Quem pode usar os dois comandos. Vazio: quem tem Gerenciar Servidor.
    #[serde(deserialize_with = "ids")]
    pub allowed_role_ids: Vec<u64>,
    /// Cargos pingados no post (so os que existem no servidor).
    #[serde(deserialize_with = "ids")]
    pub default_roles: Vec<u64>,
    #[serde(deserialize_with = "text")]
    pub game_link: Option<String>,
    #[serde(deserialize_with = "text")]
    pub voice_chat_link: Option<String>,
    #[serde(deserialize_with = "text")]
    pub title: Option<String>,
    #[serde(deserialize_with = "text")]
    pub voice_channel: Option<String>,
    #[serde(deserialize_with = "text")]
    pub image_url: Option<String>,
    /// Numero ou texto, como no ccore.
    #[serde(deserialize_with = "text")]
    pub place_id: Option<String>,
    /// Modelo com CODE_HERE ou {CODE}; tem prioridade sobre o placeId.
    #[serde(deserialize_with = "text")]
    pub quick_launch_link: Option<String>,
    /// Link no texto da DM (para o preview); vazio usa o gameLink.
    #[serde(deserialize_with = "text")]
    pub dm_game_link: Option<String>,
}

/// Lista de ids como numeros ou textos; o que nao for id fica de fora.
fn ids<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u64>, D::Error> {
    let one = |v: &serde_json::Value| match v {
        serde_json::Value::Number(n) => n.as_u64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    };
    Ok(match Option::<serde_json::Value>::deserialize(d)? {
        Some(serde_json::Value::Array(items)) => items.iter().filter_map(one).collect(),
        Some(v) => one(&v).into_iter().collect(),
        None => Vec::new(),
    }
    .into_iter()
    .filter(|&id| id != 0)
    .collect())
}

/// Texto (ou numero, para o placeId) sem espacos nas pontas; vazio, null ou
/// outro tipo vira None.
fn text<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let raw = match Option::<serde_json::Value>::deserialize(d)? {
        Some(serde_json::Value::String(s)) => s,
        Some(serde_json::Value::Number(n)) => n.to_string(),
        _ => return Ok(None),
    };
    let trimmed = raw.trim();
    Ok((!trimmed.is_empty()).then(|| trimmed.to_owned()))
}

impl AdminSettings {
    pub fn allows(&self, guild_id: u64) -> bool {
        guild_id != 0 && self.guild_ids.contains(&guild_id)
    }

    /// O servidor em admin.dmGuilds, com os cargos dele.
    pub fn dm_guild(&self, guild_id: u64) -> Option<&DmGuild> {
        self.dm_guilds.get(&guild_id)
    }

    /// O /dm funciona aqui: servidor do admin ou de admin.dmGuilds.
    pub fn allows_dm(&self, guild_id: u64) -> bool {
        self.allows(guild_id) || self.dm_guild(guild_id).is_some()
    }
}

impl Config {
    /// So o parse, sem carimbo nem cache. Vazio (ou `null`) e erro: aceitar
    /// isso faria uma janela de truncamento no meio de uma edicao zerar
    /// todas as chaves.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let config: Option<Self> = serde_json::from_reader(StripComments::new(text.as_bytes()))?;
        config.ok_or_else(|| "config/config.jsonc esta vazio ou contem apenas 'null'.".into())
    }
}

struct Cached {
    config: Arc<Config>,
    stamp: Option<SystemTime>,
}

/// Cache do arquivo, invalidado pela data de modificacao.
pub struct ConfigFile {
    path: PathBuf,
    cached: Mutex<Option<Cached>>,
    /// Carimbo da ultima versao que nao leu. Evita reler e logar a mesma
    /// falha a cada comando enquanto ninguem corrige o arquivo.
    failed: Mutex<Option<SystemTime>>,
}

impl ConfigFile {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            cached: Mutex::new(None),
            failed: Mutex::new(None),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Le de novo do disco. Usado na subida, onde falhar e fatal.
    pub fn load(&self) -> Result<Arc<Config>, Error> {
        let stamp = modified(&self.path);
        let text = std::fs::read_to_string(&self.path)?;
        let config = Arc::new(Config::parse(&text)?);
        // O carimbo de DEPOIS da leitura: se o arquivo mudou no meio, o pior
        // caso e recarregar uma vez a mais, e nao ficar preso numa versao.
        let post = modified(&self.path).or(stamp);
        *lock(&self.cached) = Some(Cached {
            config: config.clone(),
            stamp: post,
        });
        Ok(config)
    }

    /// O config atual. Se o arquivo mudou e a versao nova nao le (vazia no
    /// meio de uma edicao, JSON quebrado), continua valendo a anterior, com
    /// um aviso no log - o mesmo efeito do C#, onde a leitura falha e quem
    /// chamou fica com o valor que ja tinha.
    pub fn current(&self) -> Arc<Config> {
        let stamp = modified(&self.path);
        if let Some(cached) = lock(&self.cached).as_ref()
            && cached.stamp == stamp
        {
            return cached.config.clone();
        }
        if stamp.is_some() && *lock(&self.failed) == stamp {
            return self.fallback();
        }

        match self.load() {
            Ok(config) => {
                *lock(&self.failed) = None;
                config
            }
            Err(cause) => {
                tracing::warn!(
                    "[config] aviso: nao consegui reler {}; mantendo o anterior: {cause}",
                    self.path.display()
                );
                *lock(&self.failed) = stamp;
                self.fallback()
            }
        }
    }

    fn fallback(&self) -> Arc<Config> {
        lock(&self.cached)
            .as_ref()
            .map(|c| c.config.clone())
            .unwrap_or_default()
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

static FILE: LazyLock<ConfigFile> =
    LazyLock::new(|| ConfigFile::new(crate::paths::config("config.jsonc")));

/// O arquivo do processo (config/config.jsonc sob a raiz de `paths`).
pub fn file() -> &'static ConfigFile {
    &FILE
}

/// O config atual do processo. Barato: so um stat quando nada mudou.
pub fn current() -> Arc<Config> {
    FILE.current()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_jsonc_com_padroes_e_ignora_chaves_desconhecidas() {
        let config = Config::parse(
            r#"{
                // comentario
                "token": "abc",
                "guildIds": [1487938282200236224],
                "outraCoisa": 1,
                "autoSoftbanGuildId": "123",
                "music": { "maxTrackMinutes": 0, "maxFileSizeMB": 2000 },
                "gitFeed": null,
                /* bloco */
                "localAi": { "baseUrl": "http://127.0.0.1:9931/", "guildIds": null }
            }"#,
        )
        .unwrap();

        assert_eq!(config.token, "abc");
        assert_eq!(config.guild_ids, [1487938282200236224]);
        assert_eq!(config.auto_softban_guild_id, Some(123));
        assert_eq!(config.moderation_log_channel_id, None);
        assert_eq!(config.music.max_track_length(), Duration::from_secs(60));
        assert_eq!(config.music.max_file_size_megabytes(), 1024);
        assert_eq!(config.music.yt_dlp_path, "yt-dlp");
        assert_eq!(config.git_feed.branch, "main");
        assert!(!config.git_feed.is_configured());
        assert_eq!(config.local_ai.base_url(), "http://127.0.0.1:9931");
        assert_eq!(config.local_ai.cooldown_seconds, 20);
        assert!(!config.roblox_verify.is_configured());
        assert!(!format!("{config:?}").contains("abc"));
        // Sem a secao, o admin fica desligado em todo servidor.
        assert!(config.admin.guild_ids.is_empty());
        assert!(!config.admin.allows(1487938282200236224));
    }

    #[test]
    fn admin_so_nos_servidores_listados() {
        let config =
            Config::parse(r#"{ "admin": { "guildIds": [1406658194071355415], "outra": true } }"#)
                .unwrap();
        assert!(config.admin.allows(1406658194071355415));
        assert!(!config.admin.allows(1));
        assert!(!config.admin.allows(0));
        assert!(
            Config::parse(r#"{ "admin": null }"#)
                .unwrap()
                .admin
                .guild_ids
                .is_empty()
        );
        assert!(
            Config::parse(r#"{ "admin": { "guildIds": null } }"#)
                .unwrap()
                .admin
                .guild_ids
                .is_empty()
        );
    }

    #[test]
    fn deployment_com_todas_as_chaves() {
        let config = Config::parse(
            r#"{ "admin": { "deployment": {
                "channelId": "123",
                "allowedRoleIds": [1, "2", "x", 0, null],
                "defaultRoles": [3],
                "gameLink": " https://www.roblox.com/games/1/x ",
                "voiceChatLink": "https://discord.com/channels/1/2",
                "title": "Deploy :Sword:",
                "voiceChannel": "Main VC",
                "imageUrl": "https://x.test/a.png",
                "placeId": 456,
                "quickLaunchLink": "https://x.test/{CODE}",
                "dmGameLink": "https://x.test/dm",
                "outraChave": true
            } } }"#,
        )
        .unwrap();
        let d = &config.admin.deployment;
        assert_eq!(d.channel_id, Some(123));
        assert_eq!(d.allowed_role_ids, [1, 2]);
        assert_eq!(d.default_roles, [3]);
        assert_eq!(
            d.game_link.as_deref(),
            Some("https://www.roblox.com/games/1/x")
        );
        assert_eq!(d.title.as_deref(), Some("Deploy :Sword:"));
        assert_eq!(d.voice_channel.as_deref(), Some("Main VC"));
        assert_eq!(d.place_id.as_deref(), Some("456"));
        assert_eq!(
            d.quick_launch_link.as_deref(),
            Some("https://x.test/{CODE}")
        );
        assert_eq!(d.dm_game_link.as_deref(), Some("https://x.test/dm"));
        assert!(d.voice_chat_link.is_some() && d.image_url.is_some());
    }

    #[test]
    fn deployment_vazio_null_ou_tipo_errado() {
        for json in [
            r#"{}"#,
            r#"{ "admin": { "guildIds": [1] } }"#,
            r#"{ "admin": { "deployment": null } }"#,
            r#"{ "admin": { "deployment": {} } }"#,
            r#"{ "admin": { "deployment": { "channelId": null, "allowedRoleIds": null,
                "defaultRoles": "abc", "gameLink": "  ", "title": false, "placeId": [1] } } }"#,
        ] {
            let d = Config::parse(json).unwrap().admin.deployment;
            assert_eq!(d.channel_id, None, "{json}");
            assert!(d.allowed_role_ids.is_empty() && d.default_roles.is_empty());
            assert!(d.game_link.is_none() && d.title.is_none() && d.place_id.is_none());
        }
        // Um id so, fora de lista, tambem vale.
        let d = Config::parse(r#"{ "admin": { "deployment": { "defaultRoles": "77" } } }"#)
            .unwrap()
            .admin
            .deployment;
        assert_eq!(d.default_roles, [77]);
    }

    #[test]
    fn dm_guilds_com_chave_de_texto_e_valores_ruins() {
        let config = Config::parse(
            r#"{ "admin": { "guildIds": [1], "dmGuilds": {
                "1575178593556893886": { "roleIds": [1575179034588086392, "7", "x", 0, null] },
                " 20 ": { "roleIds": "9", "outra": true },
                "20": { "roleIds": [9, 10] },
                "30": {},
                "40": null,
                "50": [1, 2],
                "abc": { "roleIds": [1] },
                "0": { "roleIds": [1] },
                "60": { "roleIds": { "a": 1 } }
            } } }"#,
        )
        .unwrap();
        let admin = &config.admin;
        assert_eq!(
            admin.dm_guild(1575178593556893886).unwrap().role_ids,
            [1575179034588086392, 7]
        );
        assert_eq!(admin.dm_guild(20).unwrap().role_ids, [9, 10]);
        // Sem cargos: so o dono e Administrator.
        assert!(admin.dm_guild(30).unwrap().role_ids.is_empty());
        assert!(admin.dm_guild(60).unwrap().role_ids.is_empty());
        for ignored in [40, 50, 0] {
            assert!(admin.dm_guild(ignored).is_none(), "{ignored}");
        }
        assert_eq!(admin.dm_guilds.len(), 4);
        assert!(admin.allows_dm(1) && admin.allows_dm(20) && !admin.allows_dm(40));
        // So o /dm: o resto do admin continua so em guildIds.
        assert!(!admin.allows(20));

        for json in [
            r#"{ "admin": { "dmGuilds": null } }"#,
            r#"{ "admin": { "dmGuilds": [1, 2] } }"#,
            r#"{ "admin": { "dmGuilds": "1" } }"#,
            r#"{ "admin": {} }"#,
        ] {
            assert!(
                Config::parse(json).unwrap().admin.dm_guilds.is_empty(),
                "{json}"
            );
        }
    }

    #[test]
    fn o_exemplo_le() {
        // O exemplo e o que se copia: tem de continuar lendo.
        let config = Config::parse(include_str!("../config/config.example.jsonc")).unwrap();
        let d = &config.admin.deployment;
        assert!(d.channel_id.is_none() && d.title.is_none() && d.place_id.is_none());
    }

    #[test]
    fn vazio_ou_null_e_erro() {
        assert!(Config::parse("").is_err());
        assert!(Config::parse("null").is_err());
        assert!(Config::parse("// so comentario\nnull").is_err());
    }

    #[test]
    fn verificacao_so_com_segredo_e_https() {
        let mut v = RobloxVerifySettings {
            api_secret: Some("x".into()),
            ..Default::default()
        };
        assert!(v.is_configured());
        v.result_url = "http://daeese.me/x".into();
        assert!(!v.is_configured());
    }

    #[test]
    fn recarrega_quando_muda_e_mantem_o_anterior_quando_quebra() {
        let dir =
            std::env::temp_dir().join(format!("sollarety-config-{}", crate::store::short_id(8)));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.jsonc");
        let set = |text: &str, secs: u64| {
            std::fs::write(&path, text).unwrap();
            // O carimbo precisa mudar mesmo que a escrita caia no mesmo tick.
            let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 + secs);
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(when)
                .unwrap();
        };

        set(r#"{ "token": "a", "gitFeed": { "title": "Um" } }"#, 1);
        let file = ConfigFile::new(path.clone());
        assert_eq!(file.load().unwrap().git_feed.title, "Um");

        set(r#"{ "token": "a", "gitFeed": { "title": "Dois" } }"#, 2);
        assert_eq!(file.current().git_feed.title, "Dois");

        // Truncado no meio de uma edicao: continua o anterior.
        set("", 3);
        assert_eq!(file.current().git_feed.title, "Dois");
        assert_eq!(file.current().git_feed.title, "Dois");

        set(r#"{ "token": "a", "gitFeed": { "title": "Tres" } }"#, 4);
        assert_eq!(file.current().git_feed.title, "Tres");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
