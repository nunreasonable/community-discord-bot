//! Advertencias (WarningStore.cs). data/warnings.json.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{
    json_file, null_default,
    time::{self, Time},
};
use crate::Error;

const TAG: &str = "warnings";

/// Teto de advertencias guardadas POR SERVIDOR. Por servidor, e nao global:
/// um corte global faria um servidor movimentado apagar o historico de
/// moderacao de outro.
const MAX_STORED_WARNINGS: usize = 5000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Warning {
    /// Id curto, o que a pessoa digita no /delwarn.
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    pub guild_id: u64,
    pub user_id: u64,
    pub moderator_id: u64,
    #[serde(deserialize_with = "null_default")]
    pub moderator_tag: String,
    #[serde(deserialize_with = "null_default")]
    pub reason: String,
    #[serde(with = "time")]
    pub created_at_utc: Time,
}

impl Default for Warning {
    fn default() -> Self {
        Self {
            id: String::new(),
            guild_id: 0,
            user_id: 0,
            moderator_id: 0,
            moderator_tag: String::new(),
            reason: String::new(),
            created_at_utc: time::min_value(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WarningFile {
    #[serde(deserialize_with = "null_default")]
    pub warnings: Vec<Warning>,
    #[serde(with = "time::option")]
    pub last_updated_utc: Option<Time>,
}

impl WarningFile {
    /// Id curto unico dentro do arquivo. Chamado de dentro do `update`, onde
    /// o conjunto atual esta visivel e a unicidade se confere sem corrida.
    pub fn new_unique_id(&self) -> String {
        loop {
            let id = super::short_id(8);
            if !self.warnings.iter().any(|w| w.id.eq_ignore_ascii_case(&id)) {
                return id;
            }
        }
    }
}

/// Visao mutavel entregue ao `update`. Quem muta precisa dizer que mutou.
pub struct WarningEdit {
    pub file: WarningFile,
    changed: bool,
}

impl WarningEdit {
    pub fn mark_changed(&mut self) {
        self.changed = true;
    }
}

pub struct WarningStore {
    path: PathBuf,
    lock: tokio::sync::Mutex<()>,
}

impl WarningStore {
    /// Apaga o .tmp orfao (ele guarda o historico inteiro de moderacao) e
    /// confere que o arquivo le.
    pub async fn open(path: PathBuf) -> Result<Self, Error> {
        json_file::delete_leftover_temp(&path, TAG);
        let store = Self {
            path,
            lock: tokio::sync::Mutex::new(()),
        };
        store.read().await?;
        Ok(store)
    }

    pub async fn read(&self) -> Result<WarningFile, Error> {
        let _guard = self.lock.lock().await;
        json_file::read(&self.path, TAG).await
    }

    /// Advertencias de alguem num servidor, da mais nova para a mais velha.
    pub async fn list(&self, guild_id: u64, user_id: u64) -> Result<Vec<Warning>, Error> {
        let mut list: Vec<Warning> = self
            .read()
            .await?
            .warnings
            .into_iter()
            .filter(|w| w.guild_id == guild_id && w.user_id == user_id)
            .collect();
        // Estavel, como o OrderByDescending do LINQ.
        list.sort_by_key(|w| std::cmp::Reverse(w.created_at_utc));
        Ok(list)
    }

    /// A advertencia com esse id NAQUELE servidor (sem diferenciar caixa).
    pub async fn find(&self, guild_id: u64, id: &str) -> Result<Option<Warning>, Error> {
        Ok(self
            .read()
            .await?
            .warnings
            .into_iter()
            .find(|w| w.guild_id == guild_id && w.id.eq_ignore_ascii_case(id)))
    }

    /// Grava a advertencia com um id novo e devolve (id, total da pessoa no
    /// servidor). O `id` que vier em `warning` e ignorado.
    pub async fn add(&self, mut warning: Warning) -> Result<(String, usize), Error> {
        self.update(|edit| {
            warning.id = edit.file.new_unique_id();
            let id = warning.id.clone();
            let (guild, user) = (warning.guild_id, warning.user_id);
            edit.file.warnings.push(warning);
            edit.mark_changed();
            let total = edit
                .file
                .warnings
                .iter()
                .filter(|w| w.guild_id == guild && w.user_id == user)
                .count();
            (id, total)
        })
        .await
    }

    /// Apaga pelo id, preso ao servidor: um id de outro servidor nao pode ser
    /// apagado daqui. Devolve o que foi apagado.
    pub async fn delete(&self, guild_id: u64, id: &str) -> Result<Option<Warning>, Error> {
        self.update(|edit| {
            let index = edit
                .file
                .warnings
                .iter()
                .position(|w| w.guild_id == guild_id && w.id.eq_ignore_ascii_case(id))?;
            edit.mark_changed();
            Some(edit.file.warnings.remove(index))
        })
        .await
    }

    /// Unica forma de escrever: le, aplica e grava sob o mesmo lock. Uma
    /// mutacao que nao chamou `mark_changed` nao reescreve o arquivo nem mexe
    /// no lastUpdatedUtc.
    pub async fn update<T>(&self, mutate: impl FnOnce(&mut WarningEdit) -> T) -> Result<T, Error> {
        let _guard = self.lock.lock().await;
        let mut edit = WarningEdit {
            file: json_file::read(&self.path, TAG).await?,
            changed: false,
        };
        let result = mutate(&mut edit);

        if edit.changed {
            prune(&mut edit.file);
            edit.file.last_updated_utc = Some(time::now());
            json_file::write_atomic(&self.path, &edit.file).await?;
        }
        Ok(result)
    }
}

/// Descarta as mais antigas de CADA servidor acima do teto: advertencia
/// recente e a que ainda importa para decidir uma punicao.
fn prune(file: &mut WarningFile) {
    // Atalho: se o total cabe no teto, nenhum servidor sozinho estoura.
    if file.warnings.len() <= MAX_STORED_WARNINGS {
        return;
    }

    let mut by_guild: std::collections::HashMap<u64, Vec<&Warning>> = Default::default();
    for w in &file.warnings {
        by_guild.entry(w.guild_id).or_default().push(w);
    }
    let mut keep: Vec<Warning> = by_guild
        .into_values()
        .flat_map(|mut list| {
            list.sort_by_key(|w| std::cmp::Reverse(w.created_at_utc));
            list.into_iter().take(MAX_STORED_WARNINGS).cloned()
        })
        .collect();
    keep.sort_by_key(|w| w.created_at_utc);

    let dropped = file.warnings.len() - keep.len();
    file.warnings = keep;
    if dropped > 0 {
        tracing::warn!(
            "[warnings] podadas {dropped} advertencia(s) antiga(s); teto e {MAX_STORED_WARNINGS} por servidor."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::scratch_dir;

    fn sample(guild: u64, user: u64) -> Warning {
        Warning {
            guild_id: guild,
            user_id: user,
            moderator_id: 9,
            moderator_tag: "mod#0".into(),
            reason: "spam".into(),
            created_at_utc: time::now(),
            ..Warning::default()
        }
    }

    #[tokio::test]
    async fn adiciona_lista_e_apaga_preso_ao_servidor() {
        let dir = scratch_dir("warnings");
        let path = dir.join("warnings.json");
        let store = WarningStore::open(path.clone()).await.unwrap();

        let (id, total) = store.add(sample(1, 5)).await.unwrap();
        assert_eq!(id.len(), 8);
        assert_eq!(total, 1);
        let (_, total) = store.add(sample(1, 5)).await.unwrap();
        assert_eq!(total, 2);
        store.add(sample(2, 5)).await.unwrap();

        assert_eq!(store.list(1, 5).await.unwrap().len(), 2);
        assert!(store.delete(2, &id).await.unwrap().is_none());
        assert_eq!(
            store
                .delete(1, &id.to_uppercase())
                .await
                .unwrap()
                .unwrap()
                .id,
            id
        );
        assert_eq!(store.list(1, 5).await.unwrap().len(), 1);

        // O que esta no disco le de volta igual.
        let text = std::fs::read_to_string(&path).unwrap();
        let file: WarningFile = json_file::parse(&text).unwrap();
        assert_eq!(json_file::to_text(&file).unwrap(), text);
        assert!(text.contains("\"moderatorTag\": \"mod#0\""));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn le_o_formato_do_csharp() {
        let text = r#"{
  "warnings": [
    {
      "id": "a1b2c3d4",
      "guildId": 1537491530225418302,
      "userId": 1,
      "moderatorId": 2,
      "moderatorTag": "x#0",
      "reason": "r",
      "createdAtUtc": "2026-08-30T10:39:33.2565484+00:00"
    }
  ],
  "lastUpdatedUtc": null
}"#;
        let file: WarningFile = json_file::parse(text).unwrap();
        assert_eq!(file.warnings[0].guild_id, 1537491530225418302);
        assert_eq!(json_file::to_text(&file).unwrap(), text);
    }
}
