//! As ultimas linhas de log do processo, em memoria, para o /logs
//! (BotLogBuffer.cs). O stdout continua recebendo tudo pelo fmt do tracing.
//!
//! No C# isto era um tee do console com palavras-chave para adivinhar o
//! nivel. Aqui o nivel do tracing vale, e a mesma classificacao por
//! palavra-chave fica por cima: uma linha portada como
//! `info!("[gitfeed] aviso: ...")` continua aparecendo como AVISO, igual ao C#.

use std::{
    collections::VecDeque,
    fmt::{Debug, Write},
    sync::{Arc, Mutex, MutexGuard},
};

use chrono::{DateTime, Utc};
use tracing::{
    Event, Level, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::layer::{Context, Layer};

use crate::embeds::trim;

/// Mil linhas cobrem o que se quer olhar depois de um incidente.
pub const CAPACITY: usize = 1000;
/// Um stack trace inteiro numa linha so encheria o buffer sozinho.
const MAX_LINE_LENGTH: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Aviso,
    Erro,
}

impl Severity {
    /// Nome curto, o que o /logs mostra.
    pub fn name(self) -> &'static str {
        match self {
            Self::Erro => "ERROR",
            Self::Aviso => "WARN",
            Self::Info => "INFO",
        }
    }

    /// Texto do usuario num nivel. None = todos.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_lowercase().as_str() {
            "erro" | "error" => Some(Self::Erro),
            "aviso" | "warn" | "warning" => Some(Self::Aviso),
            "info" => Some(Self::Info),
            _ => None,
        }
    }

    /// A classificacao por palavra-chave do C#. Erra para o lado de INFO.
    pub fn classify(text: &str) -> Self {
        let lower = text.to_lowercase();
        let has = |needle: &str| lower.contains(needle);

        if has("[fatal]")
            || has("[unobserved]")
            || has("erro")
            || has("falha")
            || has("exception")
            || has("unhandled")
            || has("disjuntor")
        {
            return Self::Erro;
        }
        if has("[pool]")
            || has("zumbi")
            || has("lento")
            || has("socket fechado")
            || has("descartad")
            || has("aviso")
        {
            return Self::Aviso;
        }
        Self::Info
    }
}

#[derive(Clone, Debug)]
pub struct LogLine {
    pub at: DateTime<Utc>,
    pub severity: Severity,
    /// A etiqueta "[tag]" do comeco da linha, ou vazio.
    pub tag: String,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub info: usize,
    pub aviso: usize,
    pub erro: usize,
}

#[derive(Default)]
struct Inner {
    lines: VecDeque<LogLine>,
    total_seen: u64,
}

#[derive(Default)]
pub struct LogBuffer {
    inner: Mutex<Inner>,
}

impl LogBuffer {
    /// Um panic com o lock na mao "envenena" o Mutex. Log nao pode derrubar
    /// quem esta logando, entao segue com o conteudo que estiver la.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Registra uma linha. Nunca falha.
    pub fn append(&self, severity: Severity, text: &str) {
        let text = text.trim_end_matches(['\r', '\n']);
        if text.trim().is_empty() {
            return;
        }

        let text = trim(text, MAX_LINE_LENGTH);
        let line = LogLine {
            at: Utc::now(),
            severity,
            tag: extract_tag(&text),
            text,
        };

        let mut inner = self.lock();
        if inner.lines.len() >= CAPACITY {
            inner.lines.pop_front();
        }
        inner.lines.push_back(line);
        inner.total_seen += 1;
    }

    /// Da linha mais recente para a mais antiga.
    pub fn snapshot(
        &self,
        take: usize,
        severity: Option<Severity>,
        contains: Option<&str>,
    ) -> Vec<LogLine> {
        let needle = contains
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_lowercase);

        self.lock()
            .lines
            .iter()
            .rev()
            .filter(|line| severity.is_none_or(|s| line.severity == s))
            .filter(|line| {
                needle
                    .as_deref()
                    .is_none_or(|n| line.text.to_lowercase().contains(n))
            })
            .take(take.clamp(1, CAPACITY))
            .cloned()
            .collect()
    }

    pub fn counts(&self) -> Counts {
        let mut counts = Counts::default();
        for line in &self.lock().lines {
            match line.severity {
                Severity::Info => counts.info += 1,
                Severity::Aviso => counts.aviso += 1,
                Severity::Erro => counts.erro += 1,
            }
        }
        counts
    }

    /// Total de linhas capturadas desde que o processo subiu.
    pub fn total_seen(&self) -> u64 {
        self.lock().total_seen
    }
}

