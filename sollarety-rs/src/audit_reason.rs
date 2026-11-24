//! Texto que vai para o Audit Log do Discord (AuditReason.cs).
//!
//! O valor viaja num CABECALHO HTTP (X-Audit-Log-Reason): uma quebra de linha
//! no meio derruba a requisicao inteira, e a acao falha ANTES de acontecer
//! por causa de um texto que alguem colou. Por isso a limpeza mora num lugar
//! so.

use crate::embeds::trim;

/// Limite do cabecalho, com folga para o prefixo de quem executou.
const MAX_LENGTH: usize = 400;

/// Motivo carregando quem mandou fazer: o registro nativo continua util
/// mesmo se o canal de log for apagado.
pub fn for_actor(actor_tag: &str, reason: Option<&str>) -> String {
    trim(&format!("{actor_tag}: {}", clean(reason)), MAX_LENGTH)
}

/// Motivo sem autor, para acao que o proprio bot decidiu.
pub fn automatic(reason: &str) -> String {
    trim(&clean(Some(reason)), MAX_LENGTH)
}

/// Todo caractere de controle vira espaco.
fn clean(reason: Option<&str>) -> String {
    reason
        .unwrap_or("no reason given")
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tira_controle_e_corta() {
        assert_eq!(for_actor("mod#0", Some("a\nb\tc")), "mod#0: a b c");
        assert_eq!(for_actor("mod#0", None), "mod#0: no reason given");
        assert_eq!(automatic("x\r\n").as_str(), "x  ");
        assert_eq!(crate::embeds::utf16_len(&automatic(&"y".repeat(1000))), 400);
    }
}
