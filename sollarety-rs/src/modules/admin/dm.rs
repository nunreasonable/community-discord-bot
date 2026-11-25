//! /dm role e /dm user: DM com a mensagem da staff (o dm_start/dm_status/
//! dm_cancel do painel do ccore, com o mesmo limitador e as mesmas travas).
//!
//! Uma DM em massa leva MUITO mais que os 15 minutos de um token de
//! interacao (500 pessoas no ritmo do limitador passam de 25 minutos). Por
//! isso o progresso e uma mensagem comum no canal, editada pelo canal, e
//! nunca pela interacao. Os jobs vivem so na memoria: no desligamento eles
//! param e a mensagem diz isso.

use std::{
    collections::HashMap,
    pin::pin,
    sync::{Arc, LazyLock, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use poise::{futures_util::StreamExt, serenity_prelude as serenity};
use tokio::sync::watch;

use super::{
    gate, modal,
    report::{self, FailKind, Failure, Outcome, Tally},
};
use crate::{Context, Data, Error, embeds, hierarchy, modlog};

/// Teto de destinatarios por envio, o mesmo do ccore: acima disso o envio
/// passa da meia hora e o risco de o Discord marcar o bot como spam sobe.
pub const MAX_RECIPIENTS: usize = 500;
/// O campo de embed para em 1024; o ccore descobriu do jeito dificil (as 500
/// DMs falhando uma a uma).
const MAX_MESSAGE: usize = 1024;
/// Pelo menos 3 s entre duas DMs no bot inteiro.
const MIN_GAP: Duration = Duration::from_secs(3);
/// Folga entre uma tentativa e a proxima, alem do limitador.
const SPACING: Duration = Duration::from_millis(1200);
/// Falhas seguidas que param o envio: insistir so piora o limite.
const MAX_CONSECUTIVE_FAILURES: usize = 12;
/// Custo real de uma DM no ritmo acima (a folga corre junto com a espera do
/// limitador, entao fica perto dos 3 s mais a ida e volta do HTTP). So para
/// a estimativa.
const PER_DM: Duration = Duration::from_millis(3500);
/// De quanto em quanto tempo a mensagem de progresso e editada.
const PROGRESS_EVERY: Duration = Duration::from_secs(10);
/// Quanto o desligamento espera os jobs pararem e editarem a mensagem.
const SHUTDOWN_BUDGET: Duration = Duration::from_secs(5);
const CANCEL_PREFIX: &str = "admin:dm-cancel:";

// ---------------------------------------------------------------------------
// Limitador (DmLimiter do ccore)
// ---------------------------------------------------------------------------

/// O Mutex do tokio atende por ordem de chegada e fica preso durante a
/// espera: e isso que impede duas tarefas de calcular a mesma folga.
#[derive(Default)]
struct DmLimiter {
    last: tokio::sync::Mutex<Option<tokio::time::Instant>>,
}

impl DmLimiter {
    async fn wait(&self) {
        let mut last = self.last.lock().await;
        if let Some(wait) = last.and_then(|at| MIN_GAP.checked_sub(at.elapsed())) {
            tokio::time::sleep(wait).await;
        }
        *last = Some(tokio::time::Instant::now());
    }
}

static LIMITER: LazyLock<DmLimiter> = LazyLock::new(DmLimiter::default);

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    Cancelled { by: u64 },
    Shutdown,
}

struct Job {
    id: String,
    starter: u64,
    stop: watch::Sender<Option<Stop>>,
}

#[derive(Debug, PartialEq, Eq)]
enum CancelResult {
    NotRunning,
    AlreadyStopping,
    Cancelled,
}

/// Um envio por servidor. O limitador ja e do bot inteiro; mais de um envio
/// no mesmo servidor so dividiria o ritmo e dobraria o barulho.
#[derive(Default)]
struct Jobs {
    map: Mutex<HashMap<u64, Arc<Job>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Jobs {
    fn running(&self, guild: u64) -> bool {
        lock(&self.map).contains_key(&guild)
    }

    /// Reserva a vaga do servidor. None quando ja ha um envio la.
    fn register(&self, guild: u64, starter: u64) -> Option<Arc<Job>> {
        let mut map = lock(&self.map);
        if map.contains_key(&guild) {
            return None;
        }
        let (stop, _) = watch::channel(None);
        let job = Arc::new(Job {
            id: crate::store::short_id(12),
            starter,
            stop,
        });
        map.insert(guild, job.clone());
        Some(job)
    }

    /// Solta a vaga, se ainda for deste job.
    fn finish(&self, guild: u64, id: &str) {
        let mut map = lock(&self.map);
        if map.get(&guild).is_some_and(|j| j.id == id) {
            map.remove(&guild);
        }
    }

    fn get(&self, guild: u64, id: &str) -> Option<Arc<Job>> {
        lock(&self.map).get(&guild).filter(|j| j.id == id).cloned()
    }

    fn cancel(&self, guild: u64, id: &str, by: u64) -> CancelResult {
        let Some(job) = self.get(guild, id) else {
            return CancelResult::NotRunning;
        };
        if job.stop.borrow().is_some() {
            return CancelResult::AlreadyStopping;
        }
        job.stop.send_replace(Some(Stop::Cancelled { by }));
        CancelResult::Cancelled
    }

    fn stop_all(&self) -> usize {
        let map = lock(&self.map);
        for job in map.values() {
            job.stop.send_if_modified(|stop| {
                if stop.is_none() {
                    *stop = Some(Stop::Shutdown);
                    true
                } else {
                    false
                }
            });
        }
        map.len()
    }

    fn is_empty(&self) -> bool {
        lock(&self.map).is_empty()
    }
}

static JOBS: LazyLock<Jobs> = LazyLock::new(Jobs::default);

/// Quem pode cancelar: quem comecou, ou um Administrator do servidor.
fn may_cancel(starter: u64, presser: u64, presser_is_admin: bool) -> bool {
    presser == starter || presser_is_admin
}

// ---------------------------------------------------------------------------
// Comandos
// ---------------------------------------------------------------------------

/// Sends a DM from the server's staff
#[poise::command(
    slash_command,
    rename = "dm",
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD",
    subcommand_required,
    subcommands("role", "user")
)]
pub async fn dm_group(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// DMs every member with a role (not bots)
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "ADMINISTRATOR",
    required_bot_permissions = "VIEW_CHANNEL | SEND_MESSAGES | EMBED_LINKS"
)]
async fn role(
    ctx: Context<'_>,
    #[description = "Everyone with this role gets the DM"] role: serenity::Role,
) -> Result<(), Error> {
    if !gate::ensure(ctx).await? {
        return Ok(());
    }
    let guild_id = ctx
        .guild_id()
        .ok_or("comando de servidor fora de servidor")?;
    let role_id = role.id;

    // Tudo que da para recusar sem rede vem antes do modal, para ninguem
    // escrever a mensagem a toa.
    let refusal = if role_id.get() == guild_id.get() {
        Some(embeds::error(
            "Invalid role",
            "Pick a specific role; DMing the whole server through @everyone isn't allowed.",
        ))
    } else if ctx.guild().is_none_or(|g| !g.roles.contains_key(&role_id)) {
        Some(embeds::error(
            "Invalid role",
            "That role isn't in this server.",
        ))
    } else if JOBS.running(guild_id.get()) {
        Some(busy())
    } else {
        None
    };
    if let Some(embed) = refusal {
        ctx.send(poise::CreateReply::default().embed(embed).ephemeral(true))
            .await?;
        return Ok(());
    }

    let Some((submit, text)) = modal::ask(ctx, ask("DM a role")).await? else {
        return Ok(());
    };
    modal::guard(ctx, &submit, async {
        start_role_job(ctx, &submit, guild_id, role_id, &text).await
    })
    .await;
    Ok(())
}

