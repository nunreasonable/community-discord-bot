//! Os embeds de progresso e o relatorio final da DM em massa. Tudo puro:
//! recebe a contagem e devolve um `Report`, que os testes leem campo a campo
//! (o CreateEmbed do serenity nao expoe nada) e que vira embed no fim.
//!
//! Os limites que importam sao os do Discord: 1024 por campo e 6000 no
//! embed inteiro. Passar de qualquer um faz a edicao inteira ser recusada, e
//! a mensagem de progresso ficaria parada no ultimo numero.

use std::time::Duration;

use poise::serenity_prelude as serenity;

use crate::embeds::{self, colors, trim, utf16_len};

pub const FIELD_LIMIT: usize = 1024;
pub const EMBED_LIMIT: usize = 6000;
/// Largura da barra de progresso, em blocos.
const BAR_WIDTH: usize = 20;
/// Nome de pessoa dentro de uma lista: corta antes, para um nome enorme nao
/// comer a lista inteira.
const NAME_MAX: usize = 32;

/// Por que uma DM nao chegou. A ordem e a dos campos no relatorio.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FailKind {
    Closed,
    RateLimited,
    NotFound,
    Other,
}

impl FailKind {
    fn label(self) -> &'static str {
        match self {
            Self::Closed => "DMs off for this server, or bot blocked",
            Self::RateLimited => "Rate limited — stopped here",
            Self::NotFound => "User/member not found",
            Self::Other => "Other errors",
        }
    }
}

/// Classifica pelo status HTTP e pelo codigo JSON do Discord.
/// 50007 = "Cannot send messages to this user"; 40003 = "opening direct
/// messages too fast"; 10013/10007 = usuario/membro desconhecido.
pub fn classify(status: Option<u16>, code: Option<isize>) -> FailKind {
    match (status, code) {
        // 50278 ("no mutual guilds") aparece mesmo com a pessoa no servidor:
        // e o que o Discord responde quando ela desligou as DMs de membros
        // DAQUELE servidor - o servidor deixa de contar como "em comum".
        (_, Some(50007 | 50278)) | (Some(403), _) => FailKind::Closed,
        (_, Some(40003)) | (Some(429), _) => FailKind::RateLimited,
        (_, Some(10013 | 10007)) | (Some(404), _) => FailKind::NotFound,
        _ => FailKind::Other,
    }
}

#[derive(Clone, Debug)]
pub struct Failure {
    pub name: String,
    pub kind: FailKind,
    /// O erro curto; so aparece no grupo "Other errors".
    pub detail: String,
}

/// A contagem de um envio, viva durante o job.
#[derive(Clone, Debug, Default)]
pub struct Tally {
    /// Como aparece no campo Target (mencao: em embed nao notifica ninguem).
    pub target: String,
    pub total: usize,
    pub delivered: Vec<String>,
    pub failures: Vec<Failure>,
}

impl Tally {
    pub fn sent(&self) -> usize {
        self.delivered.len()
    }

    pub fn failed(&self) -> usize {
        self.failures.len()
    }

    pub fn remaining(&self) -> usize {
        self.total.saturating_sub(self.sent() + self.failed())
    }
}

/// Como o envio terminou.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Finished,
    Cancelled {
        by: u64,
    },
    /// Rate limit ou falhas demais em sequencia; o texto e o motivo.
    Aborted(String),
    Shutdown,
}

/// Um embed como dados. `to_embed` monta o do serenity.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub title: String,
    pub colour: serenity::Colour,
    pub description: String,
    pub fields: Vec<(String, String, bool)>,
    pub footer: String,
}

impl Report {
    /// Tamanho como o Discord soma para o teto de 6000.
    pub fn len(&self) -> usize {
        utf16_len(&self.title)
            + utf16_len(&self.description)
            + utf16_len(&self.footer)
            + self
                .fields
                .iter()
                .map(|(name, value, _)| utf16_len(name) + utf16_len(value))
                .sum::<usize>()
    }

