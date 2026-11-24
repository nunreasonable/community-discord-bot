//! Carteiras de SOL$ (EconomyStore.cs). data/economy.json.
//!
//! Diferente do XP, cada mudanca vai para o disco NA HORA. O mutador roda
//! numa COPIA; so se a gravacao der certo a copia vira o estado publicado.
//! Assim um disco cheio nao deixa a memoria a frente do arquivo.

use std::{
    path::PathBuf,
    sync::{Arc, RwLock},
};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::{
    json_file, null_default,
    time::{self, Time},
};
use crate::Error;

const TAG: &str = "economia";

/// Nome da moeda, como o EconomyService.Currency.
pub const CURRENCY: &str = "SOL$";

/// "1,250 SOL$" (EconomyService.Format). Fica aqui porque o /quiz (ia) e o
/// /profile (comunidade) escrevem valores tambem.
pub fn format(amount: i64) -> String {
    format!("{} {CURRENCY}", crate::embeds::number(amount))
}

/// A carteira de uma pessoa. Global: vale em todo servidor com a economia
/// ligada.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Wallet {
    pub balance: i64,
    #[serde(with = "time::option")]
    pub last_daily_utc: Option<Time>,
    pub daily_streak: i32,
    #[serde(with = "time::option")]
    pub last_work_utc: Option<Time>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EconomyFile {
    #[serde(deserialize_with = "null_default")]
    pub users: IndexMap<u64, Wallet>,
    #[serde(with = "time::option")]
    pub last_updated_utc: Option<Time>,
}

/// O que um mutador enxerga.
pub struct EconomyEdit {
    users: IndexMap<u64, Wallet>,
    changed: bool,
}

impl EconomyEdit {
    pub fn mark_changed(&mut self) {
        self.changed = true;
    }

    /// A carteira da pessoa, criada vazia se nao existir (sem marcar mudanca).
    pub fn get(&mut self, user_id: u64) -> &mut Wallet {
        self.users.entry(user_id).or_default()
    }
}

pub struct EconomyStore {
    path: PathBuf,
    lock: tokio::sync::Mutex<()>,
    /// Publicado inteiro de uma vez e nunca mais alterado.
    snapshot: RwLock<Arc<IndexMap<u64, Wallet>>>,
}

impl EconomyStore {
    /// Carrega. Arquivo corrompido derruba a subida de proposito.
    pub async fn open(path: PathBuf) -> Result<Self, Error> {
        json_file::delete_leftover_temp(&path, TAG);
        let file: EconomyFile = json_file::read(&path, TAG).await?;
        tracing::info!("[economia] carregado: {} carteira(s)", file.users.len());
        Ok(Self {
            path,
            lock: tokio::sync::Mutex::new(()),
            snapshot: RwLock::new(Arc::new(file.users)),
        })
    }

    fn snapshot(&self) -> Arc<IndexMap<u64, Wallet>> {
        self.snapshot
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Copia da carteira, ou None para quem nunca teve uma.
    pub fn for_user(&self, user_id: u64) -> Option<Wallet> {
        self.snapshot().get(&user_id).cloned()
    }

    /// Todas as carteiras com saldo, da maior para a menor (empate: menor id).
    pub fn board(&self) -> Vec<(u64, i64)> {
        let mut board: Vec<(u64, i64)> = self
            .snapshot()
            .iter()
            .filter(|(_, w)| w.balance > 0)
            .map(|(&u, w)| (u, w.balance))
            .collect();
        board.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        board
    }

    /// Muda uma ou mais carteiras numa gravacao so. O mutador e sincrono e
    /// precisa chamar `mark_changed`; sem isso nada e gravado.
    pub async fn update<T>(&self, mutate: impl FnOnce(&mut EconomyEdit) -> T) -> Result<T, Error> {
        let _guard = self.lock.lock().await;
        let mut edit = EconomyEdit {
            users: (*self.snapshot()).clone(),
            changed: false,
        };
        let result = mutate(&mut edit);

        if edit.changed {
            let file = EconomyFile {
                users: edit.users,
                last_updated_utc: Some(time::now()),
            };
            json_file::write_atomic(&self.path, &file).await?;
            *self.snapshot.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(file.users);
        }
        Ok(result)
    }

    /// Credita um premio fixo, sem cooldown nem streak (EconomyService.AwardAsync).
    /// Quem chama (o /quiz, por exemplo) ja decidiu que a pessoa ganhou.
    /// Devolve o saldo novo; estourar o i64 e erro, como o `checked` do C#.
    pub async fn award(&self, user_id: u64, amount: i64) -> Result<i64, Error> {
        self.update(|edit| {
            let wallet = edit.get(user_id);
            let balance = wallet.balance.checked_add(amount).ok_or("saldo estourou")?;
            wallet.balance = balance;
            edit.mark_changed();
            Ok::<_, Error>(balance)
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::{copy_real, scratch_dir};

    #[tokio::test]
    async fn ida_e_volta_do_arquivo_real_com_json_igual() {
        let dir = scratch_dir("economy");
        let Some(path) = copy_real(&dir, "economy.json") else {
            return;
        };
        let original = std::fs::read_to_string(&path).unwrap();
        let file: EconomyFile = json_file::parse(&original).unwrap();
        assert_eq!(json_file::to_text(&file).unwrap(), original);

        // Gravado pelo store, o JSON continua igual ao original a menos do
        // lastUpdatedUtc (comparado como valor, nao como texto).
        let store = EconomyStore::open(path.clone()).await.unwrap();
        store.update(|edit| edit.mark_changed()).await.unwrap();
        let mut before: serde_json::Value = serde_json::from_str(&original).unwrap();
        let mut after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        before["lastUpdatedUtc"] = serde_json::Value::Null;
        after["lastUpdatedUtc"] = serde_json::Value::Null;
        assert_eq!(before, after);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn award_grava_e_publica() {
        let dir = scratch_dir("economy-award");
        let path = dir.join("economy.json");
        let store = EconomyStore::open(path.clone()).await.unwrap();
        assert!(store.for_user(3).is_none());

        assert_eq!(store.award(3, 100).await.unwrap(), 100);
        assert_eq!(store.award(3, 25).await.unwrap(), 125);
        assert!(store.award(3, i64::MAX).await.is_err());
        assert_eq!(store.for_user(3).unwrap().balance, 125);
        assert_eq!(store.board(), vec![(3, 125)]);

        // O que nao marcou mudanca nao chega ao snapshot.
        store.update(|edit| edit.get(4).balance = 9).await.unwrap();
        assert!(store.for_user(4).is_none());

        let reopened = EconomyStore::open(path).await.unwrap();
        assert_eq!(reopened.for_user(3).unwrap().balance, 125);
        assert_eq!(format(1250), "1,250 SOL$");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
