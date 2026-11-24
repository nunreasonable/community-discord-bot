//! Vinculos Discord -> Roblox, GLOBAIS: quem verifica uma vez fica
//! verificado em todo servidor (RobloxLinkStore.cs). data/roblox-links.json.
//!
//! Mesmo desenho do GuildSettingsStore: lock, escrita atomica e snapshot em
//! memoria, para a entrada de membro consultar sem disco. Sem poda: cada
//! registro e um vinculo vivo, e quem apaga e o /unverify do dono.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, RwLock},
};

use serde::{Deserialize, Serialize};

use super::{
    guild_settings::nickname_format,
    json_file, null_default,
    time::{self, Time},
};
use crate::Error;

const TAG: &str = "roblox";

/// O que identifica a conta e o robloxId. Nome e exibicao mudam no Roblox
/// sem aviso; ficam aqui como a ultima leitura, e o /update os renova.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RobloxLink {
    pub discord_id: u64,
    pub roblox_id: i64,
    #[serde(deserialize_with = "null_default")]
    pub roblox_name: String,
    #[serde(deserialize_with = "null_default")]
    pub display_name: String,
    #[serde(with = "time::option")]
    pub roblox_created_utc: Option<Time>,
    #[serde(with = "time")]
    pub verified_at_utc: Time,
    #[serde(with = "time::option")]
    pub refreshed_at_utc: Option<Time>,
}

impl Default for RobloxLink {
    fn default() -> Self {
        Self {
            discord_id: 0,
            roblox_id: 0,
            roblox_name: String::new(),
            display_name: String::new(),
            roblox_created_utc: None,
            verified_at_utc: time::min_value(),
            refreshed_at_utc: None,
        }
    }
}