/// "[gw] ..." -> "gw". Vazio quando a linha nao segue a convencao.
fn extract_tag(text: &str) -> String {
    let Some(rest) = text.strip_prefix('[') else {
        return String::new();
    };
    match rest.find(']') {
        Some(end) if (1..24).contains(&end) => rest[..end].to_owned(),
        _ => String::new(),
    }
}

/// Faz cada evento do `tracing` cair tambem no buffer.
pub struct BufferLayer {
    buffer: Arc<LogBuffer>,
}

impl BufferLayer {
    pub fn new(buffer: Arc<LogBuffer>) -> Self {
        Self { buffer }
    }
}

impl<S: Subscriber> Layer<S> for BufferLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let mut visitor = FieldText::default();
        event.record(&mut visitor);

        // O que vem de dependencia leva a origem, como as tags [gw] do C#.
        let target = metadata.target();
        let text = if target == "sollarety" || target.starts_with("sollarety::") {
            visitor.0
        } else {
            format!("[{target}] {}", visitor.0)
        };

        let by_words = Severity::classify(&text);
        let severity = match *metadata.level() {
            Level::ERROR => Severity::Erro,
            Level::WARN => by_words.max(Severity::Aviso),
            _ => by_words,
        };
        self.buffer.append(severity, &text);
    }
}

#[derive(Default)]
struct FieldText(String);

impl Visit for FieldText {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        let _ = if field.name() == "message" {
            write!(self.0, "{value:?}")
        } else {
            write!(self.0, " {}={value:?}", field.name())
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descarta_a_linha_mais_antiga_quando_enche() {
        let buffer = LogBuffer::default();
        for i in 0..CAPACITY + 1 {
            buffer.append(Severity::Info, &format!("linha {i}"));
        }

        let lines = buffer.snapshot(CAPACITY + 10, None, None);
        assert_eq!(lines.len(), CAPACITY);
        assert_eq!(lines[0].text, format!("linha {CAPACITY}"));
        assert_eq!(lines[CAPACITY - 1].text, "linha 1");
        assert_eq!(buffer.total_seen(), CAPACITY as u64 + 1);
    }

    #[test]
    fn filtra_por_nivel_e_texto_sem_diferenciar_caixa() {
        let buffer = LogBuffer::default();
        buffer.append(Severity::Info, "tudo CERTO");
        buffer.append(Severity::Erro, "deu ruim");
        buffer.append(Severity::Erro, "certo que falhou");

        let lines = buffer.snapshot(10, Some(Severity::Erro), Some("certo"));
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "certo que falhou");
        assert_eq!(
            buffer.counts(),
            Counts {
                info: 1,
                aviso: 0,
                erro: 2
            }
        );
        assert_eq!(buffer.snapshot(0, None, None).len(), 1);
    }

    #[test]
    fn classifica_e_extrai_a_etiqueta_como_o_csharp() {
        assert_eq!(Severity::classify("[gitfeed] aviso: x"), Severity::Aviso);
        assert_eq!(
            Severity::classify("[modlog] falha ao registrar"),
            Severity::Erro
        );
        assert_eq!(
            Severity::classify("[pool] thread pool atrasou 3.0s"),
            Severity::Aviso
        );
        assert_eq!(Severity::classify("Bot pronto"), Severity::Info);
        assert_eq!(extract_tag("[gw] socket"), "gw");
        assert_eq!(extract_tag("[] x"), "");
        assert_eq!(extract_tag("sem tag"), "");
        assert_eq!(Severity::parse(" Warning "), Some(Severity::Aviso));
        assert_eq!(Severity::parse("todos"), None);
    }

    #[test]
    fn ignora_linha_vazia_e_corta_a_longa() {
        let buffer = LogBuffer::default();
        buffer.append(Severity::Info, "  \n");
        assert_eq!(buffer.total_seen(), 0);
        buffer.append(Severity::Info, &"x".repeat(900));
        assert_eq!(
            buffer.snapshot(1, None, None)[0].text.chars().count(),
            MAX_LINE_LENGTH
        );
    }
}
