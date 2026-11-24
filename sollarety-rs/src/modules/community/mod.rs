//! Comunidade: niveis (/rank, /level-dms, /xp), economia (/daily, /work,
//! /pay, /balance), /profile, /leaderboard, os fun (/8ball, /roll, /coinflip,
//! /choose, /avatar, /ship, /rate, /cancel, /say), jogos (/rps, /tictactoe),
//! /roleplay, /text e /morse, e os botoes "lvl:", "eco:", "rp:" e "ttt:".
//!
//! Portado do C# (CommunityBot/):
//! - commands/Levels.cs, Economy.cs, Profile.cs, Fun.cs, Games.cs,
//!   Roleplay.cs, Text.cs
//! - Services/Levels/LevelService.cs, LevelMath.cs
//! - Services/Economy/EconomyService.cs, EconomyComponents.cs
//! - Services/Fun/FunComponents.cs, NekosBest.cs, RoleplayActions.cs,
//!   StableHash.cs, TextTransforms.cs, TicTacToe.cs
//! - Services/DiceRoller.cs
//!
//! Tudo deste modulo fica DENTRO deste diretorio. Estado proprio vai num
//! `static` (LazyLock/OnceLock) daqui, e nao em campo novo no `Data`.

mod common;
mod dice;
mod economy;
mod fun;
mod games;
mod legacy_random;
mod levels;
mod profile;
mod roleplay;
mod sha256;
mod text;
mod text_transforms;

use poise::serenity_prelude as serenity;

use crate::{Data, Error};

/// Os comandos do modulo. O main junta todos e registra.
pub fn commands() -> Vec<poise::Command<Data, Error>> {
    let mut all = Vec::new();
    all.extend(levels::commands());
    all.extend(economy::commands());
    all.extend(profile::commands());
    all.extend(fun::commands());
    all.extend(games::commands());
    all.extend(roleplay::commands());
    all.extend(text::commands());
    all
}

/// Recebe TODO evento do gateway. Interacao de componente que nao comeca com
/// o prefixo deste modulo nao e daqui: ignore e devolva Ok. Trabalho longo
/// vai num `tokio::spawn`, para nao segurar os outros modulos.
pub async fn on_event(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    data: &Data,
) -> Result<(), Error> {
    match event {
        serenity::FullEvent::Message { new_message } => {
            levels::on_message(ctx, new_message, data).await?;
        }
        serenity::FullEvent::InteractionCreate {
            interaction: serenity::Interaction::Component(press),
        } => {
            let id = press.data.custom_id.as_str();
            if id.starts_with("lvl:") {
                levels::on_component(ctx, press).await?;
            } else if id.starts_with("eco:") {
                economy::on_component(ctx, data, press).await?;
            } else if id.starts_with("ttt:") {
                games::on_component(ctx, press).await?;
            } else if id.starts_with(roleplay::PREFIX) {
                roleplay::on_component(ctx, data, press).await?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Chamado uma vez no desligamento, antes de largar o gateway.
pub async fn shutdown(_data: &Data) {
    // Desligamento: nada; o flush final do XP ja e feito pelo main.
}
