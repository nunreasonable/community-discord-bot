//! Verificacao Roblox: /verify, /unverify, /update, /whois, /verify-panel,
//! os botoes "verify:", os binds de grupo (/bind) e a entrada de membro
//! (cargos e apelido de quem ja e vinculado).
//!
//! A portar do C# (CommunityBot/):
//! - commands/Verification.cs
//! - commands/Binds.cs
//! - Services/Roblox/RobloxApi.cs
//! - Services/Roblox/VerificationFlow.cs
//! - Services/Roblox/VerificationService.cs
//! - Services/Roblox/VerifyWorkerClient.cs
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
    // Desligamento: nada a esperar no C#; parar o polling pendente se houver.
}
