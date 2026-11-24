//! Comunidade: niveis (/rank, /level-dms, /xp), economia (/daily, /work,
//! /pay, /balance), /profile, /leaderboard, os fun (/8ball, /roll, /coinflip,
//! /choose, /avatar, /ship, /rate, /cancel, /say), jogos (/rps, /tictactoe),
//! /roleplay, /text e /morse, e os botoes "lvl:", "eco:", "rp:" e "ttt:".
//!
//! A portar do C# (CommunityBot/):
//! - commands/Levels.cs
//! - commands/Economy.cs
//! - commands/Profile.cs
//! - commands/Fun.cs
//! - commands/Games.cs
//! - commands/Roleplay.cs
//! - commands/Text.cs
//! - Services/Levels/LevelService.cs
//! - Services/Levels/LevelMath.cs
//! - Services/Economy/EconomyService.cs
//! - Services/Economy/EconomyComponents.cs
//! - Services/Fun/FunComponents.cs
//! - Services/Fun/NekosBest.cs
//! - Services/Fun/RoleplayActions.cs
//! - Services/Fun/StableHash.cs
//! - Services/Fun/TextTransforms.cs
//! - Services/Fun/TicTacToe.cs
//! - Services/DiceRoller.cs
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
    // Desligamento: nada; o flush final do XP ja e feito pelo main.
}