/// DMs one member of this server
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD",
    required_bot_permissions = "VIEW_CHANNEL"
)]
async fn user(
    ctx: Context<'_>,
    #[description = "Who gets the DM (must be in this server)"] user: serenity::User,
) -> Result<(), Error> {
    if !gate::ensure(ctx).await? {
        return Ok(());
    }
    let guild_id = ctx
        .guild_id()
        .ok_or("comando de servidor fora de servidor")?;

    let refusal = if user.bot {
        Some(embeds::error(
            "Invalid user",
            "Bots can't receive DMs from me.",
        ))
    } else if hierarchy::try_get_member(ctx, guild_id, user.id)
        .await?
        .is_none()
    {
        // So membro DESTE servidor: o bot nao vira canal para mandar DM a
        // qualquer conta do Discord.
        Some(embeds::error(
            "Not a member",
            format!("<@{}> isn't in this server.", user.id),
        ))
    } else {
        None
    };
    if let Some(embed) = refusal {
        ctx.send(poise::CreateReply::default().embed(embed).ephemeral(true))
            .await?;
        return Ok(());
    }

    let Some((submit, text)) = modal::ask(ctx, ask("DM a member")).await? else {
        return Ok(());
    };
    modal::guard(ctx, &submit, async {
        send_single(ctx, &submit, guild_id, &user, &text).await
    })
    .await;
    Ok(())
}

