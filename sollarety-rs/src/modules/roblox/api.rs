//! A API publica do Roblox, sem autenticacao nenhuma (RobloxApi.cs).
//!
//! A posse da conta NAO e provada aqui - isso e o OAuth, no Worker. Daqui sai
//! so o que qualquer um ve no perfil: nome, nome de exibicao, data de criacao
//! e em que grupos a pessoa esta com qual rank.

use std::{
    collections::HashMap,
    hash::Hash,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use chrono::SubsecRound;
use reqwest::StatusCode;
use serde_json::Value;
use tracing::warn;

use crate::store::time::Time;

#[derive(Clone, Debug, PartialEq)]
pub struct RobloxUser {
    pub id: i64,
    pub name: String,
    pub display_name: String,
    pub created: Option<Time>,
}

/// Participacao num grupo: o rank vai de 1 a 255; fora do grupo e 0.
#[derive(Clone, Debug, PartialEq)]
pub struct RobloxGroupRole {
    pub group_id: i64,
    pub group_name: String,
    pub rank: i32,
    pub role_name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RobloxGroup {
    pub id: i64,
    pub name: String,
}

/// Resultado de uma consulta que separa "nao existe" de "nao deu para saber".
///
/// O ccore misturava os dois: um 429 ou 5xx na busca de usuario virava
/// "conta nao encontrada". Aqui isso teria efeito pior - uma instabilidade
/// do Roblox faria o bot TIRAR cargos de bind de todo mundo que entrasse.
/// Com os tres estados separados, falha significa "nao mexa em nada".
#[derive(Clone, Debug, PartialEq)]
pub enum Lookup<T> {
    Found(T),
    Missing,
    Fail(String),
}

impl<T> Lookup<T> {
    pub fn value(&self) -> Option<&T> {
        match self {
            Lookup::Found(value) => Some(value),
            _ => None,
        }
    }

    pub fn error(&self) -> Option<&str> {
        match self {
            Lookup::Fail(error) => Some(error),
            _ => None,
        }
    }
}

/// Uma leva de entradas no servidor, ou um /update logo depois de um
/// /verify, nao precisa perguntar a mesma coisa duas vezes. Curto o bastante
/// para uma promocao no grupo aparecer no /update seguinte.
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);

/// Teto de espera num 429. Acima disto e melhor falhar e avisar.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(5);

const MAX_CACHE_ENTRIES: usize = 5000;

type Cache<T> = LazyLock<Mutex<HashMap<i64, (Instant, T)>>>;

static USERS: Cache<RobloxUser> = LazyLock::new(Mutex::default);
static GROUPS: Cache<Vec<RobloxGroupRole>> = LazyLock::new(Mutex::default);

pub fn profile_url(roblox_id: i64) -> String {
    format!("https://www.roblox.com/users/{roblox_id}/profile")
}

pub async fn get_user(http: &reqwest::Client, roblox_id: i64, fresh: bool) -> Lookup<RobloxUser> {
    if !fresh && let Some(hit) = recall(&USERS, roblox_id) {
        return Lookup::Found(hit);
    }

    let json = match get_json(
        http,
        &format!("https://users.roblox.com/v1/users/{roblox_id}"),
    )
    .await
    {
        Fetch::Json(json) => json,
        Fetch::Missing => return Lookup::Missing,
        Fetch::Fail(error) => return Lookup::Fail(error),
    };

    let user = parse_user(roblox_id, &json);
    remember(&USERS, roblox_id, user.clone());
    Lookup::Found(user)
}

pub async fn get_group_roles(
    http: &reqwest::Client,
    roblox_id: i64,
    fresh: bool,
) -> Lookup<Vec<RobloxGroupRole>> {
    if !fresh && let Some(hit) = recall(&GROUPS, roblox_id) {
        return Lookup::Found(hit);
    }

    // v2 e a versao enxuta: o v1 devolve dono, descricao e mural de cada
    // grupo, e quem esta em cinquenta grupos baixa tudo isso a toa.
    let url = format!("https://groups.roblox.com/v2/users/{roblox_id}/groups/roles");
    let json = match get_json(http, &url).await {
        Fetch::Json(json) => json,
        Fetch::Missing => return Lookup::Missing,
        Fetch::Fail(error) => return Lookup::Fail(error),
    };

    let roles = parse_group_roles(&json);
    remember(&GROUPS, roblox_id, roles.clone());
    Lookup::Found(roles)
}

pub async fn get_group(http: &reqwest::Client, group_id: i64) -> Lookup<RobloxGroup> {
    let url = format!("https://groups.roblox.com/v1/groups/{group_id}");
    match get_json(http, &url).await {
        Fetch::Json(json) => Lookup::Found(RobloxGroup {
            id: group_id,
            name: value_string(&json, "name").unwrap_or_else(|| format!("group {group_id}")),
        }),
        Fetch::Missing => Lookup::Missing,
        Fetch::Fail(error) => Lookup::Fail(error),
    }
}

/// Esquece o que se sabe de um usuario. Chamado no /update.
pub fn forget(roblox_id: i64) {
    lock(&USERS).remove(&roblox_id);
    lock(&GROUPS).remove(&roblox_id);
}

fn parse_user(roblox_id: i64, json: &Value) -> RobloxUser {
    let name = value_string(json, "name");
    RobloxUser {
        id: roblox_id,
        display_name: value_string(json, "displayName")
            .or_else(|| name.clone())
            .unwrap_or_default(),
        name: name.unwrap_or_default(),
        created: value_string(json, "created").and_then(|c| parse_date(&c)),
    }
}

fn parse_group_roles(json: &Value) -> Vec<RobloxGroupRole> {
    let Some(entries) = json.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };

    entries
        .iter()
        .filter(|entry| entry.is_object())
        .map(|entry| {
            let group = entry.get("group").filter(|g| g.is_object());
            let role = entry.get("role").filter(|r| r.is_object());
            RobloxGroupRole {
                group_id: group.and_then(|g| value_i64(g, "id")).unwrap_or(0),
                group_name: group
                    .and_then(|g| value_string(g, "name"))
                    .unwrap_or_default(),
                rank: role
                    .and_then(|r| value_i64(r, "rank"))
                    .and_then(|r| i32::try_from(r).ok())
                    .unwrap_or(0),
                role_name: role
                    .and_then(|r| value_string(r, "name"))
                    .unwrap_or_default(),
            }
        })
        .filter(|r| r.group_id > 0)
        .collect()
}

