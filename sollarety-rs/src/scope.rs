//! Onde cada comando aparece (allowedContexts/integrationTypes do C#) e onde
//! um comando "de qualquer lugar" esta rodando (CommandScope.cs).
//!
//! No C#, 19 comandos funcionam em qualquer lugar (servidor, DM com o bot,
//! DM entre pessoas, grupo e servidor onde so a pessoa instalou o app) e o
//! resto so em servidor. Dois jeitos de marcar, no proprio comando:
//!
//! ```ignore
//! #[poise::command(slash_command,
//!     install_context = "Guild|User",
//!     interaction_context = "Guild|BotDm|PrivateChannel")]
//! #[poise::command(slash_command, interaction_context = "Guild")]
//! ```
//!
//! ou na lista do `commands()` do modulo: `scope::anywhere(cmd())` e
//! `scope::guild_only(cmd())`.

use poise::serenity_prelude as serenity;

use crate::{Context, Data, Error};

pub type Command = poise::Command<Data, Error>;

/// Funciona tambem como app pessoal: instalado no servidor ou no usuario, e
/// usavel em servidor, DM com o bot e DM/grupo entre pessoas.
pub fn anywhere(mut command: Command) -> Command {
    command.install_context = Some(vec![
        serenity::InstallationContext::Guild,
        serenity::InstallationContext::User,
    ]);
    command.interaction_context = Some(vec![
        serenity::InteractionContext::Guild,
        serenity::InteractionContext::BotDm,
        serenity::InteractionContext::PrivateChannel,
    ]);
    command
}

/// So dentro de servidor: some da DM com o bot, onde apareceria so para
/// recusar.
pub fn guild_only(mut command: Command) -> Command {
    command.interaction_context = Some(vec![serenity::InteractionContext::Guild]);
    command
}

/// O servidor, se o BOT estiver nele; None em DM e em servidor onde so a
/// pessoa instalou o app. La o Discord manda o id do servidor, mas o bot nao
/// ve membros, cargos nem configuracao - so o cache dele diz isso.
pub fn bot_guild(ctx: Context<'_>) -> Option<serenity::GuildId> {
    let guild_id = ctx.guild_id()?;
    ctx.serenity_context()
        .cache
        .guild(guild_id)
        .is_some()
        .then_some(guild_id)
}