fn ask(title: &str) -> modal::Ask<'_> {
    modal::Ask {
        title,
        label: "Message",
        placeholder: "Sent in an embed that says it's from this server's staff.",
        max_length: MAX_MESSAGE as u16,
    }
}

fn busy() -> serenity::CreateEmbed {
    embeds::error(
        "Already sending",
        "A mass DM is already running in this server. Wait for it to finish or cancel it first.",
    )
}

/// O texto vira campo de embed DEPOIS do escape: um texto de 1024 com
/// colchetes passaria do teto, e todas as DMs falhariam uma a uma.
fn check_text(text: &str) -> Option<serenity::CreateEmbed> {
    if text.trim().is_empty() {
        return Some(embeds::error("Empty message", "There's nothing to send."));
    }
    let length = embeds::utf16_len(&embeds::safe(text));
    (length > MAX_MESSAGE).then(|| {
        embeds::error(
            "Message too long",
            format!(
                "With formatting escaped the message has {length} characters; a DM can carry {MAX_MESSAGE}."
            ),
        )
    })
}

/// O nome e o icone do servidor, do cache.
fn server_info(ctx: &serenity::Context, guild_id: serenity::GuildId) -> (String, Option<String>) {
    match ctx.cache.guild(guild_id) {
        Some(g) => (g.name.clone(), g.icon_url()),
        None => ("this server".to_owned(), None),
    }
}

fn dm_message(server: &str, icon: Option<String>, text: &str) -> serenity::CreateMessage {
    serenity::CreateMessage::new()
        .embed(report::dm_embed(server, icon, text))
        .allowed_mentions(serenity::CreateAllowedMentions::new())
}

struct Recipient {
    id: serenity::UserId,
    name: String,
}

/// Os membros com o cargo, sem bots. O cache so tem parte dos membros: a
/// lista inteira vem da API. Err e a frase pronta.
async fn resolve(
    ctx: &serenity::Context,
    guild_id: serenity::GuildId,
    role_id: serenity::RoleId,
) -> Result<Vec<Recipient>, String> {
    let mut found = Vec::new();
    let mut all = pin!(guild_id.members_iter(ctx));
    while let Some(member) = all.next().await {
        let member = member.map_err(|e| {
            format!(
                "I couldn't list the server's members: {}",
                embeds::trim(&e.to_string(), 150)
            )
        })?;
        if !member.user.bot && member.roles.contains(&role_id) {
            found.push(Recipient {
                id: member.user.id,
                name: member.display_name().to_owned(),
            });
        }
    }
    check_count(found.len(), role_id.get())?;
    Ok(found)
}

/// Nenhum ou demais: as duas recusas antes de comecar.
fn check_count(count: usize, role_id: u64) -> Result<(), String> {
    if count == 0 {
        return Err(format!("No members (excluding bots) have <@&{role_id}>."));
    }
    if count > MAX_RECIPIENTS {
        return Err(format!(
            "<@&{role_id}> has {} members. To stay clear of Discord's spam limits, a mass DM can reach at most {MAX_RECIPIENTS} at a time.",
            embeds::number(count as i64)
        ));
    }
    Ok(())
}

