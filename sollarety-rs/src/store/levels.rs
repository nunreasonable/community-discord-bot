//! XP de cada pessoa em cada servidor, e a preferencia de DM de level up
//! (LevelStore.cs). data/levels.json.
//!
//! Diferente dos outros stores, a MEMORIA e a fonte da verdade e o disco
//! recebe uma copia a cada 30 segundos: XP muda a cada mensagem, e reler e
//! regravar o arquivo inteiro por mensagem nao faz sentido. Um crash perde no
//! maximo 30 segundos de XP; o desligamento normal grava tudo (`flush` no fim
//! do main).

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::{
    json_file, null_default,
    time::{self, Time},
};
use crate::Error;

const TAG: &str = "niveis";
const FLUSH_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LevelFile {
    /// servidor -> pessoa -> XP total naquele servidor. IndexMap para o
    /// arquivo voltar ao disco na ordem em que estava.
    #[serde(deserialize_with = "null_default")]
    pub guilds: IndexMap<u64, IndexMap<u64, i64>>,
    /// So quem mudou algo do padrao aparece aqui.
    #[serde(deserialize_with = "null_default")]
    pub users: IndexMap<u64, UserPrefs>,
    #[serde(with = "time::option")]
    pub last_updated_utc: Option<Time>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UserPrefs {
    pub level_up_dms: bool,
}

impl Default for UserPrefs {
    fn default() -> Self {
        Self { level_up_dms: true }
    }
}

struct State {
    file: LevelFile,
    dirty: bool,
}

pub struct LevelStore {
    path: PathBuf,
    state: Mutex<State>,
    /// Uma gravacao por vez; a memoria continua mudando enquanto ela corre.
    write_lock: tokio::sync::Mutex<()>,
}

impl LevelStore {
    /// Carrega. Arquivo corrompido derruba a subida de proposito: subir com
    /// "ninguem tem XP" e o proximo flush apagariam tudo.
    pub async fn open(path: PathBuf) -> Result<Self, Error> {
        json_file::delete_leftover_temp(&path, TAG);
        let file: LevelFile = json_file::read(&path, TAG).await?;
        let members: usize = file.guilds.values().map(IndexMap::len).sum();
        tracing::info!(
            "[niveis] carregado: XP de {members} membro(s) em {} servidor(es)",
            file.guilds.len()
        );
        Ok(Self {
            path,
            state: Mutex::new(State { file, dirty: false }),
            write_lock: tokio::sync::Mutex::new(()),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Grava a cada 30 segundos, se houve mudanca. Chamado uma vez no main.
    pub fn spawn_flush_timer(self: &Arc<Self>) {
        let store = Arc::clone(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(FLUSH_INTERVAL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await;
            loop {
                tick.tick().await;
                if let Err(cause) = store.flush().await {
                    tracing::error!("[niveis] falha ao gravar {}: {cause}", store.path.display());
                }
            }
        });
    }

    // ------------------------------------------------------------------
    // XP
    // ------------------------------------------------------------------

    /// Soma (ou tira, com delta negativo) e devolve o XP antes e depois.
    /// Nunca fica negativo; zero sai do arquivo.
    pub fn add_xp(&self, guild_id: u64, user_id: u64, delta: i64) -> (i64, i64) {
        let mut state = self.lock();
        let members = state.file.guilds.entry(guild_id).or_default();
        let old = members.get(&user_id).copied().unwrap_or(0);
        let updated = old.saturating_add(delta).max(0);

        if updated == 0 {
            members.shift_remove(&user_id);
        } else {
            members.insert(user_id, updated);
        }
        if updated != old {
            state.dirty = true;
        }
        (old, updated)
    }

    pub fn xp_of(&self, guild_id: u64, user_id: u64) -> i64 {
        self.lock()
            .file
            .guilds
            .get(&guild_id)
            .and_then(|m| m.get(&user_id))
            .copied()
            .unwrap_or(0)
    }

    /// O ranking de um servidor, do maior XP para o menor (empate: menor id).
    pub fn guild_board(&self, guild_id: u64) -> Vec<(u64, i64)> {
        let state = self.lock();
        let mut board: Vec<(u64, i64)> = state
            .file
            .guilds
            .get(&guild_id)
            .map(|m| m.iter().map(|(&u, &xp)| (u, xp)).collect())
            .unwrap_or_default();
        sort_board(&mut board);
        board
    }

    /// O ranking global: a soma do XP de cada pessoa nos servidores em que o
    /// nivel esta LIGADO. Os dados de um servidor desligado continuam
    /// guardados para quando religar.
    pub fn global_board(&self, guild_enabled: impl Fn(u64) -> bool) -> Vec<(u64, i64)> {
        let state = self.lock();
        let mut totals: IndexMap<u64, i64> = IndexMap::new();
        for (&guild_id, members) in &state.file.guilds {
            if !guild_enabled(guild_id) {
                continue;
            }
            for (&user_id, &xp) in members {
                *totals.entry(user_id).or_default() += xp;
            }
        }
        let mut board: Vec<(u64, i64)> = totals.into_iter().collect();
        sort_board(&mut board);
        board
    }

    /// Em quantos servidores com nivel ligado a pessoa tem XP.
    pub fn active_guild_count(&self, user_id: u64, guild_enabled: impl Fn(u64) -> bool) -> usize {
        self.lock()
            .file
            .guilds
            .iter()
            .filter(|(g, m)| guild_enabled(**g) && m.contains_key(&user_id))
            .count()
    }

    // ------------------------------------------------------------------
    // DM de level up
    // ------------------------------------------------------------------

    pub fn dm_enabled(&self, user_id: u64) -> bool {
        self.lock()
            .file
            .users
            .get(&user_id)
            .is_none_or(|p| p.level_up_dms)
    }

    pub fn set_dm(&self, user_id: u64, enabled: bool) {
        let mut state = self.lock();
        // O padrao (ligado) nao ocupa espaco no arquivo.
        let changed = if enabled {
            state.file.users.shift_remove(&user_id).is_some()
        } else {
            !state.file.users.contains_key(&user_id)
        };
        if !enabled {
            state.file.users.insert(
                user_id,
                UserPrefs {
                    level_up_dms: false,
                },
            );
        }
        if changed {
            state.dirty = true;
        }
    }

    // ------------------------------------------------------------------
    // Disco
    // ------------------------------------------------------------------

    /// Grava se houve mudanca desde a ultima vez. Chamado pelo timer e no
    /// desligamento.
    pub async fn flush(&self) -> Result<(), Error> {
        let _write = self.write_lock.lock().await;
        let snapshot = {
            let mut state = self.lock();
            if !state.dirty {
                return Ok(());
            }
            state.dirty = false;
            // Copia sob o lock: a gravacao corre fora dele, enquanto as
            // mensagens continuam somando XP no original.
            LevelFile {
                last_updated_utc: Some(time::now()),
                ..state.file.clone()
            }
        };

        if let Err(cause) = json_file::write_atomic(&self.path, &snapshot).await {
            // Volta a marcar como sujo: a proxima volta do timer tenta de novo.
            self.lock().dirty = true;
            return Err(cause);
        }
        Ok(())
    }
}

fn sort_board(board: &mut [(u64, i64)]) {
    board.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::{copy_real, scratch_dir};

    #[tokio::test]
    async fn ida_e_volta_do_arquivo_real() {
        let dir = scratch_dir("levels");
        let Some(path) = copy_real(&dir, "levels.json") else {
            return;
        };
        let original = std::fs::read_to_string(&path).unwrap();
        let file: LevelFile = json_file::parse(&original).unwrap();
        // Mesmos bytes: ordem das chaves inclusive.
        assert_eq!(json_file::to_text(&file).unwrap(), original);

        let store = LevelStore::open(path.clone()).await.unwrap();
        let (&guild, members) = file.guilds.first().unwrap();
        let (&user, &xp) = members.first().unwrap();
        assert_eq!(store.xp_of(guild, user), xp);

        // Mudar e gravar, e o arquivo novo continua sendo o mesmo JSON a menos
        // do que mudou.
        store.add_xp(guild, user, 10);
        store.flush().await.unwrap();
        let reread: LevelFile = json_file::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(reread.guilds[&guild][&user], xp + 10);
        assert_eq!(reread.users, file.users);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn xp_nunca_negativo_e_dm_padrao_nao_ocupa_espaco() {
        let dir = scratch_dir("levels-ops");
        let store = LevelStore::open(dir.join("levels.json")).await.unwrap();

        assert_eq!(store.add_xp(1, 7, 50), (0, 50));
        assert_eq!(store.add_xp(1, 7, -80), (50, 0));
        assert_eq!(store.guild_board(1), vec![]);

        store.add_xp(1, 7, 10);
        store.add_xp(1, 8, 10);
        store.add_xp(2, 8, 5);
        assert_eq!(store.guild_board(1), vec![(7, 10), (8, 10)]);
        assert_eq!(
            store.global_board(|g| g == 1 || g == 2),
            vec![(8, 15), (7, 10)]
        );
        assert_eq!(store.active_guild_count(8, |g| g == 2), 1);

        assert!(store.dm_enabled(7));
        store.set_dm(7, false);
        assert!(!store.dm_enabled(7));
        store.set_dm(7, true);
        assert!(store.lock().file.users.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
