//! Feed de commits (GitFeed.cs): olha um repositorio git desta maquina e
//! posta num canal o resumo de cada commit novo, com o .patch em anexo.
//!
//! Por varredura, e nao por hook de post-commit: o hook dispara uma vez so,
//! e um commit feito com o bot fora do ar (ou com a rede caindo) sumiria.
//! Aqui o ultimo commit postado fica em disco, e a volta seguinte posta o que
//! faltou.
//!
//! So LE o repositorio. Nada de fetch, checkout ou qualquer coisa que pegue
//! lock: quem commita la nao pode tropecar no bot.

use std::{
    path::{Component, Path, PathBuf},
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use poise::serenity_prelude as serenity;
use serenity::{ChannelId, CreateEmbed, GuildId, Permissions};
use tokio::sync::watch;
use tracing::{error, info, warn};

use crate::{
    Error,
    config::{self, GitFeedSettings},
    embeds, paths, process,
    store::git_feed::{self as state_file, GitFeedState},
};

const MAX_FILES_LISTED: usize = 15;
const MAX_COMMITS_PER_ROUND: usize = 10;

/// Abaixo dos 10 MB de anexo do Discord, com folga para o resto da
/// requisicao.
const MAX_PATCH_BYTES: usize = 8 * 1024 * 1024;

const COLOR: serenity::Colour = serenity::Colour::new(0xB5651D);

/// O cabecalho do "fuller", mas SEM e-mail: o --format=fuller poe
/// "Author: nome <email>" e "Commit: nome <email>" no .patch, e o e-mail do
/// dono do repositorio nao tem o que fazer num canal do Discord. So nome e
/// data; %w(0,4,4) recua a mensagem como o git faz.
const PATCH_HEADER_FORMAT: &str = "--format=commit %H%nAuthor:     %an%nAuthorDate: %ad%nCommit:     %cn%nCommitDate: %cd%n%n%w(0,4,4)%B";

const GIT_TIMEOUT: Duration = Duration::from_secs(30);

const GIT_ENV: &[(&str, &str)] = &[
    // Sem isto, um `git status` qualquer poderia reescrever o index; os
    // comandos daqui nao escrevem, mas a garantia fica explicita.
    ("GIT_OPTIONAL_LOCKS", "0"),
    ("GIT_TERMINAL_PROMPT", "0"),
    // Erro do git em ingles e previsivel no log; saida em UTF-8.
    ("LC_ALL", "C.UTF-8"),
];

static STARTED: AtomicBool = AtomicBool::new(false);
static STOP: LazyLock<watch::Sender<bool>> = LazyLock::new(|| watch::channel(false).0);
static LAST_REPORT: Mutex<Option<String>> = Mutex::new(None);

/// Liga a varredura na primeira vez que os servidores terminam de carregar -
/// antes disso o canal nao esta no cache. Reconexao nao liga uma segunda.
pub fn start(ctx: &serenity::Context) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let ctx = ctx.clone();
    let stop = STOP.subscribe();
    tokio::spawn(run(ctx, stop));
}

/// Para a varredura. Uma volta em andamento e largada no meio: o `git` em
/// voo morre junto (process::run mata o grupo ao ser largado).
pub fn stop() {
    STOP.send_replace(true);
}

async fn run(ctx: serenity::Context, mut stop: watch::Receiver<bool>) {
    loop {
        if *stop.borrow() {
            return;
        }
        // Relido a cada volta: ligar e so editar o config, sem reiniciar.
        let settings = config::current().git_feed.clone();

        tokio::select! {
            result = round(&ctx, &settings) => {
                if let Err(cause) = result {
                    report(Level::Error, format!("[gitfeed] falha: {cause}"));
                }
            }
            _ = stop.wait_for(|stopped| *stopped) => return,
        }

        // Desligado, olha o config de novo a cada minuto.
        let wait = if settings.is_configured() {
            settings.poll_interval()
        } else {
            Duration::from_secs(60)
        };
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            _ = stop.wait_for(|stopped| *stopped) => return,
        }
    }
}

/// O canal resolvido, com o que os logs precisam (copiado do cache).
struct Target {
    id: ChannelId,
    name: String,
    guild_name: String,
}