async fn start_role_job(
    ctx: Context<'_>,
    submit: &serenity::ModalInteraction,
    guild_id: serenity::GuildId,
    role_id: serenity::RoleId,
    text: &str,
) -> Result<(), Error> {
    if let Some(embed) = check_text(text) {
        return modal::finish(ctx.http(), submit, embed).await;
    }
    // A trava de novo: o config pode ter mudado com o modal aberto.
    if !gate::guild_allowed(Some(guild_id)) {
        return modal::finish(ctx.http(), submit, gate::locked_embed()).await;
    }
    if JOBS.running(guild_id.get()) {
        return modal::finish(ctx.http(), submit, busy()).await;
    }

    let recipients = match resolve(ctx.serenity_context(), guild_id, role_id).await {
        Ok(found) => found,
        Err(why) => {
            return modal::finish(ctx.http(), submit, embeds::error("Can't send", why)).await;
        }
    };

    let Some(job) = JOBS.register(guild_id.get(), ctx.author().id.get()) else {
        return modal::finish(ctx.http(), submit, busy()).await;
    };

    let tally = Tally {
        target: format!("<@&{role_id}>"),
        total: recipients.len(),
        ..Default::default()
    };
    let started_unix = chrono::Utc::now().timestamp();
    let cancel = vec![serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new(format!("{CANCEL_PREFIX}{}", job.id))
            .label("Cancel")
            .style(serenity::ButtonStyle::Danger),
    ])];
    // Mensagem comum, sem ping nenhum: so ela sobrevive aos 15 minutos.
    let progress = serenity::CreateMessage::new()
        .embed(report::progress(&tally, started_unix, PER_DM, PROGRESS_EVERY).to_embed())
        .components(cancel)
        .allowed_mentions(serenity::CreateAllowedMentions::new());
    let channel_id = ctx.channel_id();
    let progress = match channel_id.send_message(ctx.http(), progress).await {
        Ok(message) => message,
        Err(cause) => {
            JOBS.finish(guild_id.get(), &job.id);
            return modal::finish(
                ctx.http(),
                submit,
                embeds::error(
                    "Can't post the progress message",
                    format!(
                        "I need to post and edit a progress message in this channel. Nothing was sent.\n\nDetails: {}",
                        embeds::trim(&cause.to_string(), 400)
                    ),
                ),
            )
            .await;
        }
    };

    // Daqui ate o spawn nada pode falhar com `?`: um erro deixaria a vaga
    // do servidor presa sem ninguem enviando. O registro vem antes do envio
    // (o modlog nunca falha) para o "started" nao chegar depois do
    // "finished" num envio curto.
    let count = recipients.len();
    let job_id = job.id.clone();
    let link = progress.link();
    let extra = format!(
        "Role: <@&{role_id}> · {count} recipient(s) · {link}\n\nMessage:\n{}",
        embeds::trim(text, 600)
    );
    modlog::record(
        ctx,
        &ctx.data().guild_settings,
        guild_id,
        modlog::Entry::new("Mass DM started", ctx.author())
            .no_target_label(&format!("<@&{role_id}>"))
            .extra(&extra),
    )
    .await;

    let (server, icon) = server_info(ctx.serenity_context(), guild_id);
    let run = Run {
        ctx: ctx.serenity_context().clone(),
        data: ctx.data().clone(),
        guild_id,
        job,
        starter: ctx.author().clone(),
        recipients,
        message: dm_message(&server, icon, text),
        channel_id,
        message_id: progress.id,
        started_unix,
        tally,
    };
    tokio::spawn(run.execute());
    tracing::info!(
        "[admin] DM em massa {job_id} comecou em {guild_id}: cargo {role_id}, {count} destinatario(s), por {}",
        ctx.author().id
    );

    let started = embeds::ok(
        "Mass DM started",
        format!(
            "Sending to {} member(s) with <@&{role_id}>. That takes {}.\n[Follow the progress here]({link})",
            embeds::number(count as i64),
            report::eta_text(count, PER_DM)
        ),
    );
    if let Err(cause) = modal::finish(ctx.http(), submit, started).await {
        tracing::warn!(
            "[admin] DM em massa {job_id}: nao consegui confirmar a quem pediu: {cause}"
        );
    }
    Ok(())
}