    pub fn to_embed(&self) -> serenity::CreateEmbed {
        let mut embed = serenity::CreateEmbed::new()
            .title(&self.title)
            .colour(self.colour)
            .timestamp(serenity::Timestamp::now());
        if !self.description.is_empty() {
            embed = embed.description(&self.description);
        }
        for (name, value, inline) in &self.fields {
            embed = embed.field(name, value, *inline);
        }
        if !self.footer.is_empty() {
            embed = embed.footer(serenity::CreateEmbedFooter::new(&self.footer));
        }
        embed
    }
}

/// "`[██████░░░░]` 60%"
pub fn progress_bar(done: usize, total: usize) -> String {
    // Total zero conta como pronto: nao ha o que mandar.
    let done = done.min(total);
    let filled = (done * BAR_WIDTH).checked_div(total).unwrap_or(BAR_WIDTH);
    let percent = (done * 100).checked_div(total).unwrap_or(100);
    format!(
        "`[{}{}]` {percent}%",
        "█".repeat(filled),
        "░".repeat(BAR_WIDTH - filled)
    )
}

/// "38 s", "4 min 12 s", "1 h 5 min".
pub fn duration_text(span: Duration) -> String {
    let secs = span.as_secs();
    match secs {
        0..60 => format!("{secs} s"),
        60..3600 => format!("{} min {} s", secs / 60, secs % 60),
        _ => format!("{} h {} min", secs / 3600, (secs % 3600) / 60),
    }
}

/// Estimativa grosseira: o que falta vezes o custo de uma DM no ritmo do
/// limitador.
pub fn eta_text(remaining: usize, per_dm: Duration) -> String {
    if remaining == 0 {
        return "—".to_owned();
    }
    let secs = per_dm.as_secs_f64() * remaining as f64;
    if secs < 60.0 {
        format!("~{} s", secs.ceil() as u64)
    } else {
        format!("~{} min", (secs / 60.0).ceil() as u64)
    }
}

/// O embed enquanto envia.
pub fn progress(tally: &Tally, started_unix: i64, per_dm: Duration, every: Duration) -> Report {
    let done = tally.sent() + tally.failed();
    Report {
        title: "📨 Sending DMs…".to_owned(),
        colour: colors::BLURPLE,
        description: format!(
            "{}\nStarted <t:{started_unix}:R> · updates every ~{} s",
            progress_bar(done, tally.total),
            every.as_secs()
        ),
        fields: vec![
            ("Target".to_owned(), tally.target.clone(), true),
            ("Total".to_owned(), embeds::number(tally.total as i64), true),
            ("Sent".to_owned(), embeds::number(tally.sent() as i64), true),
            (
                "Failed".to_owned(),
                embeds::number(tally.failed() as i64),
                true,
            ),
            (
                "Remaining".to_owned(),
                embeds::number(tally.remaining() as i64),
                true,
            ),
            ("ETA".to_owned(), eta_text(tally.remaining(), per_dm), true),
        ],
        footer: "Only the person who started this or an Administrator can cancel it.".to_owned(),
    }
}

/// Titulo e cor pelo desfecho: cancelado cinza; ninguem recebeu vermelho;
/// tudo certo verde; parcial ou interrompido laranja.
pub fn verdict(tally: &Tally, outcome: &Outcome) -> (&'static str, serenity::Colour) {
    match outcome {
        Outcome::Cancelled { .. } => ("⏹️ Sending cancelled", colors::GRAY),
        _ if tally.sent() == 0 && tally.failed() > 0 => {
            ("❌ Nobody received the DM", colors::INDIAN_RED)
        }
        Outcome::Finished if tally.failed() == 0 => ("✅ DMs sent", colors::SPRING_GREEN),
        Outcome::Finished => ("⚠️ DMs sent, some failed", colors::ORANGE),
        Outcome::Aborted(_) => ("⚠️ Sending stopped early", colors::ORANGE),
        Outcome::Shutdown => ("⚠️ Sending interrupted", colors::ORANGE),
    }
}

