//! IA local: a resposta quando mencionado (LocalAi), a trava de spam e
//! ordens (AiGuard), /translate, /quiz e os botoes "quiz:".
//!
//! Portado do C# (CommunityBot/):
//! - commands/Ai.cs -> commands.rs
//! - Services/LocalAi.cs -> local.rs
//! - Services/AiGuard.cs -> guard.rs
//! - Services/Ai/AiQuiz.cs -> quiz.rs
//! - Services/Ai/AiQuizComponents.cs -> quiz_components.rs
//! - Services/Ai/QuizSchema.cs -> quiz_schema.rs
//!
//! Tudo deste modulo fica DENTRO deste diretorio. Estado proprio vai num
//! `static` (LazyLock/OnceLock) daqui, e nao em campo novo no `Data`.

mod commands;
mod guard;
mod local;
mod quiz;
mod quiz_components;
mod quiz_schema;

use poise::serenity_prelude as serenity;

use crate::{Data, Error, scope};

/// Os comandos do modulo. O main junta todos e registra.
pub fn commands() -> Vec<poise::Command<Data, Error>> {
    vec![
        scope::anywhere(commands::translate()),
        scope::guild_only(commands::quiz()),
    ]
}

/// Recebe TODO evento do gateway. Interacao de componente que nao comeca com
/// o prefixo deste modulo ("quiz:") nao e daqui: ignora e devolve Ok.
/// Trabalho longo (chamada ao modelo local, o /quiz tocando) vai num
/// `tokio::spawn` dentro de `local` e `quiz_components`, para nao segurar o
/// loop de eventos do serenity nem os outros modulos.
pub async fn on_event(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    data: &Data,
) -> Result<(), Error> {
    match event {
        serenity::FullEvent::Message { new_message } => {
            local::on_message_create(ctx, data, new_message);
        }
        serenity::FullEvent::InteractionCreate {
            interaction: serenity::Interaction::Component(component),
        } if component.data.custom_id.starts_with(quiz::PREFIX) => {
            quiz_components::on_component(ctx, component).await?;
        }
        _ => {}
    }
    Ok(())
}

/// Chamado uma vez no desligamento, antes de largar o gateway.
pub async fn shutdown(_data: &Data) {
    // Desligamento: nada no C#. Um /quiz ou uma resposta em voo ficam pela
    // metade, igual ao comportamento de hoje (o processo so cai).
}