/// A data do Roblox ("2006-02-27T21:06:40.3Z"), cortada em ticks de 100 ns
/// como o DateTimeOffset guardaria.
fn parse_date(value: &str) -> Option<Time> {
    crate::store::time::parse(value)
        .ok()
        .map(|t| t.trunc_subsecs(7))
}

/// O `Value<string>` do Newtonsoft: texto vem como esta, numero e booleano
/// viram texto, null e ausencia viram None.
pub(super) fn value_string(json: &Value, key: &str) -> Option<String> {
    match json.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(true) => Some("True".into()),
        Value::Bool(false) => Some("False".into()),
        _ => None,
    }
}

/// O `Value<long?>` do Newtonsoft: aceita numero e texto numerico.
pub(super) fn value_i64(json: &Value, key: &str) -> Option<i64> {
    match json.get(key)? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.round() as i64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

enum Fetch {
    Json(Value),
    /// "Nao existe": 404 no users e 400 no groups.
    Missing,
    Fail(String),
}

/// GET com uma nova tentativa em 429, respeitando o Retry-After.
///
/// Devolve Missing para "nao existe": 404 no users e 400 no groups ("Group is
/// invalid or does not exist") - os dois endpoints dizem a mesma coisa com
/// codigos diferentes, e toda requisicao daqui e montada pelo bot, entao um
/// 400 nao e pedido malformado.
async fn get_json(http: &reqwest::Client, url: &str) -> Fetch {
    let mut attempt = 0;
    loop {
        let response = match http.get(url).send().await {
            Ok(response) => response,
            Err(cause) => return transport_failure(url, &cause),
        };

        let status = response.status();
        if status == StatusCode::NOT_FOUND || status == StatusCode::BAD_REQUEST {
            return Fetch::Missing;
        }

        if status == StatusCode::TOO_MANY_REQUESTS && attempt == 0 {
            let wait = retry_after(response.headers()).unwrap_or(Duration::from_secs(1));
            if wait <= MAX_RETRY_AFTER {
                tokio::time::sleep(wait).await;
                attempt += 1;
                continue;
            }
        }

        if !status.is_success() {
            warn!("[roblox] aviso: {url} respondeu {}", status.as_u16());
            return Fetch::Fail(format!("Roblox responded with {}", status.as_u16()));
        }

        let body = match response.text().await {
            Ok(body) => body,
            Err(cause) => return transport_failure(url, &cause),
        };

        // So objeto: o JObject.Load do C# recusava qualquer outra coisa.
        return match serde_json::from_str::<Value>(&body) {
            Ok(json) if json.is_object() => Fetch::Json(json),
            Ok(_) => {
                warn!("[roblox] aviso: {url} falhou: a resposta nao e um objeto JSON");
                Fetch::Fail("Roblox couldn't be reached".into())
            }
            Err(cause) => {
                warn!("[roblox] aviso: {url} falhou: {cause}");
                Fetch::Fail("Roblox couldn't be reached".into())
            }
        };
    }
}

fn transport_failure(url: &str, cause: &reqwest::Error) -> Fetch {
    if cause.is_timeout() {
        warn!("[roblox] aviso: {url} demorou demais");
        Fetch::Fail("Roblox took too long to respond".into())
    } else {
        warn!("[roblox] aviso: {url} falhou: {cause}");
        Fetch::Fail("Roblox couldn't be reached".into())
    }
}

/// So a forma em segundos, como o `RetryAfter.Delta` do C#: a forma de data
/// fica None e quem chama espera um segundo.
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

fn lock<K, V>(cache: &Mutex<HashMap<K, V>>) -> std::sync::MutexGuard<'_, HashMap<K, V>> {
    cache.lock().unwrap_or_else(|e| e.into_inner())
}

