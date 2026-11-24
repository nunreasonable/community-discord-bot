//! Softban: banir e desbanir na sequencia (Softban.cs).
//!
//! O efeito util e o do MEIO. O ban do Discord aceita apagar o historico
//! recente do alvo em todos os canais de uma vez - coisa que nem o /kick nem
//! o /purge fazem - e o unban logo depois devolve a pessoa a condicao de quem
//! so foi expulso: ela pode voltar por convite. Feito a mao sao dois passos,
//! e esquecer o segundo transforma uma limpeza num banimento permanente.
//!
//! Mora aqui, e nao dentro do comando, porque o vigia de canal precisa
//! exatamente do mesmo caminho e nao tem interacao nenhuma nas maos.

use std::time::Duration;

use poise::serenity_prelude as serenity;

/// Sete dias, o teto que o Discord permite. O serenity recebe DIAS e manda
/// `delete_message_seconds` (dias * 86400) para a API - o C# carregava o
/// comentario do dia em que passar "7" cru apagou 7 segundos de mensagens.
/// Apagar o historico e o proposito inteiro do softban: nao ha valor menor
/// que faca sentido oferecer como opcao.
pub const DELETE_MESSAGE_DAYS: u8 = 7;

const UNBAN_ATTEMPTS: u32 = 3;
const UNBAN_RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Banido e desbanido. Fim.
    Ok,
    /// O ban nao passou; nada aconteceu com o alvo.
    BanFailed(String),
    /// O ban passou e o unban NAO. O alvo ficou banido.
    UnbanFailed(String),
}

/// Aplica o softban. Nunca falha: o resultado diz o que sobrou de pe.
///
/// Os dois modos de falha PRECISAM ser distinguiveis. Se o ban falha, o
/// servidor ficou como estava. Se o unban falha, a pessoa esta banida de
/// verdade - o oposto do que o comando prometeu - e quem chamou tem de dizer
/// isso em voz alta, para alguem desfazer a mao.
pub async fn apply(
    http: &serenity::Http,
    guild_id: serenity::GuildId,
    user_id: serenity::UserId,
    audit_reason: &str,
) -> Outcome {
    if let Err(cause) = guild_id
        .ban_with_reason(http, user_id, DELETE_MESSAGE_DAYS, audit_reason)
        .await
    {
        return Outcome::BanFailed(cause.to_string());
    }

    // O unban INSISTE, e o ban nao. Se o unban nao passa, existe agora um
    // banimento permanente que ninguem pediu. Tres tentativas com espera
    // crescente cobrem o 5xx passageiro e o intervalo em que o proprio
    // serenity esta segurando um 429. Nao e garantia: se todas falharem, o
    // resultado diz isso.
    let mut last = String::new();
    for attempt in 1..=UNBAN_ATTEMPTS {
        match http.remove_ban(guild_id, user_id, Some(audit_reason)).await {
            Ok(()) => return Outcome::Ok,
            Err(cause) => {
                last = cause.to_string();
                if attempt < UNBAN_ATTEMPTS {
                    tokio::time::sleep(UNBAN_RETRY_DELAY * attempt).await;
                }
            }
        }
    }
    Outcome::UnbanFailed(last)
}
