//! Os arquivos de data/, compativeis byte a byte com o que o bot em C#
//! (Newtonsoft, Formatting.Indented) escreve: mesmas chaves, mesma ordem de
//! campos, null escrito como null, datas no formato do DateTimeOffset.
//!
//! Cada store e um Arc dentro do `Data`. Os mutadores sao SINCRONOS de
//! proposito, como no C#: nada de chamar a API do Discord com o arquivo
//! travado.

pub mod economy;
pub mod git_feed;
pub mod guild_settings;
pub mod json_file;
pub mod levels;
pub mod roblox_links;
pub mod tickets;
pub mod time;
pub mod warnings;

pub use economy::{EconomyStore, Wallet};
pub use guild_settings::{GroupBind, GuildSettings, GuildSettingsStore};
pub use levels::LevelStore;
pub use roblox_links::{RobloxLink, RobloxLinkStore};
pub use tickets::{Ticket, TicketStore};
pub use warnings::{Warning, WarningStore};

use rand::Rng;
use serde::{Deserialize, Deserializer};

/// O Newtonsoft aceita `null` numa lista ou texto e deixa o campo nulo; o C#
/// depois trata como vazio (`?? new()`). Aqui o null vira o padrao direto.
pub(crate) fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// Id curto em hexa minusculo, como `Guid.NewGuid().ToString("N")[..n]`.
pub fn short_id(len: usize) -> String {
    let mut rng = rand::rng();
    (0..len)
        .map(|_| char::from_digit(rng.random_range(0..16), 16).unwrap_or('0'))
        .collect()
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};

    /// Onde estao os arquivos de verdade do bot em C#. Os testes so leem
    /// COPIAS: nada aqui escreve na pasta original.
    fn real_data_dir() -> PathBuf {
        std::env::var_os("SOLLARETY_REAL_DATA").map_or_else(
            || {
                PathBuf::from(
                    "/home/daeese/daeesewrkspc/community-discord-bot/CommunityBot/bin/Release/net10.0/data",
                )
            },
            PathBuf::from,
        )
    }

    pub fn scratch_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sollarety-test-{name}-{}", super::short_id(8)));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Copia o arquivo real para `dir`. None (e o teste passa sem olhar
    /// nada) numa maquina que nao tem os dados: o repositorio e publico e os
    /// arquivos reais nao vao para ele.
    pub fn copy_real(dir: &Path, name: &str) -> Option<PathBuf> {
        let source = real_data_dir().join(name);
        if !source.exists() {
            eprintln!("sem {} nesta maquina; pulando", source.display());
            let _ = std::fs::remove_dir(dir);
            return None;
        }
        let target = dir.join(name);
        std::fs::copy(&source, &target).unwrap();
        Some(target)
    }
}
