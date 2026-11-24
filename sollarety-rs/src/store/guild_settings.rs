//! Configuracao POR SERVIDOR, escrita pelo /config e lida por todo o resto
//! (GuildSettingsStore.cs). data/guild-settings.json.
//!
//! Duas diferencas deliberadas em relacao aos outros stores, as mesmas do C#:
//! nao tem poda (cada registro e a configuracao viva de um servidor), e tem
//! snapshot em memoria, republicado inteiro a cada escrita, para o caminho do
//! gateway consultar sem disco.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, RwLock},
};

use serde::{Deserialize, Serialize};

use super::{
    json_file, null_default,
    time::{self, Time},
};
use crate::Error;

const TAG: &str = "config";

/// O que cada servidor configurou. Nomes e ordem dos campos sao os do JSON.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GuildSettings {
    pub guild_id: u64,

    pub moderation_log_channel_id: Option<u64>,
    pub auto_softban_channel_id: Option<u64>,
    pub ticket_category_id: Option<u64>,
    pub ticket_staff_role_id: Option<u64>,
    pub ticket_log_channel_id: Option<u64>,

    // Verificacao Roblox. O vinculo em si e global (RobloxLinkStore); aqui
    // mora so o que ESTE servidor faz com ele.
    pub verified_role_id: Option<u64>,
    pub unverified_role_id: Option<u64>,
    /// Uma das chaves de `nickname_format`. None e "nenhum": o bot nao mexe
    /// no apelido.
    pub nickname_format: Option<String>,
    /// Idade minima da conta Roblox, em dias. Zero desliga o filtro.
    pub min_account_age_days: i32,
    #[serde(deserialize_with = "null_default")]
    pub group_binds: Vec<GroupBind>,

    // Niveis e economia: desligados ate quem administra o servidor ligar. A
    // ausencia da chave no arquivo vira false.
    pub leveling_enabled: bool,
    pub economy_enabled: bool,

    #[serde(with = "time::option")]
    pub updated_at_utc: Option<Time>,
    pub updated_by_id: Option<u64>,
}

impl GuildSettings {
    pub fn new(guild_id: u64) -> Self {
        Self {
            guild_id,
            ..Self::default()
        }
    }

    /// VerificationService.IsConfigured: o servidor ligou alguma parte da
    /// verificacao. Fica aqui porque o /config (moderacao) e a verificacao
    /// (roblox) perguntam a mesma coisa.
    pub fn verification_configured(&self) -> bool {
        self.verified_role_id.is_some_and(|id| id > 0)
            || self.unverified_role_id.is_some_and(|id| id > 0)
            || !self.group_binds.is_empty()
            || self
                .nickname_format
                .as_deref()
                .is_some_and(|f| f != nickname_format::NONE)
    }
}

/// Os formatos de apelido da verificacao (NicknameFormats do C#). Os valores
/// sao os que ficam gravados no arquivo, por isso em portugues.
pub mod nickname_format {
    pub const NONE: &str = "nenhum";
    pub const USERNAME: &str = "usuario";
    pub const DISPLAY: &str = "exibicao";
    pub const SMART: &str = "inteligente";

    /// Teto de apelido do Discord.
    pub const MAX_LENGTH: usize = 32;

    pub fn describe(format: Option<&str>) -> &'static str {
        match format {
            Some(USERNAME) => "Roblox username",
            Some(DISPLAY) => "Roblox display name",
            Some(SMART) => "display name (@username)",
            _ => "don't change the nickname",
        }
    }
}

/// Regra "quem esta no grupo X com rank entre A e B ganha o cargo Y". Rank 0
/// e "fora do grupo", como o Roblox conta.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GroupBind {
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    pub group_id: i64,
    #[serde(deserialize_with = "null_default")]
    pub group_name: String,
    pub min_rank: i32,
    pub max_rank: i32,
    pub role_id: u64,
    pub created_by_id: u64,
    #[serde(with = "time")]
    pub created_at_utc: Time,
}

impl Default for GroupBind {
    fn default() -> Self {
        Self {
            id: String::new(),
            group_id: 0,
            group_name: String::new(),
            min_rank: 0,
            max_rank: 0,
            role_id: 0,
            created_by_id: 0,
            created_at_utc: time::min_value(),
        }
    }
}

