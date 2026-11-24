//! Busca no Worker o resultado de uma verificacao (VerifyWorkerClient.cs).
//!
//! O bot nao tem porta aberta para a internet, entao quem termina o OAuth
//! (o Worker) nao consegue avisar o bot. O caminho e o inverso: o Worker
//! guarda o resultado por meia hora, indexado pelo id do Discord que o
//! navegador PROVOU pelo OAuth do Discord, e o bot vem buscar. A leitura
//! consome: o mesmo resultado nao e entregue duas vezes.
//!
//! O contrato (URL, cabecalho, JSON) e o mesmo do bot em C#: o Worker e um
//! so para os dois.

use reqwest::StatusCode;
use serde_json::Value;
use tracing::{error, warn};

use super::api::{value_i64, value_string};
use crate::{config::RobloxVerifySettings, store::time::Time};

/// O que o Worker devolve depois dos dois OAuth.
#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedAccount {
    pub discord_id: u64,
    pub roblox_id: i64,
    pub name: String,
    pub display_name: String,
    pub created: Option<Time>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Take {
    Pending,
    Found(VerifiedAccount),
    Failed(String),
}

pub async fn take(
    http: &reqwest::Client,
    settings: &RobloxVerifySettings,
    discord_id: u64,
) -> Take {
    if !settings.is_configured() {
        return Take::Failed("verification is not configured".into());
    }

    let secret = settings.api_secret.as_deref().unwrap_or_default();
    let url = format!("{}?d={discord_id}", settings.result_url);

    // O segredo vai no cabecalho: um IP literal no config nao passa pelo
    // resolver que barra endereco interno, entao a URL e conferida antes.
    if !url::Url::parse(&url).is_ok_and(|u| crate::http::is_allowed_url(&u)) {
        warn!("[roblox] aviso: falha ao consultar o Worker: endereco recusado");
        return Take::Failed("I couldn't reach the verification server".into());
    }

    let response = match http.get(&url).bearer_auth(secret).send().await {
        Ok(response) => response,
        Err(cause) => return transport_failure(cause),
    };

    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Take::Pending;
    }

    if status == StatusCode::UNAUTHORIZED {
        // Nao e falha passageira: o segredo daqui e o do Worker nao batem.
        // Vai para o log como erro para aparecer no /logs.
        error!(
            "[roblox] erro: o Worker recusou o apiSecret (401). Confira robloxVerify.apiSecret contra o BOT_API_SECRET do Worker."
        );
        return Take::Failed("the verification server rejected the bot's credentials".into());
    }

    if !status.is_success() {
        return Take::Failed(format!(
            "the verification server responded with {}",
            status.as_u16()
        ));
    }

    let body = match response.text().await {
        Ok(body) => body,
        Err(cause) => return transport_failure(cause),
    };

    match parse(&body, discord_id) {
        Ok(take) => take,
        Err(cause) => {
            warn!("[roblox] aviso: falha ao consultar o Worker: {cause}");
            Take::Failed("I couldn't reach the verification server".into())
        }
    }
}

fn transport_failure(cause: reqwest::Error) -> Take {
    if cause.is_timeout() {
        return Take::Failed("the verification server took too long to respond".into());
    }
    // Sem a URL: ela e do config, mas o erro do reqwest a repetiria inteira.
    warn!(
        "[roblox] aviso: falha ao consultar o Worker: {}",
        cause.without_url()
    );
    Take::Failed("I couldn't reach the verification server".into())
}

/// Le a resposta do Worker. Err e JSON quebrado (o JsonException do C#);
/// resposta valida mas sem sentido vira `Take::Failed`.
fn parse(body: &str, discord_id: u64) -> Result<Take, String> {
    let json: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    if !json.is_object() {
        return Err("a resposta nao e um objeto JSON".into());
    }

    // Os ids vem como string: o id do Discord nao cabe num double, e e isso
    // que o JavaScript do Worker tem como numero.
    let returned_id = value_string(&json, "discordId").and_then(|s| digits::<u64>(&s));
    let roblox_id = value_string(&json, "robloxId")
        .and_then(|s| digits::<i64>(&s))
        .filter(|&id| id > 0);
    let (Some(returned_id), Some(roblox_id)) = (returned_id, roblox_id) else {
        return Ok(Take::Failed(
            "the verification server returned an invalid response".into(),
        ));
    };

    // O Worker indexa pelo id pedido, entao isto so falharia com um Worker
    // quebrado. Custa uma comparacao e fecha a porta de vez.
    if returned_id != discord_id {
        return Ok(Take::Failed(
            "the verification result belonged to a different Discord account".into(),
        ));
    }

    let created = value_i64(&json, "createdAt")
        .filter(|&unix| unix > 0)
        .and_then(|unix| chrono::DateTime::from_timestamp(unix, 0));
    let name = value_string(&json, "name");

    Ok(Take::Found(VerifiedAccount {
        discord_id: returned_id,
        roblox_id,
        display_name: value_string(&json, "displayName")
            .or_else(|| name.clone())
            .unwrap_or_default(),
        name: name.unwrap_or_default(),
        created,
    }))
}

/// `TryParse` com `NumberStyles.None`: so digitos, sem sinal nem espaco.
fn digits<T: std::str::FromStr>(text: &str) -> Option<T> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resultado_valido() {
        let take = parse(
            r#"{"discordId":"123456789012345678","robloxId":"42","name":"bob","displayName":"Bob","createdAt":1700000000}"#,
            123456789012345678,
        )
        .unwrap();
        let Take::Found(account) = take else {
            panic!("esperava Found: {take:?}");
        };
        assert_eq!(account.roblox_id, 42);
        assert_eq!(account.display_name, "Bob");
        assert_eq!(account.created.unwrap().timestamp(), 1700000000);
    }

    #[test]
    fn sem_exibicao_nem_data() {
        let Take::Found(account) = parse(
            r#"{"discordId":"5","robloxId":"6","name":"x","createdAt":0}"#,
            5,
        )
        .unwrap() else {
            panic!();
        };
        assert_eq!(account.display_name, "x");
        assert_eq!(account.created, None);
    }

    #[test]
    fn recusa_id_estranho_ou_de_outra_pessoa() {
        let invalid = Take::Failed("the verification server returned an invalid response".into());
        assert_eq!(
            parse(r#"{"discordId":"5","robloxId":"0"}"#, 5).unwrap(),
            invalid
        );
        assert_eq!(
            parse(r#"{"discordId":" 5","robloxId":"6"}"#, 5).unwrap(),
            invalid
        );
        assert_eq!(
            parse(r#"{"discordId":"5","robloxId":"-6"}"#, 5).unwrap(),
            invalid
        );
        assert_eq!(parse(r#"{"robloxId":"6"}"#, 5).unwrap(), invalid);
        assert!(matches!(
            parse(r#"{"discordId":"7","robloxId":"6"}"#, 5).unwrap(),
            Take::Failed(e) if e.contains("different Discord account")
        ));
        assert!(parse("nao e json", 5).is_err());
        assert!(parse("[]", 5).is_err());
    }
}