async fn round(ctx: &serenity::Context, settings: &GitFeedSettings) -> Result<(), Error> {
    if !settings.is_configured() {
        report(
            Level::Info,
            "[gitfeed] desligado: faltam repoPath, guildId ou channelId na secao \"gitFeed\" do config."
                .into(),
        );
        return Ok(());
    }
    let repo = settings.repo_path.clone().unwrap_or_default();

    if !Path::new(&repo).is_dir() {
        report(
            Level::Warn,
            format!("[gitfeed] aviso: o repositorio {repo} nao existe."),
        );
        return Ok(());
    }

    let channel = match resolve_channel(ctx, settings) {
        Ok(channel) => channel,
        Err(problem) => {
            report(Level::Warn, format!("[gitfeed] aviso: {problem}"));
            return Ok(());
        }
    };

    let Some(head) = head(settings).await? else {
        report(
            Level::Warn,
            format!(
                "[gitfeed] aviso: o branch \"{}\" nao existe em {repo}.",
                settings.branch
            ),
        );
        return Ok(());
    };

    let state_path = paths::data("git-feed.json");
    let full = full_path(&repo);
    let state = state_file::load(&state_path).filter(|s| {
        s.last_commit.is_some()
            && s.repo_path.as_deref() == Some(full.as_str())
            && s.branch.as_deref() == Some(settings.branch.as_str())
    });

    let Some(mut state) = state else {
        // Primeira volta com este repositorio: posta so o commit mais
        // recente, como prova de que o canal esta certo, em vez de despejar o
        // historico inteiro.
        post(ctx, &channel, settings, &head).await?;
        state_file::save(
            &state_path,
            &GitFeedState {
                repo_path: Some(full),
                branch: Some(settings.branch.clone()),
                last_commit: Some(head),
            },
        )?;
        report_online(&channel, settings);
        return Ok(());
    };

    report_online(&channel, settings);

    let last = state.last_commit.clone().unwrap_or_default();
    if last == head {
        return Ok(());
    }

    let ancestor = git(settings, ["merge-base", "--is-ancestor", &last, &head]).await?;
    let mut commits = if ancestor.exit_code == 0 {
        let list = git(
            settings,
            [
                "rev-list",
                "--reverse",
                "--first-parent",
                &format!("{last}..{head}"),
            ],
        )
        .await?;
        lines(&list.stdout)
    } else {
        // amend, rebase, reset: o ultimo postado saiu do historico. Nao ha "o
        // que veio depois" para mostrar; posta so a ponta nova.
        warn!(
            "[gitfeed] aviso: o historico de {} foi reescrito; postando so {}.",
            settings.branch,
            short(&head)
        );
        vec![head.clone()]
    };

    if commits.len() > MAX_COMMITS_PER_ROUND {
        warn!(
            "[gitfeed] aviso: {} commits novos; postando so os {MAX_COMMITS_PER_ROUND} mais recentes.",
            commits.len()
        );
        commits = commits.split_off(commits.len() - MAX_COMMITS_PER_ROUND);
    }

    for sha in commits {
        post(ctx, &channel, settings, &sha).await?;
        // Um por um: se o proximo falhar, a volta seguinte recomeca dele, e
        // nao do comeco da lista.
        state.last_commit = Some(sha);
        state_file::save(&state_path, &state)?;
    }
    Ok(())
}

// ------------------------------------------------------------------
// Canal
// ------------------------------------------------------------------

/// Pelo servidor, e nao buscando o canal solto: um channelId de outro
/// servidor no config nao pode fazer o feed vazar para la.
fn resolve_channel(ctx: &serenity::Context, settings: &GitFeedSettings) -> Result<Target, String> {
    let guild_id = settings.guild_id.unwrap_or_default();
    let channel_id = settings.channel_id.unwrap_or_default();
    if guild_id == 0 || channel_id == 0 {
        return Err("faltam guildId ou channelId.".into());
    }

    let Some(guild) = ctx.cache.guild(GuildId::new(guild_id)) else {
        return Err(format!("o bot nao esta no servidor {guild_id}."));
    };
    let Some(channel) = guild.channels.get(&ChannelId::new(channel_id)) else {
        return Err(format!(
            "o canal {channel_id} nao existe em \"{}\" (ou o bot nao o enxerga).",
            guild.name
        ));
    };

    let needed = Permissions::VIEW_CHANNEL
        | Permissions::SEND_MESSAGES
        | Permissions::EMBED_LINKS
        | Permissions::ATTACH_FILES;
    let has = guild
        .members
        .get(&ctx.cache.current_user().id)
        .map_or(Permissions::empty(), |me| {
            guild.user_permissions_in(channel, me)
        });
    let missing = needed - has;
    if !missing.is_empty() {
        return Err(format!(
            "faltam permissoes em #{} (\"{}\"): {missing}.",
            channel.name, guild.name
        ));
    }

    Ok(Target {
        id: channel.id,
        name: channel.name.clone(),
        guild_name: guild.name.clone(),
    })
}