/// Uma pessoa so: cabe folgado no token da interacao, entao o relatorio vem
/// na propria resposta efemera, no mesmo formato do envio em massa.
async fn send_single(
    ctx: Context<'_>,
    submit: &serenity::ModalInteraction,
    guild_id: serenity::GuildId,
    user: &serenity::User,
    text: &str,
) -> Result<(), Error> {
    if let Some(embed) = check_text(text) {
        return modal::finish(ctx.http(), submit, embed).await;
    }
    if !gate::guild_allowed(Some(guild_id)) {
        return modal::finish(ctx.http(), submit, gate::locked_embed()).await;
    }
    let name = match hierarchy::try_get_member(ctx, guild_id, user.id).await? {
        Some(member) => member.display_name().to_owned(),
        None => {
            return modal::finish(
                ctx.http(),
                submit,
                embeds::error(
                    "Not a member",
                    format!("<@{}> isn't in this server.", user.id),
                ),
            )
            .await;
        }
    };

    let (server, icon) = server_info(ctx.serenity_context(), guild_id);
    let started = Instant::now();
    let mut tally = Tally {
        target: format!("<@{}>", user.id),
        total: 1,
        ..Default::default()
    };
    LIMITER.wait().await;
    match user
        .id
        .direct_message(ctx, dm_message(&server, icon, text))
        .await
    {
        Ok(_) => tally.delivered.push(name),
        Err(cause) => tally.failures.push(failure(name, &cause)),
    }
    let outcome = Outcome::Finished;
    let result = report::final_report(&tally, &outcome, started.elapsed());
    modal::finish(ctx.http(), submit, result.to_embed()).await?;

    let delivered = if tally.sent() == 1 {
        "Delivered."
    } else {
        "Not delivered (DMs closed or an error)."
    };
    let extra = format!("{delivered}\n\nMessage:\n{}", embeds::trim(text, 600));
    modlog::record(
        ctx,
        &ctx.data().guild_settings,
        guild_id,
        modlog::Entry::new("DM sent", ctx.author())
            .target(user)
            .extra(&extra),
    )
    .await;
    Ok(())
}

/// Aviso do /role strip (o punishment_dm do ccore). Falha e silenciosa para
/// quem chama: DM fechada nao desfaz a remocao. True quando chegou.
pub async fn notify_roles_removed(
    ctx: &serenity::Context,
    guild_id: serenity::GuildId,
    user: &serenity::User,
    reason: Option<&str>,
) -> bool {
    let (server, _) = server_info(ctx, guild_id);
    let message = serenity::CreateMessage::new()
        .content(report::roles_removed_text(&server, reason))
        .allowed_mentions(serenity::CreateAllowedMentions::new());
    LIMITER.wait().await;
    user.id.direct_message(ctx, message).await.is_ok()
}

// ---------------------------------------------------------------------------
// O envio em segundo plano
// ---------------------------------------------------------------------------

/// Status HTTP e codigo JSON do Discord, quando o erro veio da API.
fn discord_error(error: &serenity::Error) -> (Option<u16>, Option<isize>) {
    match error {
        serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(response)) => (
            Some(response.status_code.as_u16()),
            Some(response.error.code),
        ),
        _ => (None, None),
    }
}

fn failure(name: String, error: &serenity::Error) -> Failure {
    let (status, code) = discord_error(error);
    Failure {
        name,
        kind: report::classify(status, code),
        detail: embeds::trim(&error.to_string(), 80),
    }
}

struct Run {
    ctx: serenity::Context,
    data: Data,
    guild_id: serenity::GuildId,
    job: Arc<Job>,
    starter: serenity::User,
    recipients: Vec<Recipient>,
    message: serenity::CreateMessage,
    channel_id: serenity::ChannelId,
    message_id: serenity::MessageId,
    started_unix: i64,
    tally: Tally,
}