impl RobloxLink {
    /// O apelido que este vinculo produz num formato, ou None para "nao
    /// mexer" (NicknameFormats.Render).
    pub fn nickname(&self, format: Option<&str>) -> Option<String> {
        let name = self.roblox_name.as_str();
        let display = if self.display_name.trim().is_empty() {
            name
        } else {
            self.display_name.as_str()
        };

        let rendered = match format {
            Some(nickname_format::USERNAME) => name.to_owned(),
            Some(nickname_format::DISPLAY) => display.to_owned(),
            // "Exibicao (@usuario)" so quando os dois diferem, como o
            // {smart-name} do BloxLink; passando de 32, fica so a exibicao.
            Some(nickname_format::SMART) => {
                if display.to_lowercase() == name.to_lowercase() {
                    name.to_owned()
                } else {
                    let both = format!("{display} (@{name})");
                    if crate::embeds::utf16_len(&both) <= nickname_format::MAX_LENGTH {
                        both
                    } else {
                        display.to_owned()
                    }
                }
            }
            _ => return None,
        };

        (!rendered.trim().is_empty())
            .then(|| crate::embeds::trim(&rendered, nickname_format::MAX_LENGTH))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RobloxLinkFile {
    #[serde(deserialize_with = "null_default")]
    pub links: Vec<RobloxLink>,
    #[serde(with = "time::option")]
    pub last_updated_utc: Option<Time>,
}

pub struct RobloxLinkStore {
    path: PathBuf,
    lock: tokio::sync::Mutex<()>,
    snapshot: RwLock<Arc<HashMap<u64, RobloxLink>>>,
}

impl RobloxLinkStore {
    pub async fn open(path: PathBuf) -> Result<Self, Error> {
        json_file::delete_leftover_temp(&path, TAG);
        let store = Self {
            path,
            lock: tokio::sync::Mutex::new(()),
            snapshot: RwLock::default(),
        };
        let file = json_file::read(&store.path, TAG).await?;
        store.publish(&file);
        Ok(store)
    }

    fn snapshot(&self) -> Arc<HashMap<u64, RobloxLink>> {
        self.snapshot
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// O vinculo de alguem, ou None. Sincrono e sem I/O.
    pub fn for_user(&self, discord_id: u64) -> Option<RobloxLink> {
        self.snapshot().get(&discord_id).cloned()
    }

    pub fn count(&self) -> usize {
        self.snapshot().len()
    }

    /// Grava (ou troca) o vinculo de uma conta do Discord. Trocar de conta
    /// Roblox e so verificar de novo: o registro anterior e substituido.
    pub async fn set(&self, link: RobloxLink) -> Result<RobloxLink, Error> {
        self.update(|file| {
            file.links.retain(|l| l.discord_id != link.discord_id);
            file.links.push(link.clone());
            (true, link)
        })
        .await
    }

    /// Apaga o vinculo. Devolve o que foi apagado, ou None se nao havia.
    pub async fn remove(&self, discord_id: u64) -> Result<Option<RobloxLink>, Error> {
        self.update(
            |file| match file.links.iter().position(|l| l.discord_id == discord_id) {
                Some(index) => (true, Some(file.links.remove(index))),
                None => (false, None),
            },
        )
        .await
    }

    /// Renova nome, exibicao e data de criacao lidos da API publica. So se o
    /// vinculo ainda aponta para a MESMA conta: entre a leitura e a escrita a
    /// pessoa pode ter verificado outra.
    pub async fn refresh(
        &self,
        discord_id: u64,
        roblox_id: i64,
        name: &str,
        display_name: &str,
        created: Option<Time>,
    ) -> Result<Option<RobloxLink>, Error> {
        self.update(|file| {
            let Some(existing) = file.links.iter_mut().find(|l| l.discord_id == discord_id) else {
                return (false, None);
            };
            if existing.roblox_id != roblox_id {
                return (false, Some(existing.clone()));
            }
            existing.roblox_name = name.to_owned();
            existing.display_name = display_name.to_owned();
            existing.roblox_created_utc = created.or(existing.roblox_created_utc);
            existing.refreshed_at_utc = Some(time::now());
            (true, Some(existing.clone()))
        })
        .await
    }

    /// Mutador SINCRONO que devolve (mudou, resultado).
    async fn update<T>(
        &self,
        mutate: impl FnOnce(&mut RobloxLinkFile) -> (bool, T),
    ) -> Result<T, Error> {
        let _guard = self.lock.lock().await;
        let mut file: RobloxLinkFile = json_file::read(&self.path, TAG).await?;
        let (changed, result) = mutate(&mut file);

        if changed {
            file.last_updated_utc = Some(time::now());
            json_file::write_atomic(&self.path, &file).await?;
            self.publish(&file);
        }
        Ok(result)
    }

    fn publish(&self, file: &RobloxLinkFile) {
        // Repetido, vale o ultimo (o `GroupBy(...).Last()` do C#).
        let map = file
            .links
            .iter()
            .map(|l| (l.discord_id, l.clone()))
            .collect();
        *self.snapshot.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(map);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::{copy_real, scratch_dir};

    fn link(discord: u64, roblox: i64) -> RobloxLink {
        RobloxLink {
            discord_id: discord,
            roblox_id: roblox,
            roblox_name: "builderman".into(),
            display_name: "Builder".into(),
            verified_at_utc: time::now(),
            ..RobloxLink::default()
        }
    }

    #[tokio::test]
    async fn ida_e_volta_do_arquivo_real() {
        let dir = scratch_dir("links");
        let Some(path) = copy_real(&dir, "roblox-links.json") else {
            return;
        };
        let original = std::fs::read_to_string(&path).unwrap();
        let file: RobloxLinkFile = json_file::parse(&original).unwrap();
        assert_eq!(json_file::to_text(&file).unwrap(), original);
        let store = RobloxLinkStore::open(path).await.unwrap();
        assert_eq!(store.count(), file.links.len());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn set_troca_refresh_confere_a_conta_e_remove() {
        let dir = scratch_dir("links-ops");
        let path = dir.join("roblox-links.json");
        let store = RobloxLinkStore::open(path.clone()).await.unwrap();

        store.set(link(1, 10)).await.unwrap();
        store.set(link(1, 11)).await.unwrap();
        assert_eq!(store.count(), 1);
        assert_eq!(store.for_user(1).unwrap().roblox_id, 11);

        // Conta trocada no meio: nao mistura os nomes.
        let stale = store
            .refresh(1, 10, "old", "Old", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stale.roblox_name, "builderman");
        let fresh = store
            .refresh(1, 11, "new", "", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fresh.roblox_name, "new");
        assert!(fresh.refreshed_at_utc.is_some());

        let reopened = RobloxLinkStore::open(path).await.unwrap();
        assert_eq!(reopened.for_user(1), Some(fresh));
        assert_eq!(store.remove(1).await.unwrap().unwrap().roblox_id, 11);
        assert!(store.remove(1).await.unwrap().is_none());
        assert!(store.for_user(1).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn apelido_por_formato() {
        let mut l = link(1, 1);
        assert_eq!(l.nickname(Some(nickname_format::NONE)), None);
        assert_eq!(
            l.nickname(Some(nickname_format::USERNAME)).unwrap(),
            "builderman"
        );
        assert_eq!(
            l.nickname(Some(nickname_format::SMART)).unwrap(),
            "Builder (@builderman)"
        );
        l.display_name = "BUILDERMAN".into();
        assert_eq!(
            l.nickname(Some(nickname_format::SMART)).unwrap(),
            "builderman"
        );
        l.display_name = "a".repeat(20);
        l.roblox_name = "b".repeat(20);
        assert_eq!(
            l.nickname(Some(nickname_format::SMART)).unwrap(),
            "a".repeat(20)
        );
    }
}