// ------------------------------------------------------------------
// Git
// ------------------------------------------------------------------

async fn git<'a>(
    settings: &'a GitFeedSettings,
    args: impl IntoIterator<Item = &'a str>,
) -> Result<process::ProcessResult, Error> {
    let repo = settings.repo_path.as_deref().unwrap_or_default();
    let mut all = vec![
        "-C",
        repo,
        // Caminho com acento sai como texto, e nao como "\303\241".
        "-c",
        "core.quotePath=false",
        "-c",
        "color.ui=never",
    ];
    all.extend(args);
    process::run(&settings.git_path, all, GIT_TIMEOUT, GIT_ENV).await
}

async fn head(settings: &GitFeedSettings) -> Result<Option<String>, Error> {
    let reference = format!("refs/heads/{}^{{commit}}", settings.branch);
    let result = git(settings, ["rev-parse", "--verify", "--quiet", &reference]).await?;
    let sha = result.stdout.trim();
    Ok((result.exit_code == 0 && sha.len() >= 40).then(|| sha.to_owned()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileChange {
    path: String,
    added: Option<i64>,
    deleted: Option<i64>,
}

impl FileChange {
    fn weight(&self) -> i64 {
        self.added.unwrap_or(0) + self.deleted.unwrap_or(0)
    }
}

async fn post(
    ctx: &serenity::Context,
    channel: &Target,
    settings: &GitFeedSettings,
    sha: &str,
) -> Result<(), Error> {
    // %x00 separa os campos: nenhum deles pode conter um NUL, e qualquer
    // outro separador poderia aparecer dentro da mensagem do commit.
    let meta = git(
        settings,
        ["show", "-s", "--format=%h%x00%an%x00%aI%x00%s%x00%b", sha],
    )
    .await?;
    let fields: Vec<&str> = meta.stdout.split('\0').collect();
    if meta.exit_code != 0 || fields.len() < 5 {
        return Err(format!(
            "git show de {} saiu com {}: {}",
            short(sha),
            meta.exit_code,
            meta.stderr.trim()
        )
        .into());
    }
    let (short_sha, author, date, subject, body) =
        (fields[0], fields[1], fields[2], fields[3], fields[4].trim());

    // --diff-merges=first-parent: merge mostra o que entrou no branch, e o
    // commit raiz mostra tudo como adicionado.
    let numstat = git(
        settings,
        [
            "show",
            "--format=",
            "--numstat",
            "-M",
            "--diff-merges=first-parent",
            sha,
        ],
    )
    .await?;
    let files: Vec<FileChange> = lines(&numstat.stdout)
        .iter()
        .filter_map(|l| parse_numstat(l))
        .collect();

    let patch = git(
        settings,
        [
            "show",
            PATCH_HEADER_FORMAT,
            "--stat",
            "--patch",
            "-M",
            "--diff-merges=first-parent",
            sha,
        ],
    )
    .await?;
    let patch_bytes = patch.stdout.into_bytes();
    let attach = patch_bytes.len() <= MAX_PATCH_BYTES;

    let embed = build_embed(
        settings, short_sha, author, date, subject, body, &files, attach,
    );

    // A mensagem do commit e texto livre: um "@everyone" nela nao pode
    // notificar o servidor.
    let mut message = serenity::CreateMessage::new()
        .embed(embed)
        .allowed_mentions(serenity::CreateAllowedMentions::new());
    if attach {
        message = message.add_file(serenity::CreateAttachment::bytes(
            patch_bytes,
            format!("{short_sha}.patch"),
        ));
    }

    channel.id.send_message(ctx, message).await?;
    info!(
        "[gitfeed] postado {short_sha} em #{} ({} arquivo(s))",
        channel.name,
        files.len()
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn build_embed(
    settings: &GitFeedSettings,
    short_sha: &str,
    author: &str,
    date: &str,
    subject: &str,
    body: &str,
    files: &[FileChange],
    attach: bool,
) -> CreateEmbed {
    let added: i64 = files.iter().map(|f| f.added.unwrap_or(0)).sum();
    let deleted: i64 = files.iter().map(|f| f.deleted.unwrap_or(0)).sum();

    let mut embed = CreateEmbed::new()
        .author(serenity::CreateEmbedAuthor::new(embeds::trim(
            &format!("{} · {}", settings.title, settings.branch),
            256,
        )))
        // Titulo de embed nao renderiza link mascarado; escapar ali so
        // deixaria a barra invertida a mostra.
        .title(embeds::trim(subject, 256))
        .description(embeds::trim(&describe(body, files, attach), 4000))
        .footer(serenity::CreateEmbedFooter::new(embeds::trim(
            &format!(
                "{short_sha} · {author} · {} file(s) · +{added} -{deleted}",
                files.len()
            ),
            2048,
        )))
        .colour(COLOR);

    if let Ok(when) = chrono::DateTime::parse_from_rfc3339(date) {
        embed = embed.timestamp(serenity::Timestamp::from(when));
    }
    embed
}

/// Corpo do commit, os arquivos mais mexidos e os avisos.
fn describe(body: &str, files: &[FileChange], attach: bool) -> String {
    let mut text = String::new();
    if !body.is_empty() {
        text.push_str(&embeds::safe_trim(body, 1500));
        text.push_str("\n\n");
    }

    if files.is_empty() {
        text.push_str("*No file changes.*\n");
    } else {
        // Os mais mexidos primeiro: e o que diz do que o commit trata. Num
        // bloco de codigo, onde "_" e "*" de nome de arquivo nao viram
        // italico.
        let mut listed: Vec<&FileChange> = files.iter().collect();
        listed.sort_by_key(|f| std::cmp::Reverse(f.weight()));
        listed.truncate(MAX_FILES_LISTED);

        text.push_str("```\n");
        for f in &listed {
            let counts = match f.added {
                None => "binary".to_owned(),
                Some(added) => format!("+{added} -{}", f.deleted.unwrap_or(0)),
            };
            text.push_str(&format!(
                "{counts:<13} {}\n",
                embeds::trim(&f.path.replace('`', "'"), 90)
            ));
        }
        text.push_str("```\n");

        if files.len() > listed.len() {
            text.push_str(&format!(
                "…and {} more file(s).\n",
                files.len() - listed.len()
            ));
        }
    }

    if !attach {
        text.push_str("*The full patch is too large to attach.*\n");
    }
    text
}

/// "12\t3\tcaminho", ou "-\t-\tcaminho" para binario.
fn parse_numstat(line: &str) -> Option<FileChange> {
    let mut parts = line.splitn(3, '\t');
    let (added, deleted, path) = (parts.next()?, parts.next()?, parts.next()?);
    Some(FileChange {
        path: path.to_owned(),
        added: added.parse().ok(),
        deleted: deleted.parse().ok(),
    })
}

// ------------------------------------------------------------------
// Estado e log
// ------------------------------------------------------------------

/// O caminho absoluto e normalizado, como o Path.GetFullPath do C#: e ele
/// que vai para o git-feed.json, e um caminho escrito diferente no config
/// nao pode virar "primeira volta" (e um commit repetido no canal).
fn full_path(path: &str) -> String {
    let joined = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };

    let mut parts: Vec<String> = Vec::new();
    for component in joined.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::ParentDir => {
                parts.pop();
            }
            Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
        }
    }

    let mut full = format!("/{}", parts.join("/"));
    // O GetFullPath guarda a barra do fim.
    if path.ends_with('/') && !parts.is_empty() {
        full.push('/');
    }
    full
}

fn report_online(channel: &Target, settings: &GitFeedSettings) {
    report(
        Level::Info,
        format!(
            "[gitfeed] ligado: {} ({}) -> #{} em \"{}\", a cada {}s",
            settings.repo_path.as_deref().unwrap_or_default(),
            settings.branch,
            channel.name,
            channel.guild_name,
            settings.poll_interval().as_secs()
        ),
    );
}

#[derive(Clone, Copy)]
enum Level {
    Info,
    Warn,
    Error,
}

/// Uma linha por mudanca de estado. Sem isto, um canal sem permissao
/// renderia a mesma linha de aviso a cada 30 segundos, para sempre.
fn report(level: Level, line: String) {
    {
        let mut last = LAST_REPORT.lock().unwrap_or_else(|e| e.into_inner());
        if last.as_deref() == Some(line.as_str()) {
            return;
        }
        *last = Some(line.clone());
    }
    match level {
        Level::Info => info!("{line}"),
        Level::Warn => warn!("{line}"),
        Level::Error => error!("{line}"),
    }
}

/// Linhas nao vazias, aparadas (o Split com RemoveEmptyEntries e
/// TrimEntries do C#).
fn lines(text: &str) -> Vec<String> {
    text.split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(path: &str, added: Option<i64>, deleted: Option<i64>) -> FileChange {
        FileChange {
            path: path.into(),
            added,
            deleted,
        }
    }

    #[test]
    fn numstat_le_texto_e_binario() {
        assert_eq!(
            parse_numstat("12\t3\tsrc/a b.rs"),
            Some(change("src/a b.rs", Some(12), Some(3)))
        );
        assert_eq!(
            parse_numstat("-\t-\timg.png"),
            Some(change("img.png", None, None))
        );
        assert_eq!(
            parse_numstat("1\t2\tdir/{a => b}\tx"),
            Some(change("dir/{a => b}\tx", Some(1), Some(2)))
        );
        assert_eq!(parse_numstat("lixo"), None);
    }

    #[test]
    fn descricao_lista_os_mais_mexidos_primeiro() {
        let files = vec![
            change("pouco.rs", Some(1), Some(0)),
            change("muito_`x`.rs", Some(40), Some(2)),
            change("bin.png", None, None),
        ];
        assert_eq!(
            describe("Corpo [link](u)", &files, true),
            "Corpo \\[link](u)\n\n```\n+40 -2        muito_'x'.rs\n+1 -0         pouco.rs\nbinary        bin.png\n```\n"
        );
        assert_eq!(
            describe("", &[], false),
            "*No file changes.*\n*The full patch is too large to attach.*\n"
        );

        let many: Vec<FileChange> = (0..20)
            .map(|i| change(&format!("f{i}"), Some(i), Some(0)))
            .collect();
        let text = describe("", &many, true);
        assert!(text.ends_with("```\n…and 5 more file(s).\n"));
        assert!(text.starts_with("```\n+19 -0        f19\n"));
    }

    #[test]
    fn caminho_completo_como_o_getfullpath() {
        assert_eq!(full_path("/home/x/repo"), "/home/x/repo");
        assert_eq!(full_path("/home/x/../y/./repo"), "/home/y/repo");
        assert_eq!(full_path("/home//x/"), "/home/x/");
        assert_eq!(full_path("/"), "/");
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(full_path("rel"), format!("{}/rel", cwd.display()));
    }

    #[test]
    fn linhas_e_sha_curto() {
        assert_eq!(lines(" a \n\n b\r\n"), ["a", "b"]);
        assert_eq!(short("0123456789"), "0123456");
        assert_eq!(short("abc"), "abc");
    }

    #[tokio::test]
    async fn le_head_e_commit_de_um_repositorio_de_verdade() {
        // Repositorio descartavel no diretorio temporario, nunca o de producao.
        let dir = std::env::temp_dir().join(format!("sollarety-gitfeed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sh = |script: &str| {
            let ok = std::process::Command::new("sh")
                .args(["-c", script])
                .current_dir(&dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            assert!(ok, "falhou: {script}");
        };
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        sh(
            "git init -q -b main && git -c user.name=Ana -c user.email=a@b commit -q --allow-empty -m 'Primeiro' -m 'Corpo'",
        );

        let settings = GitFeedSettings {
            repo_path: Some(dir.to_string_lossy().into_owned()),
            ..GitFeedSettings::default()
        };
        let sha = head(&settings).await.unwrap().unwrap();
        assert_eq!(sha.len(), 40);

        let meta = git(
            &settings,
            ["show", "-s", "--format=%h%x00%an%x00%aI%x00%s%x00%b", &sha],
        )
        .await
        .unwrap();
        let fields: Vec<&str> = meta.stdout.split('\0').collect();
        assert_eq!(fields[1], "Ana");
        assert_eq!(fields[3], "Primeiro");
        assert_eq!(fields[4].trim(), "Corpo");
        assert!(chrono::DateTime::parse_from_rfc3339(fields[2]).is_ok());

        let missing = GitFeedSettings {
            branch: "nao-existe".into(),
            ..settings
        };
        assert!(head(&missing).await.unwrap().is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
