//! O /logs (commands/BotLogs.cs): as ultimas linhas que o bot escreveu,
//! lidas do buffer. Evita precisar de acesso SSH a maquina para descobrir por
//! que alguma coisa falhou.

use chrono::Local;
use poise::serenity_prelude as serenity;
use serenity::CreateEmbed;

use super::commands::edit;
use crate::{
    Context, Error, embeds,
    log_buffer::{LogBuffer, LogLine, Severity},
};

/// 2800, e nao 3200: o `scope` e prefixado a TODA pagina e nunca era
/// contado, e a cerca de crases soma mais alguns. A descricao de um embed
/// para em 4096 e estourar derruba o /logs inteiro com "Command failed" -
/// justamente quando alguem esta tentando ler o log para entender um
/// incidente.
const MAX_CHARS_PER_PAGE: usize = 2800;

#[derive(Clone, Copy, Debug, PartialEq, Eq, poise::ChoiceParameter)]
pub enum Level {
    #[name = "All"]
    All,
    #[name = "Errors only"]
    Errors,
    #[name = "Warnings only"]
    Warnings,
    #[name = "Info only"]
    Info,
}

impl Level {
    fn severity(self) -> Option<Severity> {
        match self {
            Self::All => None,
            Self::Errors => Some(Severity::Erro),
            Self::Warnings => Some(Severity::Aviso),
            Self::Info => Some(Severity::Info),
        }
    }
}

// Os logs carregam ids de usuario, mensagens de erro e caminhos da maquina
// que hospeda o bot.
//
// `guild_only` nao e redundante com a permissao de administrador: ela nao
// vale em DM, e sem ele, com o registro global, qualquer um que pudesse
// mandar DM ao bot leria o buffer inteiro.
//
// Os dois juntos ainda param no "administrador DAQUELE servidor", e esse
// portao deixou de bastar quando o registro virou global: o buffer e do
// PROCESSO, com linhas de todos os servidores em que o bot esta, e qualquer
// pessoa que criasse um servidor e convidasse o bot seria administradora
// nele. Por isso a checagem de dono da aplicacao no corpo do comando.