impl GroupBind {
    pub fn matches(&self, rank: i32) -> bool {
        rank >= self.min_rank && rank <= self.max_rank
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GuildSettingsFile {
    #[serde(deserialize_with = "null_default")]
    pub guilds: Vec<GuildSettings>,
    /// As chaves antigas do config.jsonc ja foram trazidas para ca. No
    /// arquivo, e nao em memoria: a migracao roda uma vez na vida da
    /// instalacao.
    pub migrated_from_config: bool,
    #[serde(with = "time::option")]
    pub last_updated_utc: Option<Time>,
}

/// Visao mutavel entregue ao `update`. Quem muta precisa dizer que mutou.
pub struct GuildSettingsEdit {
    pub file: GuildSettingsFile,
    changed: bool,
}

impl GuildSettingsEdit {
    pub fn mark_changed(&mut self) {
        self.changed = true;
    }

    /// O registro daquele servidor, criando um vazio se ainda nao existir.
    /// Nao marca alterado sozinho - quem chama decide.
    pub fn get(&mut self, guild_id: u64) -> &mut GuildSettings {
        let index = match self.file.guilds.iter().position(|g| g.guild_id == guild_id) {
            Some(index) => index,
            None => {
                self.file.guilds.push(GuildSettings::new(guild_id));
                self.file.guilds.len() - 1
            }
        };
        &mut self.file.guilds[index]
    }
}

pub struct GuildSettingsStore {
    path: PathBuf,
    lock: tokio::sync::Mutex<()>,
    /// Publicado inteiro a cada escrita, nunca mutado no lugar.
    snapshot: RwLock<Arc<HashMap<u64, GuildSettings>>>,
}

impl GuildSettingsStore {
    /// Abre e carrega. Arquivo corrompido e erro: subir com "ninguem
    /// configurou nada" e o proximo /config apagariam tudo.
    pub async fn open(path: PathBuf) -> Result<Self, Error> {
        json_file::delete_leftover_temp(&path, TAG);
        let store = Self {
            path,
            lock: tokio::sync::Mutex::new(()),
            snapshot: RwLock::default(),
        };
        let file = store.read_file().await?;
        store.publish(&file);
        Ok(store)
    }

    /// A configuracao de um servidor, ou None se ele nunca configurou nada.
    /// Sincrono e sem I/O: e chamado no caminho do gateway.
    pub fn for_guild(&self, guild_id: u64) -> Option<GuildSettings> {
        self.all().get(&guild_id).cloned()
    }

    /// Todos os servidores com alguma configuracao.
    pub fn all(&self) -> Arc<HashMap<u64, GuildSettings>> {
        self.snapshot
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Unico caminho de escrita: le, aplica e grava sob o mesmo lock. Se o
    /// mutador nao chamar `mark_changed`, nada e gravado.
    pub async fn update<T>(
        &self,
        mutate: impl FnOnce(&mut GuildSettingsEdit) -> T,
    ) -> Result<T, Error> {
        let _guard = self.lock.lock().await;
        let mut edit = GuildSettingsEdit {
            file: self.read_file().await?,
            changed: false,
        };
        let result = mutate(&mut edit);

        if edit.changed {
            edit.file.last_updated_utc = Some(time::now());
            json_file::write_atomic(&self.path, &edit.file).await?;
            self.publish(&edit.file);
        }
        Ok(result)
    }

    /// Aplica uma mudanca na configuracao de um servidor e devolve como ela
    /// ficou. Guarda quem mexeu e quando, para o /config view poder dizer.
    pub async fn set(
        &self,
        guild_id: u64,
        actor_id: u64,
        apply: impl FnOnce(&mut GuildSettings),
    ) -> Result<GuildSettings, Error> {
        self.update(|edit| {
            let settings = edit.get(guild_id);
            apply(settings);
            settings.updated_at_utc = Some(time::now());
            settings.updated_by_id = Some(actor_id);
            let copy = settings.clone();
            edit.mark_changed();
            copy
        })
        .await
    }

    async fn read_file(&self) -> Result<GuildSettingsFile, Error> {
        json_file::read(&self.path, TAG).await
    }

    fn publish(&self, file: &GuildSettingsFile) {
        // O ultimo de um id repetido vence, como no ToDictionary... que no C#
        // lancaria. Aqui so nao se perde a subida por isso.
        let map = file
            .guilds
            .iter()
            .map(|g| (g.guild_id, g.clone()))
            .collect();
        *self.snapshot.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(map);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::{copy_real, scratch_dir};

    #[tokio::test]
    async fn ida_e_volta_do_arquivo_real_sem_mudar_um_byte() {
        let dir = scratch_dir("guild-settings");
        let Some(path) = copy_real(&dir, "guild-settings.json") else {
            return;
        };
        let original = std::fs::read_to_string(&path).unwrap();

        let file: GuildSettingsFile = json_file::parse(&original).unwrap();
        assert_eq!(json_file::to_text(&file).unwrap(), original);

        let store = GuildSettingsStore::open(path.clone()).await.unwrap();
        for g in &file.guilds {
            assert_eq!(store.for_guild(g.guild_id).as_ref(), Some(g));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn set_grava_publica_e_guarda_quem_mexeu() {
        let dir = scratch_dir("guild-settings-set");
        let path = dir.join("guild-settings.json");
        let store = GuildSettingsStore::open(path.clone()).await.unwrap();
        assert!(store.for_guild(1).is_none());

        let saved = store
            .set(1, 42, |s| s.leveling_enabled = true)
            .await
            .unwrap();
        assert!(saved.leveling_enabled);
        assert_eq!(saved.updated_by_id, Some(42));
        assert_eq!(store.for_guild(1), Some(saved.clone()));

        // Um mutador que nao marca mudanca nao grava.
        let before = std::fs::read_to_string(&path).unwrap();
        store.update(|edit| edit.get(2).guild_id).await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        assert!(store.for_guild(2).is_none());

        let reopened = GuildSettingsStore::open(path).await.unwrap();
        assert_eq!(reopened.for_guild(1), Some(saved));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn verificacao_configurada() {
        let mut s = GuildSettings::new(1);
        assert!(!s.verification_configured());
        s.nickname_format = Some(nickname_format::NONE.into());
        assert!(!s.verification_configured());
        s.nickname_format = Some(nickname_format::SMART.into());
        assert!(s.verification_configured());
    }
}