fn recall<T: Clone>(cache: &Mutex<HashMap<i64, (Instant, T)>>, key: i64) -> Option<T> {
    lock(cache)
        .get(&key)
        .filter(|(expires, _)| *expires > Instant::now())
        .map(|(_, value)| value.clone())
}

fn remember<T>(cache: &Mutex<HashMap<i64, (Instant, T)>>, key: i64, value: T) {
    let now = Instant::now();
    let mut map = lock(cache);
    prune(&mut map, now);
    map.insert(key, (now + CACHE_TTL, value));
}

/// Poda grosseira: o cache so existe para absorver rajada, e passar do teto
/// e sinal de que ele esta guardando gente que nao volta.
fn prune<K: Eq + Hash, T>(map: &mut HashMap<K, (Instant, T)>, now: Instant) {
    if map.len() > MAX_CACHE_ENTRIES {
        map.retain(|_, (expires, _)| *expires > now);
        if map.len() > MAX_CACHE_ENTRIES {
            map.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn usuario_sem_exibicao_usa_o_nome_e_le_a_data() {
        let user = parse_user(
            1,
            &json!({"name": "builderman", "created": "2006-02-27T21:06:40.3Z"}),
        );
        assert_eq!(user.name, "builderman");
        assert_eq!(user.display_name, "builderman");
        assert_eq!(
            crate::store::time::format(&user.created.unwrap()),
            "2006-02-27T21:06:40.3+00:00"
        );

        let user = parse_user(1, &json!({"name": "a", "displayName": "B", "created": "x"}));
        assert_eq!(user.display_name, "B");
        assert_eq!(user.created, None);
    }

    #[test]
    fn grupos_ignoram_entrada_sem_id() {
        let roles = parse_group_roles(&json!({"data": [
            {"group": {"id": 7, "name": "G"}, "role": {"rank": 200, "name": "Admin"}},
            {"group": {"name": "sem id"}, "role": {"rank": 1}},
            "lixo",
            {"group": {"id": 9}}
        ]}));
        assert_eq!(roles.len(), 2);
        assert_eq!(roles[0].group_id, 7);
        assert_eq!(roles[0].rank, 200);
        assert_eq!(roles[1].rank, 0);
        assert!(parse_group_roles(&json!({})).is_empty());
    }

    #[test]
    fn valores_como_o_newtonsoft() {
        let j = json!({"a": 5, "b": "6", "c": null, "d": true});
        assert_eq!(value_string(&j, "a").as_deref(), Some("5"));
        assert_eq!(value_string(&j, "c"), None);
        assert_eq!(value_string(&j, "d").as_deref(), Some("True"));
        assert_eq!(value_i64(&j, "b"), Some(6));
        assert_eq!(value_i64(&j, "x"), None);
    }

    #[test]
    fn retry_after_so_em_segundos() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(reqwest::header::RETRY_AFTER, "3".parse().unwrap());
        assert_eq!(retry_after(&h), Some(Duration::from_secs(3)));
        h.insert(
            reqwest::header::RETRY_AFTER,
            "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(retry_after(&h), None);
    }

    #[test]
    fn cache_expira_e_poda() {
        let cache: Mutex<HashMap<i64, (Instant, u8)>> = Mutex::default();
        remember(&cache, 1, 9);
        assert_eq!(recall(&cache, 1), Some(9));
        lock(&cache).insert(2, (Instant::now() - Duration::from_secs(1), 3));
        assert_eq!(recall(&cache, 2), None);

        let now = Instant::now();
        let mut map: HashMap<i64, (Instant, u8)> = (0..=MAX_CACHE_ENTRIES as i64)
            .map(|k| (k, (now - Duration::from_secs(1), 0)))
            .collect();
        map.insert(-1, (now + CACHE_TTL, 1));
        prune(&mut map, now);
        assert_eq!(map.len(), 1);
    }
}
