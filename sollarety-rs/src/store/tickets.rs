//! Tickets em disco (TicketStore.cs). data/tickets.json.
//!
//! Em disco, e nao em memoria, e o que faz um painel publicado continuar
//! funcionando depois de o bot reiniciar.

use std::path::PathBuf;

use chrono::Duration;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::{
    json_file, null_default,
    time::{self, Time},
};
use crate::Error;

const TAG: &str = "tickets";

/// Teto de tickets FECHADOS guardados por servidor. Ticket aberto nunca e
/// podado: os botoes do canal resolvem pelo id no arquivo.
const MAX_CLOSED_PER_GUILD: usize = 2000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Ticket {
    /// Id curto, o que viaja no custom id dos botoes.
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    /// Sequencial POR SERVIDOR; e o numero no nome do canal.
    pub number: u64,
    pub guild_id: u64,
    pub channel_id: u64,
    pub opener_id: u64,
    /// Chave do tipo (TicketTypes).
    #[serde(rename = "type", deserialize_with = "null_default")]
    pub kind: String,
    #[serde(deserialize_with = "null_default")]
    pub subject: String,
    /// Quem assumiu o atendimento, se alguem assumiu.
    pub claimed_by_id: Option<u64>,
    #[serde(with = "time")]
    pub opened_at_utc: Time,
    #[serde(with = "time::option")]
    pub closed_at_utc: Option<Time>,
    pub closed_by_id: Option<u64>,
    /// Quando alguem comecou a fechar (arquivar e so depois apagar). Barra o
    /// fechamento duplo sem declarar fechado o que ainda nao foi arquivado.
    #[serde(with = "time::option")]
    pub closing_since: Option<Time>,
    /// O C# serializa a propriedade calculada `IsOpen` (e ignora na
    /// leitura). Aqui e recalculada antes de cada gravacao; use `is_open()`.
    #[serde(rename = "IsOpen", skip_deserializing)]
    is_open_on_disk: bool,
}

impl Default for Ticket {
    fn default() -> Self {
        Self {
            id: String::new(),
            number: 0,
            guild_id: 0,
            channel_id: 0,
            opener_id: 0,
            kind: String::new(),
            subject: String::new(),
            claimed_by_id: None,
            opened_at_utc: time::min_value(),
            closed_at_utc: None,
            closed_by_id: None,
            closing_since: None,
            is_open_on_disk: true,
        }
    }
}

impl Ticket {
    pub fn is_open(&self) -> bool {
        self.closed_at_utc.is_none()
    }