impl Run {
    async fn execute(self) {
        let started = Instant::now();
        let tally = Mutex::new(self.tally.clone());

        let outcome = {
            let mut sending = pin!(send_all(
                &self.ctx,
                &self.recipients,
                &self.message,
                &tally,
                self.job.stop.subscribe(),
            ));
            let mut ticker = tokio::time::interval(PROGRESS_EVERY);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker.tick().await;
            let mut warned = false;
            loop {
                tokio::select! {
                    outcome = &mut sending => break outcome,
                    _ = ticker.tick() => {
                        let snapshot = lock(&tally).clone();
                        let embed = report::progress(&snapshot, self.started_unix, PER_DM, PROGRESS_EVERY).to_embed();
                        if let Err(cause) = self
                            .channel_id
                            .edit_message(&self.ctx, self.message_id, serenity::EditMessage::new().embed(embed))
                            .await
                            && !warned
                        {
                            // Uma linha so: a mensagem apagada daria uma
                            // falha a cada 10 s ate o fim.
                            warned = true;
                            tracing::warn!("[admin] DM em massa {}: nao consegui atualizar o progresso: {cause}", self.job.id);
                        }
                    }
                }
            }
        };

        let tally = lock(&tally).clone();
        let elapsed = started.elapsed();
        let final_report = report::final_report(&tally, &outcome, elapsed);
        let edit = serenity::EditMessage::new()
            .embed(final_report.to_embed())
            .components(Vec::new());
        if let Err(cause) = self
            .channel_id
            .edit_message(&self.ctx, self.message_id, edit)
            .await
        {
            // Mensagem apagada no meio: o relatorio sai numa nova, senao
            // ninguem fica sabendo como terminou.
            tracing::warn!(
                "[admin] DM em massa {}: nao consegui editar o relatorio ({cause}); postando de novo",
                self.job.id
            );
            let fresh = serenity::CreateMessage::new()
                .embed(final_report.to_embed())
                .allowed_mentions(serenity::CreateAllowedMentions::new());
            if let Err(cause) = self.channel_id.send_message(&self.ctx, fresh).await {
                tracing::error!(
                    "[admin] DM em massa {}: o relatorio final se perdeu: {cause}",
                    self.job.id
                );
            }
        }

        let how = match &outcome {
            Outcome::Finished => "finished".to_owned(),
            Outcome::Cancelled { by } => format!("cancelled by <@{by}>"),
            Outcome::Aborted(reason) => format!("stopped early: {reason}"),
            Outcome::Shutdown => "interrupted by a bot shutdown".to_owned(),
        };
        tracing::info!(
            "[admin] DM em massa {} terminou ({how}): {} enviada(s), {} falha(s), {} sem tentativa",
            self.job.id,
            tally.sent(),
            tally.failed(),
            tally.remaining()
        );
        let extra = format!(
            "Target: {} · {how}\nSent {} · Failed {} · Never attempted {} · Total {}\nDuration: {}",
            tally.target,
            tally.sent(),
            tally.failed(),
            tally.remaining(),
            tally.total,
            report::duration_text(elapsed)
        );
        modlog::record(
            &self.ctx,
            &self.data.guild_settings,
            self.guild_id,
            modlog::Entry::new("Mass DM finished", &self.starter)
                .no_target_label(&tally.target)
                .extra(&extra),
        )
        .await;

        JOBS.finish(self.guild_id.get(), &self.job.id);
    }
}

/// Espera `span`, ou menos se pedirem para parar.
async fn pause(span: Duration, stop: &mut watch::Receiver<Option<Stop>>) {
    tokio::select! {
        _ = tokio::time::sleep(span) => {}
        _ = stop.changed() => {}
    }
}

fn stopped(stop: &watch::Receiver<Option<Stop>>) -> Option<Outcome> {
    stop.borrow().map(|s| match s {
        Stop::Cancelled { by } => Outcome::Cancelled { by },
        Stop::Shutdown => Outcome::Shutdown,
    })
}