/// Show the bot's latest logs
#[poise::command(
    slash_command,
    rename = "logs",
    guild_only,
    default_member_permissions = "ADMINISTRATOR",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn logs(
    ctx: Context<'_>,
    #[description = "How many lines to show (1–100, default 25)"] amount: Option<i64>,
    #[description = "Filter by severity"] level: Option<Level>,
    #[description = "Only show lines that contain this text"] filter: Option<String>,
    #[rename = "private"]
    #[description = "Only show the result to you"]
    private_: Option<bool>,
) -> Result<(), Error> {
    if !is_application_owner(ctx) {
        ctx.send(
            poise::CreateReply::default()
                .embed(embeds::error(
                    "Command denied",
                    "`/logs` shows the log for the **entire process** — what the bot did in \
                     every server it's in, including user IDs, exception messages and \
                     file paths on the host machine.\n\n\
                     That's why it's reserved for whoever runs **the bot**, not whoever runs \
                     a server.",
                ))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let private_ = private_.unwrap_or(true);
    if private_ {
        ctx.defer_ephemeral().await?;
    } else {
        ctx.defer().await?;
    }

    let take = amount.unwrap_or(25).clamp(1, 100) as usize;
    let level = level.unwrap_or(Level::All);
    let contains = filter
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(str::to_owned);

    let buffer: &LogBuffer = &ctx.data().logs;
    let lines = buffer.snapshot(take, level.severity(), contains.as_deref());
    let scope = scope_text(buffer, level, contains.as_deref(), take, lines.len());

    if lines.is_empty() {
        let description = if buffer.total_seen() == 0 {
            "No lines captured yet. The buffer starts empty every time the bot restarts.".to_owned()
        } else {
            format!("No lines match the filter.\n{scope}")
        };
        return edit(
            ctx,
            CreateEmbed::new()
                .title("Bot logs")
                .description(description)
                .colour(embeds::colors::ORANGE),
        )
        .await;
    }

    let rendered: Vec<String> = lines.iter().map(render_line).collect();
    let pages = build_pages(&rendered, &scope, MAX_CHARS_PER_PAGE);

    if pages.len() == 1 {
        return edit(ctx, pages.into_iter().next().unwrap_or_default()).await;
    }

    // So quem rodou o comando troca de pagina: o paginate responde ao clique
    // dos outros em vez de deixar o Discord dizer "interacao falhou".
    crate::paginate::send(ctx, pages, private_).await
}

/// Quem administra o BOT, e nao quem administra um servidor qualquer. O
/// poise preenche os donos na subida (dono da aplicacao ou membros
/// Admin/Developer do time).
///
/// Falha FECHADA: se a lista nao foi preenchida, a resposta e "nao", pela
/// mesma razao da checagem de hierarquia - o modo de errar aqui e entregar o
/// log da maquina a quem nao devia.
fn is_application_owner(ctx: Context<'_>) -> bool {
    ctx.framework().options().owners.contains(&ctx.author().id)
}

fn scope_text(
    buffer: &LogBuffer,
    level: Level,
    contains: Option<&str>,
    take: usize,
    shown: usize,
) -> String {
    let counts = buffer.counts();
    describe_scope(
        level.severity(),
        contains,
        take,
        shown,
        (counts.info, counts.aviso, counts.erro),
    )
}

/// Mostra o nome exibido do nivel (ERROR/WARN/INFO), e nao o valor interno
/// da escolha.
fn describe_scope(
    severity: Option<Severity>,
    contains: Option<&str>,
    take: usize,
    shown: usize,
    (info, warn, error): (usize, usize, usize),
) -> String {
    let mut parts = Vec::new();
    if let Some(severity) = severity {
        parts.push(format!("level: `{}`", severity.name()));
    }
    if let Some(contains) = contains {
        parts.push(format!("text: `{}`", embeds::trim(contains, 40)));
    }

    let buffer = format!("Buffer: {info} info · {warn} warn · {error} error.");
    if parts.is_empty() {
        format!("Latest {shown} of up to {take} line(s). {buffer}")
    } else {
        format!("Filter — {} · {shown} line(s). {buffer}", parts.join(" · "))
    }
}

/// "hora NIVEL texto", ja escapado. Escapa ANTES de medir: escapando
/// depois, cada ``` virava cinco caracteres e uma pagina cheia passava do
/// teto do embed. E linha de log com ``` chega ate aqui: uma mensagem de erro
/// da API pode ecoar o `reason` que alguem digitou.
fn render_line(line: &LogLine) -> String {
    let stamp = line.at.with_timezone(&Local).format("%H:%M:%S");
    embeds::fence_safe(&format!(
        "[{stamp}] {:<5} {}\n",
        line.severity.name(),
        line.text
    ))
}

/// Uma linha por entrada, em bloco de codigo para o alinhamento sobreviver.
fn build_pages(lines: &[String], scope: &str, max_chars: usize) -> Vec<CreateEmbed> {
    let chunks = chunk_lines(lines, max_chars);
    let total = chunks.len();
    chunks
        .into_iter()
        .enumerate()
        .map(|(i, chunk)| {
            CreateEmbed::new()
                .title("Bot logs")
                .description(format!("{scope}\n```\n{chunk}```"))
                .colour(embeds::colors::BLURPLE)
                .footer(serenity::CreateEmbedFooter::new(format!(
                    "Page {}/{total} — newest first",
                    i + 1
                )))
                .timestamp(serenity::Timestamp::now())
        })
        .collect()
}

/// Junta linhas ate o teto (contado em UTF-16, como o Discord).
fn chunk_lines(lines: &[String], max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut length = 0;
    for line in lines {
        let size = embeds::utf16_len(line);
        if length + size > max_chars && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            length = 0;
        }
        current.push_str(line);
        length += size;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escopo_como_o_csharp() {
        assert_eq!(
            describe_scope(None, None, 25, 3, (1, 2, 3)),
            "Latest 3 of up to 25 line(s). Buffer: 1 info · 2 warn · 3 error."
        );
        assert_eq!(
            describe_scope(Some(Severity::Erro), Some("tick"), 25, 2, (0, 0, 2)),
            "Filter — level: `ERROR` · text: `tick` · 2 line(s). Buffer: 0 info · 0 warn · 2 error."
        );
    }

    #[test]
    fn paginas_respeitam_o_teto() {
        let lines: Vec<String> = (0..10)
            .map(|i| format!("{i}{}\n", "x".repeat(98)))
            .collect();
        let chunks = chunk_lines(&lines, 250);
        assert_eq!(chunks.len(), 5);
        assert!(chunks.iter().all(|c| embeds::utf16_len(c) <= 250));
        assert_eq!(chunks.concat(), lines.concat());
        // Uma linha maior que o teto ainda sai, sozinha, sem pagina vazia.
        assert_eq!(chunk_lines(&["y".repeat(300)], 250).len(), 1);
    }

    #[test]
    fn linha_com_cerca_e_escapada() {
        let line = LogLine {
            at: chrono::Utc::now(),
            severity: Severity::Aviso,
            tag: String::new(),
            text: "a ```b".into(),
        };
        let text = render_line(&line);
        assert!(text.contains(" WARN  a `\u{200b}`\u{200b}`b\n"));
        assert!(!text.contains("```"));
    }

    #[test]
    fn comando_de_administrador_so_em_servidor() {
        let command = logs();
        assert_eq!(command.name, "logs");
        assert!(command.guild_only);
        assert!(command.default_member_permissions.administrator());
        let names: Vec<_> = command.parameters.iter().map(|p| &*p.name).collect();
        assert_eq!(names, ["amount", "level", "filter", "private"]);
    }
}