    /// Ha um fechamento em andamento e recente. A janela de 2 minutos evita
    /// que uma tentativa que morreu no meio tranque o ticket para sempre.
    pub fn is_closing(&self, now: Time) -> bool {
        self.closing_since
            .is_some_and(|since| now - since < Duration::minutes(2))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TicketFile {
    #[serde(deserialize_with = "null_default")]
    pub tickets: Vec<Ticket>,
    /// Ultimo numero usado em cada servidor, com a chave em texto como no C#.
    /// Guardado, e nao max(number)+1: a poda apaga fechados antigos e um
    /// contador derivado repetiria numeros.
    #[serde(deserialize_with = "null_default")]
    pub last_number_by_guild: IndexMap<String, u64>,
    #[serde(with = "time::option")]
    pub last_updated_utc: Option<Time>,
}

impl TicketFile {
    /// Proximo numero do servidor. Chamado DENTRO do mutador, para dois
    /// cliques simultaneos nao tirarem o mesmo.
    pub fn next_number(&mut self, guild_id: u64) -> u64 {
        let slot = self
            .last_number_by_guild
            .entry(guild_id.to_string())
            .or_insert(0);
        *slot += 1;
        *slot
    }

    /// Id curto unico no arquivo, conferido sem corrida.
    pub fn new_unique_id(&self) -> String {
        loop {
            let id = super::short_id(8);
            if !self.tickets.iter().any(|t| t.id.eq_ignore_ascii_case(&id)) {
                return id;
            }
        }
    }

    fn sync_is_open(&mut self) {
        for ticket in &mut self.tickets {
            ticket.is_open_on_disk = ticket.is_open();
        }
    }
}

/// Visao mutavel entregue ao `update`. Quem muta precisa dizer que mutou.
pub struct TicketEdit {
    pub file: TicketFile,
    changed: bool,
}

impl TicketEdit {
    pub fn mark_changed(&mut self) {
        self.changed = true;
    }
}

pub struct TicketStore {
    path: PathBuf,
    lock: tokio::sync::Mutex<()>,
}

impl TicketStore {
    pub async fn open(path: PathBuf) -> Result<Self, Error> {
        json_file::delete_leftover_temp(&path, TAG);
        let store = Self {
            path,
            lock: tokio::sync::Mutex::new(()),
        };
        store.read().await?;
        Ok(store)
    }

    pub async fn read(&self) -> Result<TicketFile, Error> {
        let _guard = self.lock.lock().await;
        self.read_file().await
    }

    async fn read_file(&self) -> Result<TicketFile, Error> {
        let mut file: TicketFile = json_file::read(&self.path, TAG).await?;
        file.sync_is_open();
        Ok(file)
    }

    /// O ticket de um canal, aberto ou nao.
    pub async fn by_channel(&self, channel_id: u64) -> Result<Option<Ticket>, Error> {
        Ok(self
            .read()
            .await?
            .tickets
            .into_iter()
            .find(|t| t.channel_id == channel_id))
    }

    /// O ticket de um id curto, dentro do servidor (sem diferenciar caixa).
    pub async fn by_id(&self, guild_id: u64, id: &str) -> Result<Option<Ticket>, Error> {
        Ok(self
            .read()
            .await?
            .tickets
            .into_iter()
            .find(|t| t.guild_id == guild_id && t.id.eq_ignore_ascii_case(id)))
    }

    /// Unico caminho de escrita. Mutador SINCRONO de proposito: o trabalho
    /// de rede acontece fora deste bloco.
    pub async fn update<T>(&self, mutate: impl FnOnce(&mut TicketEdit) -> T) -> Result<T, Error> {
        let _guard = self.lock.lock().await;
        let mut edit = TicketEdit {
            file: self.read_file().await?,
            changed: false,
        };
        let result = mutate(&mut edit);

        if edit.changed {
            prune(&mut edit.file);
            edit.file.last_updated_utc = Some(time::now());
            edit.file.sync_is_open();
            json_file::write_atomic(&self.path, &edit.file).await?;
        }
        Ok(result)
    }
}

/// Poda so o que ja fechou, por servidor.
fn prune(file: &mut TicketFile) {
    let mut closed_per_guild: std::collections::HashMap<u64, usize> = Default::default();
    for t in file.tickets.iter().filter(|t| !t.is_open()) {
        *closed_per_guild.entry(t.guild_id).or_default() += 1;
    }
    // O portao olha o MAIOR servidor, e nao o total.
    if closed_per_guild
        .values()
        .all(|&n| n <= MAX_CLOSED_PER_GUILD)
    {
        return;
    }

    let mut closed: std::collections::HashMap<u64, Vec<&Ticket>> = Default::default();
    for t in file.tickets.iter().filter(|t| !t.is_open()) {
        closed.entry(t.guild_id).or_default().push(t);
    }
    let mut keep: Vec<Ticket> = file
        .tickets
        .iter()
        .filter(|t| t.is_open())
        .cloned()
        .collect();
    for mut list in closed.into_values() {
        list.sort_by_key(|t| std::cmp::Reverse(t.closed_at_utc));
        keep.extend(list.into_iter().take(MAX_CLOSED_PER_GUILD).cloned());
    }
    keep.sort_by_key(|t| t.opened_at_utc);

    let dropped = file.tickets.len() - keep.len();
    file.tickets = keep;
    if dropped > 0 {
        tracing::warn!(
            "[tickets] podados {dropped} ticket(s) fechado(s); teto e {MAX_CLOSED_PER_GUILD} por servidor."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::{copy_real, scratch_dir};

    #[tokio::test]
    async fn ida_e_volta_do_arquivo_real_com_is_open() {
        let dir = scratch_dir("tickets");
        let Some(path) = copy_real(&dir, "tickets.json") else {
            return;
        };
        let original = std::fs::read_to_string(&path).unwrap();
        let store = TicketStore::open(path.clone()).await.unwrap();
        let file = store.read().await.unwrap();
        assert_eq!(json_file::to_text(&file).unwrap(), original);

        // Regravado pelo store: o mesmo JSON a menos do lastUpdatedUtc.
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
    async fn abre_numera_por_servidor_e_fecha() {
        let dir = scratch_dir("tickets-ops");
        let path = dir.join("tickets.json");
        let store = TicketStore::open(path.clone()).await.unwrap();

        let id = store
            .update(|edit| {
                let ticket = Ticket {
                    id: edit.file.new_unique_id(),
                    number: edit.file.next_number(5),
                    guild_id: 5,
                    channel_id: 50,
                    kind: "duvida".into(),
                    opened_at_utc: time::now(),
                    ..Ticket::default()
                };
                let id = ticket.id.clone();
                edit.file.tickets.push(ticket);
                edit.mark_changed();
                id
            })
            .await
            .unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"type\": \"duvida\""));
        assert!(text.contains("\"IsOpen\": true"));
        assert!(text.contains("\"5\": 1"));

        store
            .update(|edit| {
                let t = edit.file.tickets.iter_mut().find(|t| t.id == id).unwrap();
                t.closed_at_utc = Some(time::now());
                edit.mark_changed();
            })
            .await
            .unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("\"IsOpen\": false")
        );

        let ticket = store.by_id(5, &id.to_uppercase()).await.unwrap().unwrap();
        assert!(!ticket.is_open());
        assert_eq!(store.by_channel(50).await.unwrap().unwrap().number, 1);
        assert!(!ticket.is_closing(time::now()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