/// Um por um (o send_tracked do ccore). Para no primeiro sinal de rate
/// limit, ou depois de falhas demais em sequencia: insistir so piora o
/// limite e faz o Discord marcar o bot como spam. DM fechada sozinha e
/// normal e nao para nada.
async fn send_all(
    ctx: &serenity::Context,
    recipients: &[Recipient],
    message: &serenity::CreateMessage,
    tally: &Mutex<Tally>,
    mut stop: watch::Receiver<Option<Stop>>,
) -> Outcome {
    let mut streak = 0;
    for (i, recipient) in recipients.iter().enumerate() {
        if i > 0 {
            pause(SPACING, &mut stop).await;
        }
        if let Some(outcome) = stopped(&stop) {
            return outcome;
        }
        LIMITER.wait().await;
        // O limitador pode ter segurado ate 3 s.
        if let Some(outcome) = stopped(&stop) {
            return outcome;
        }

        let Err(cause) = recipient.id.direct_message(ctx, message.clone()).await else {
            lock(tally).delivered.push(recipient.name.clone());
            streak = 0;
            continue;
        };

        let fail = failure(recipient.name.clone(), &cause);
        let kind = fail.kind;
        lock(tally).failures.push(fail);
        streak += 1;

        if kind == FailKind::RateLimited {
            return Outcome::Aborted(
                "Discord signalled a rate limit, so I stopped to keep the bot from being flagged as spam."
                    .to_owned(),
            );
        }
        if streak >= MAX_CONSECUTIVE_FAILURES {
            return Outcome::Aborted(format!("Stopped after {streak} failures in a row."));
        }
    }
    Outcome::Finished
}

// ---------------------------------------------------------------------------
// Botao de cancelar e desligamento
// ---------------------------------------------------------------------------

/// Ponto de entrada do `on_event`. Volta na hora; o trabalho vai numa
/// tarefa propria.
pub fn on_interaction(ctx: &serenity::Context, interaction: &serenity::Interaction, data: &Data) {
    let serenity::Interaction::Component(press) = interaction else {
        return;
    };
    if !press.data.custom_id.starts_with(CANCEL_PREFIX) {
        return;
    }
    let (ctx, data, press) = (ctx.clone(), data.clone(), press.clone());
    tokio::spawn(async move {
        if let Err(cause) = cancel_pressed(&ctx, &data, &press).await {
            tracing::error!(
                "[admin] falha ao tratar o cancelamento de {}: {cause}",
                press.user.id
            );
        }
    });
}

async fn cancel_pressed(
    ctx: &serenity::Context,
    data: &Data,
    press: &serenity::ComponentInteraction,
) -> Result<(), Error> {
    let reply = |embed: serenity::CreateEmbed| {
        serenity::CreateInteractionResponse::Message(
            serenity::CreateInteractionResponseMessage::new()
                .embed(embed)
                .ephemeral(true),
        )
    };
    let Some(guild_id) = press
        .guild_id
        .filter(|_| gate::guild_allowed(press.guild_id))
    else {
        press
            .create_response(ctx, reply(gate::locked_embed()))
            .await?;
        return Ok(());
    };
    let job_id = &press.data.custom_id[CANCEL_PREFIX.len()..];

    let Some(job) = JOBS.get(guild_id.get(), job_id) else {
        press
            .create_response(
                ctx,
                reply(embeds::info(
                    "Not running",
                    "This mass DM isn't running anymore.",
                )),
            )
            .await?;
        return Ok(());
    };
    let is_admin = press
        .member
        .as_ref()
        .and_then(|m| m.permissions)
        .is_some_and(|p| p.administrator());
    if !may_cancel(job.starter, press.user.id.get(), is_admin) {
        press
            .create_response(
                ctx,
                reply(embeds::error(
                    "Not allowed",
                    "Only the person who started this or an Administrator can cancel it.",
                )),
            )
            .await?;
        return Ok(());
    }

    let embed = match JOBS.cancel(guild_id.get(), job_id, press.user.id.get()) {
        CancelResult::Cancelled => embeds::ok(
            "Cancelling",
            "No more DMs will go out. The progress message turns into the final report in a moment.",
        ),
        CancelResult::AlreadyStopping => {
            embeds::info("Already stopping", "This mass DM is already stopping.")
        }
        CancelResult::NotRunning => {
            embeds::info("Not running", "This mass DM isn't running anymore.")
        }
    };
    press.create_response(ctx, reply(embed)).await?;

    tracing::info!(
        "[admin] DM em massa {job_id} cancelada por {}",
        press.user.id
    );
    let extra = format!("Job {job_id} — cancel requested.");
    modlog::record(
        ctx,
        &data.guild_settings,
        guild_id,
        modlog::Entry::new("Mass DM cancelled", &press.user)
            .no_target_label("—")
            .extra(&extra),
    )
    .await;
    Ok(())
}

