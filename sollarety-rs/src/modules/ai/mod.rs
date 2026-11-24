//! IA local: a resposta quando mencionado (LocalAi), a trava de spam e
//! ordens (AiGuard), /translate, /quiz e os botoes "quiz:".
//!
//! A portar do C# (CommunityBot/):
//! - commands/Ai.cs
//! - Services/LocalAi.cs
//! - Services/AiGuard.cs
//! - Services/Ai/AiQuiz.cs
//! - Services/Ai/AiQuizComponents.cs
//! - Services/Ai/QuizSchema.cs
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
    // Desligamento: nada no C#.
}