/// O relatorio final, que substitui o embed de progresso.
pub fn final_report(tally: &Tally, outcome: &Outcome, elapsed: Duration) -> Report {
    let (title, colour) = verdict(tally, outcome);

    let mut lines = Vec::new();
    match outcome {
        Outcome::Finished => {}
        Outcome::Cancelled { by } => lines.push(format!("Cancelled by <@{by}>.")),
        Outcome::Aborted(reason) => lines.push(reason.clone()),
        Outcome::Shutdown => {
            lines.push("The bot was shutting down, so sending stopped.".to_owned());
        }
    }
    let never = tally.remaining();
    if never > 0 && *outcome != Outcome::Finished {
        lines.push(format!(
            "{} member(s) were never attempted.",
            embeds::number(never as i64)
        ));
    }
    if tally.sent() == 0 && tally.failed() > 0 {
        lines.push(
            "Nobody received it. Members need to allow DMs from server members (User Settings › Privacy & Safety, or this server's Privacy Settings)."
                .to_owned(),
        );
    }

    let mut report = Report {
        title: title.to_owned(),
        colour,
        description: lines.join("\n"),
        fields: vec![
            ("Target".to_owned(), tally.target.clone(), true),
            ("Total".to_owned(), embeds::number(tally.total as i64), true),
            ("Sent".to_owned(), embeds::number(tally.sent() as i64), true),
            (
                "Failed".to_owned(),
                embeds::number(tally.failed() as i64),
                true,
            ),
            ("Duration".to_owned(), duration_text(elapsed), true),
        ],
        footer: String::new(),
    };

    // As listas: entregues e uma por motivo de falha.
    let mut lists: Vec<(String, Vec<String>)> = Vec::new();
    if !tally.delivered.is_empty() {
        lists.push((
            format!("Delivered ({})", tally.sent()),
            tally
                .delivered
                .iter()
                .map(|n| embeds::plain(n, NAME_MAX))
                .collect(),
        ));
    }
    for kind in [
        FailKind::Closed,
        FailKind::RateLimited,
        FailKind::NotFound,
        FailKind::Other,
    ] {
        let group: Vec<String> = tally
            .failures
            .iter()
            .filter(|f| f.kind == kind)
            .map(|f| {
                let name = embeds::plain(&f.name, NAME_MAX);
                if kind == FailKind::Other && !f.detail.is_empty() {
                    format!("{name} ({})", embeds::plain(&f.detail, 80))
                } else {
                    name
                }
            })
            .collect();
        if !group.is_empty() {
            lists.push((format!("{} ({})", kind.label(), group.len()), group));
        }
    }

    // O que sobra do teto de 6000 depois do resto, dividido entre as listas.
    let fixed = report.len() + lists.iter().map(|(n, _)| utf16_len(n)).sum::<usize>();
    let budget = EMBED_LIMIT.saturating_sub(fixed);
    let natural: Vec<usize> = lists
        .iter()
        .map(|(_, items)| utf16_len(&name_list(items, FIELD_LIMIT)))
        .collect();
    let caps = share(&natural, budget);
    for ((name, items), cap) in lists.into_iter().zip(caps) {
        let value = name_list(&items, cap);
        if !value.is_empty() {
            report.fields.push((name, value, false));
        }
    }
    report
}

/// Divide `budget` entre listas que querem `wants`: quem quer menos que a
/// parte igual leva o que quer, e o resto e dividido entre os outros.
pub fn share(wants: &[usize], budget: usize) -> Vec<usize> {
    let mut caps = vec![0; wants.len()];
    let mut open: Vec<usize> = (0..wants.len()).collect();
    let mut left = budget;
    loop {
        if open.is_empty() {
            return caps;
        }
        let even = left / open.len();
        let (small, big): (Vec<usize>, Vec<usize>) = open.iter().partition(|&&i| wants[i] <= even);
        if small.is_empty() {
            for i in big {
                caps[i] = even;
            }
            return caps;
        }
        for i in small {
            caps[i] = wants[i];
            left -= wants[i];
        }
        open = big;
    }
}