/// Para todo envio em andamento e espera as mensagens de progresso virarem
/// relatorio ("interrupted by a bot shutdown"), com prazo.
pub async fn shutdown() {
    let running = JOBS.stop_all();
    if running == 0 {
        return;
    }
    tracing::info!("[admin] parando {running} DM(s) em massa para desligar...");
    let deadline = tokio::time::Instant::now() + SHUTDOWN_BUDGET;
    while !JOBS.is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if !JOBS.is_empty() {
        tracing::warn!(
            "[admin] aviso: DM em massa nao terminou em {SHUTDOWN_BUDGET:?}; a mensagem de progresso pode ter ficado sem o relatorio final"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn um_envio_por_servidor() {
        let jobs = Jobs::default();
        let a = jobs.register(1, 10).unwrap();
        assert!(jobs.register(1, 11).is_none());
        assert!(jobs.register(2, 11).is_some());
        assert!(jobs.running(1));

        // Soltar com o id errado nao solta a vaga de outro job.
        jobs.finish(1, "outro");
        assert!(jobs.running(1));
        jobs.finish(1, &a.id);
        assert!(!jobs.running(1));
        assert!(jobs.register(1, 12).is_some());
    }

    #[test]
    fn cancelar() {
        let jobs = Jobs::default();
        let job = jobs.register(1, 10).unwrap();
        assert_eq!(jobs.cancel(1, "nao-existe", 5), CancelResult::NotRunning);
        assert_eq!(jobs.cancel(2, &job.id, 5), CancelResult::NotRunning);
        assert_eq!(jobs.cancel(1, &job.id, 5), CancelResult::Cancelled);
        assert_eq!(*job.stop.borrow(), Some(Stop::Cancelled { by: 5 }));
        assert_eq!(jobs.cancel(1, &job.id, 6), CancelResult::AlreadyStopping);
    }

    #[test]
    fn desligar_para_todos_sem_apagar_o_cancelamento() {
        let jobs = Jobs::default();
        let a = jobs.register(1, 10).unwrap();
        let b = jobs.register(2, 10).unwrap();
        jobs.cancel(1, &a.id, 5);
        assert_eq!(jobs.stop_all(), 2);
        assert_eq!(*a.stop.borrow(), Some(Stop::Cancelled { by: 5 }));
        assert_eq!(*b.stop.borrow(), Some(Stop::Shutdown));
    }

    #[test]
    fn quem_pode_cancelar() {
        assert!(may_cancel(10, 10, false));
        assert!(may_cancel(10, 11, true));
        assert!(!may_cancel(10, 11, false));
    }

    #[test]
    fn tetos() {
        assert!(check_count(0, 9).unwrap_err().contains("No members"));
        assert!(check_count(1, 9).is_ok());
        assert!(check_count(MAX_RECIPIENTS, 9).is_ok());
        let over = check_count(MAX_RECIPIENTS + 1, 9).unwrap_err();
        assert!(over.contains("501") && over.contains("500"));

        assert!(check_text("   ").is_some());
        assert!(check_text(&"a".repeat(MAX_MESSAGE)).is_none());
        // 600 colchetes viram 1200 com o escape: passam do campo.
        assert!(check_text(&"[".repeat(600)).is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn limitador_espaca_tres_segundos() {
        let limiter = DmLimiter::default();
        let start = tokio::time::Instant::now();
        limiter.wait().await;
        limiter.wait().await;
        assert!(start.elapsed() >= Duration::from_secs(3));
    }

    #[tokio::test(start_paused = true)]
    async fn pausa_acaba_quando_pedem_para_parar() {
        let (tx, mut rx) = watch::channel(None);
        let start = tokio::time::Instant::now();
        let waiting = tokio::spawn(async move {
            pause(Duration::from_secs(60), &mut rx).await;
            stopped(&rx)
        });
        tokio::task::yield_now().await;
        tx.send_replace(Some(Stop::Shutdown));
        assert_eq!(waiting.await.unwrap(), Some(Outcome::Shutdown));
        assert!(start.elapsed() < Duration::from_secs(60));
    }
}