/// "a, b, c … and 12 more", no maximo `max` unidades UTF-16. Os itens ja
/// vem escapados.
pub fn name_list(items: &[String], max: usize) -> String {
    let full = items.join(", ");
    if utf16_len(&full) <= max {
        return full;
    }
    // A reserva usa o total: o "and N more" de verdade nunca e maior.
    let reserve = utf16_len(&format!(" … and {} more", items.len()));
    let mut out = String::new();
    for (shown, item) in items.iter().enumerate() {
        let candidate = if out.is_empty() {
            item.clone()
        } else {
            format!("{out}, {item}")
        };
        if utf16_len(&candidate) + reserve > max {
            let more = items.len() - shown;
            return if out.is_empty() {
                trim(&format!("… and {more} more"), max)
            } else {
                format!("{out} … and {more} more")
            };
        }
        out = candidate;
    }
    out
}

/// O texto da DM do /role strip (o punishment_dm do ccore), dizendo de qual
/// servidor: a pessoa pode estar em varios com este bot.
pub fn roles_removed_text(server: &str, reason: Option<&str>) -> String {
    let mut text = format!(
        "Your roles in **{}** were removed by the server's staff.",
        embeds::plain(server, 100)
    );
    if let Some(reason) = reason.filter(|r| !r.trim().is_empty()) {
        text.push_str(&format!("\nReason: {}", trim(reason, 1000)));
    }
    text
}

/// A DM do /dm. Diz com todas as letras de qual servidor veio e que foi a
/// staff: o bot nao fala em nome de ninguem.
pub fn dm_embed(server: &str, icon: Option<String>, text: &str) -> serenity::CreateEmbed {
    let mut embed = serenity::CreateEmbed::new()
        .title(trim(
            &format!("Message from the staff of {}", embeds::plain(server, 100)),
            256,
        ))
        .colour(colors::BLURPLE)
        .field("Message", embeds::safe(text), false)
        .footer(serenity::CreateEmbedFooter::new(trim(
            &format!("Sent by the staff of {server} through this bot. Replies here aren't read."),
            2048,
        )))
        .timestamp(serenity::Timestamp::now());
    if let Some(icon) = icon {
        embed = embed.thumbnail(icon);
    }
    embed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tally(total: usize, delivered: usize, failures: Vec<Failure>) -> Tally {
        Tally {
            target: "<@&9>".to_owned(),
            total,
            delivered: (0..delivered).map(|i| format!("user{i}")).collect(),
            failures,
        }
    }

    fn fail(name: &str, kind: FailKind) -> Failure {
        Failure {
            name: name.to_owned(),
            kind,
            detail: "boom".to_owned(),
        }
    }

    fn field<'a>(report: &'a Report, prefix: &str) -> Option<&'a str> {
        report
            .fields
            .iter()
            .find(|(n, _, _)| n.starts_with(prefix))
            .map(|(_, v, _)| v.as_str())
    }

    #[test]
    fn classifica_erros_do_discord() {
        assert_eq!(classify(Some(403), Some(50007)), FailKind::Closed);
        assert_eq!(classify(Some(400), Some(50007)), FailKind::Closed);
        assert_eq!(classify(Some(429), None), FailKind::RateLimited);
        assert_eq!(classify(Some(400), Some(50278)), FailKind::Closed);
        assert_eq!(classify(Some(400), Some(40003)), FailKind::RateLimited);
        assert_eq!(classify(Some(404), Some(10013)), FailKind::NotFound);
        assert_eq!(classify(Some(400), Some(10007)), FailKind::NotFound);
        assert_eq!(classify(Some(500), Some(0)), FailKind::Other);
        assert_eq!(classify(None, None), FailKind::Other);
    }

    #[test]
    fn barra_e_tempos() {
        assert_eq!(progress_bar(0, 10), format!("`[{}]` 0%", "░".repeat(20)));
        assert_eq!(
            progress_bar(5, 10),
            format!("`[{}{}]` 50%", "█".repeat(10), "░".repeat(10))
        );
        assert_eq!(progress_bar(12, 10), format!("`[{}]` 100%", "█".repeat(20)));
        assert_eq!(duration_text(Duration::from_secs(38)), "38 s");
        assert_eq!(duration_text(Duration::from_secs(252)), "4 min 12 s");
        assert_eq!(duration_text(Duration::from_secs(3900)), "1 h 5 min");
        assert_eq!(eta_text(0, Duration::from_secs(3)), "—");
        assert_eq!(eta_text(10, Duration::from_secs(3)), "~30 s");
        assert_eq!(eta_text(100, Duration::from_millis(3500)), "~6 min");
    }

    #[test]
    fn progresso_tem_os_numeros() {
        let t = tally(10, 3, vec![fail("x", FailKind::Closed)]);
        let r = progress(
            &t,
            1700000000,
            Duration::from_secs(3),
            Duration::from_secs(10),
        );
        assert_eq!(field(&r, "Sent"), Some("3"));
        assert_eq!(field(&r, "Failed"), Some("1"));
        assert_eq!(field(&r, "Remaining"), Some("6"));
        assert_eq!(field(&r, "ETA"), Some("~18 s"));
        assert!(r.description.contains("40%") && r.description.contains("<t:1700000000:R>"));
    }

    #[test]
    fn titulo_e_cor_pelo_desfecho() {
        let ok = tally(2, 2, vec![]);
        let partial = tally(2, 1, vec![fail("a", FailKind::Closed)]);
        let nobody = tally(
            2,
            0,
            vec![fail("a", FailKind::Closed), fail("b", FailKind::Other)],
        );
        assert_eq!(verdict(&ok, &Outcome::Finished).1, colors::SPRING_GREEN);
        assert_eq!(verdict(&partial, &Outcome::Finished).1, colors::ORANGE);
        assert_eq!(verdict(&nobody, &Outcome::Finished).1, colors::INDIAN_RED);
        assert_eq!(
            verdict(&nobody, &Outcome::Cancelled { by: 1 }).1,
            colors::GRAY
        );
        assert_eq!(
            verdict(&partial, &Outcome::Aborted("x".into())).1,
            colors::ORANGE
        );
        // Parou antes de tentar alguem: interrompido, nao "ninguem recebeu".
        assert_eq!(
            verdict(&tally(5, 0, vec![]), &Outcome::Shutdown).0,
            "⚠️ Sending interrupted"
        );
    }

    #[test]
    fn relatorio_agrupa_por_motivo() {
        let t = tally(
            10,
            2,
            vec![
                fail("a", FailKind::Closed),
                fail("b", FailKind::Other),
                fail("c", FailKind::Closed),
                fail("d", FailKind::RateLimited),
            ],
        );
        let r = final_report(
            &t,
            &Outcome::Aborted("Discord signalled a rate limit.".into()),
            Duration::from_secs(70),
        );
        assert_eq!(field(&r, "Delivered (2)"), Some("user0, user1"));
        assert_eq!(
            field(&r, "DMs off for this server, or bot blocked (2)"),
            Some("a, c")
        );
        assert_eq!(field(&r, "Rate limited — stopped here (1)"), Some("d"));
        assert_eq!(field(&r, "Other errors (1)"), Some("b (boom)"));
        assert!(field(&r, "User/member not found").is_none());
        assert_eq!(field(&r, "Duration"), Some("1 min 10 s"));
        assert!(r.description.contains("rate limit"));
        assert!(r.description.contains("4 member(s) were never attempted."));
        // A ordem dos grupos e fixa.
        let names: Vec<&str> = r.fields.iter().map(|(n, _, _)| n.as_str()).collect();
        let closed = names
            .iter()
            .position(|n| n.starts_with("DMs off"))
            .unwrap();
        let other = names.iter().position(|n| n.starts_with("Other")).unwrap();
        assert!(closed < other);
    }

    #[test]
    fn dica_de_privacidade_so_quando_ninguem_recebeu() {
        let nobody = tally(1, 0, vec![fail("a", FailKind::Closed)]);
        let some = tally(2, 1, vec![fail("a", FailKind::Closed)]);
        let r = final_report(&nobody, &Outcome::Finished, Duration::ZERO);
        assert!(r.description.contains("Privacy"));
        let r = final_report(&some, &Outcome::Finished, Duration::ZERO);
        assert!(!r.description.contains("Privacy"));
        assert!(!r.description.contains("never attempted"));
    }

    #[test]
    fn lista_cortada_com_e_mais() {
        let items: Vec<String> = (0..10).map(|i| format!("name{i}")).collect();
        assert_eq!(name_list(&items[..2], 100), "name0, name1");
        let cut = name_list(&items, 30);
        assert!(utf16_len(&cut) <= 30, "{cut}");
        assert!(cut.starts_with("name0, name1") && cut.ends_with("more"));
        let more: usize = cut
            .rsplit("and ")
            .next()
            .unwrap()
            .trim_end_matches(" more")
            .parse()
            .unwrap();
        let shown = cut.split(" … ").next().unwrap().split(", ").count();
        assert_eq!(shown + more, 10);
        assert!(name_list(&items, 5).chars().count() <= 5);
    }

    #[test]
    fn divide_o_orcamento() {
        assert_eq!(share(&[10, 20], 100), [10, 20]);
        assert_eq!(share(&[10, 500, 500], 410), [10, 200, 200]);
        assert_eq!(share(&[], 10), Vec::<usize>::new());
    }

    #[test]
    fn respeita_1024_por_campo_e_6000_no_total() {
        // 500 nomes longos em cada grupo: sem corte passaria dos dois tetos.
        let long = |p: &str, i: usize| format!("{p}{i}_{}", "x".repeat(25));
        let mut t = Tally {
            target: "<@&9>".into(),
            total: 2000,
            delivered: (0..500).map(|i| long("ok", i)).collect(),
            failures: Vec::new(),
        };
        for (kind, p) in [
            (FailKind::Closed, "c"),
            (FailKind::NotFound, "n"),
            (FailKind::Other, "o"),
            (FailKind::RateLimited, "r"),
        ] {
            for i in 0..(if kind == FailKind::RateLimited {
                1
            } else {
                400
            }) {
                t.failures.push(Failure {
                    name: long(p, i),
                    kind,
                    detail: "Some long HTTP error text that keeps going".into(),
                });
            }
        }
        let r = final_report(&t, &Outcome::Aborted("stop".into()), Duration::from_secs(9));
        assert!(r.len() <= EMBED_LIMIT, "{}", r.len());
        for (name, value, _) in &r.fields {
            assert!(utf16_len(value) <= FIELD_LIMIT, "{name}");
            assert!(!value.is_empty());
        }
        assert!(field(&r, "Delivered (500)").unwrap().ends_with("more"));
        assert_eq!(
            field(&r, "Rate limited — stopped here (1)").map(|v| v.starts_with('r')),
            Some(true)
        );
    }

    #[test]
    fn nomes_sao_escapados() {
        let t = Tally {
            target: "<@9>".into(),
            total: 1,
            delivered: vec!["**bold**_x".into()],
            failures: Vec::new(),
        };
        let r = final_report(&t, &Outcome::Finished, Duration::ZERO);
        assert_eq!(field(&r, "Delivered"), Some(r"\*\*bold\*\*\_x"));
    }

    #[test]
    fn textos_da_dm() {
        let text = roles_removed_text("My *Server*", Some("spam"));
        assert!(text.contains(r"**My \*Server\***") && text.ends_with("Reason: spam"));
        assert!(!roles_removed_text("S", Some("  ")).contains("Reason"));
        let embed = serde_json::to_value(dm_embed("Guild", None, "[x](http://e)")).unwrap();
        assert_eq!(embed["title"], "Message from the staff of Guild");
        assert_eq!(embed["fields"][0]["value"], r"\[x](http://e)");
    }
}
