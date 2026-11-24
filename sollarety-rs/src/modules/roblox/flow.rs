//! O caminho do /verify e dos botoes dele (VerificationFlow.cs).
//!
//! Como funciona, de ponta a ponta:
//!
//! 1. O /verify (ou o botao do painel) responde, so para quem pediu, com um
//!    botao-link para daeese.me/oauth/roblox/start. O link e o MESMO para todo
//!    mundo e nao carrega identidade nenhuma.
//! 2. No navegador, o Worker faz a pessoa entrar com o Discord e depois com o
//!    Roblox. As duas identidades ficam provadas no mesmo navegador, e o Worker
//!    guarda "Discord X e dono do Roblox Y" por meia hora.
//! 3. Aqui, o bot pergunta ao Worker a cada poucos segundos se ja existe
//!    resultado para aquele id do Discord. Quando existe, grava o vinculo,
//!    aplica no servidor e edita a resposta.
//!
//! O passo pelo Discord no navegador e o que fecha o golpe do link repassado:
//! se o link carregasse um codigo do /verify, bastaria mandar esse link para a
//! vitima - ela autorizaria o Roblox DELA e a conta cairia no Discord de quem
//! mandou. Sendo o Discord do navegador que decide, a vitima vincularia a
//! propria conta a si mesma.
//!
//! Custom ids com prefixo "verify:".

use std::{
    collections::HashMap,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use poise::serenity_prelude as serenity;
use serenity::builder::Builder;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tracing::{info, warn};

use super::{
    api,
    service::{self, ApplyReport},
    worker::{self, Take},
};
use crate::{
    Data, Error, audit_reason, embeds,
    store::{RobloxLink, time},
};

pub const PREFIX: &str = "verify:";
pub const START_ID: &str = "verify:start";
const CHECK_ID: &str = "verify:check";
pub const UNLINK_ID: &str = "verify:unlink";

const POLL_INTERVAL: Duration = Duration::from_secs(4);

/// Abaixo dos 15 minutos de vida do token da interacao, que e o que permite
/// editar a resposta quando o resultado chega.
const POLL_WINDOW: Duration = Duration::from_secs(10 * 60);

/// Quanto o botao "I've authorized" espera uma conclusao em andamento.
const GATE_WAIT: Duration = Duration::from_secs(20);

/// Um polling por pessoa: um /verify novo substitui o anterior. O id separa
/// o polling atual de um antigo que ainda esta terminando.
struct Poll {
    id: u64,
    stop: watch::Sender<bool>,
}

static POLLS: LazyLock<Mutex<HashMap<u64, Poll>>> = LazyLock::new(Mutex::default);
static NEXT_POLL: AtomicU64 = AtomicU64::new(1);

/// Uma conclusao por pessoa de cada vez. Sem isto o polling e o botao "I've
/// authorized" podiam consumir o resultado em paralelo - um levava, o outro
/// dizia "Nothing received yet" para quem acabou de verificar.
static GATES: LazyLock<Mutex<HashMap<u64, Arc<Semaphore>>>> = LazyLock::new(Mutex::default);

fn polls() -> std::sync::MutexGuard<'static, HashMap<u64, Poll>> {
    POLLS.lock().unwrap_or_else(|e| e.into_inner())
}

/// A resposta de uma interacao, por id e token: serve igual ao comando e ao
/// botao, e o token sozinho deixa editar a resposta de dentro do polling,
/// minutos depois. `responded` diz se a resposta inicial ja foi dada.
#[derive(Clone)]
pub struct Reply {
    id: serenity::InteractionId,
    token: String,
    responded: Arc<AtomicBool>,
}

impl Reply {
    pub fn new(id: serenity::InteractionId, token: &str) -> Self {
        Self {
            id,
            token: token.to_owned(),
            responded: Arc::default(),
        }
    }

    pub fn responded(&self) -> bool {
        self.responded.load(Ordering::SeqCst)
    }

    async fn respond(
        &self,
        http: &serenity::Http,
        response: serenity::CreateInteractionResponse,
    ) -> Result<(), serenity::Error> {
        response.execute(http, (self.id, &self.token)).await?;
        self.responded.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn edit(
        &self,
        http: &serenity::Http,
        edit: serenity::EditInteractionResponse,
    ) -> Result<(), serenity::Error> {
        edit.execute(http, &self.token).await.map(drop)
    }

    /// O `ModifyMode.Replace` do C#: a mensagem inteira e trocada, e os
    /// botoes que ela tinha somem junto.
    async fn replace(
        &self,
        http: &serenity::Http,
        embed: serenity::CreateEmbed,
    ) -> Result<(), serenity::Error> {
        self.edit(
            http,
            serenity::EditInteractionResponse::new()
                .embed(embed)
                .components(Vec::new()),
        )
        .await
    }

    async fn follow_up(&self, http: &serenity::Http, embed: serenity::CreateEmbed) {
        let builder = serenity::CreateInteractionResponseFollowup::new()
            .embed(embed)
            .ephemeral(true);
        if let Err(cause) = builder.execute(http, (None, &self.token)).await {
            warn!("[verificacao] nao consegui responder ao botao: {cause}");
        }
    }

    async fn try_fail(&self, http: &serenity::Http, message: &str) {
        let response = serenity::CreateInteractionResponse::Message(
            serenity::CreateInteractionResponseMessage::new()
                .embed(embeds::error("Error", message))
                .ephemeral(true),
        );
        if self.respond(http, response).await.is_err() {
            // Ja tinha respondido (defer): o follow-up e o que resta.
            self.follow_up(http, embeds::error("Error", message)).await;
        }
    }
}

fn ephemeral_defer() -> serenity::CreateInteractionResponse {
    serenity::CreateInteractionResponse::Defer(
        serenity::CreateInteractionResponseMessage::new().ephemeral(true),
    )
}

/// O /verify e o botao do painel caem aqui.
pub async fn begin(
    sctx: &serenity::Context,
    data: &Data,
    reply: &Reply,
    guild_id: serenity::GuildId,
    member: &serenity::Member,
) -> Result<(), Error> {
    if let Some(existing) = data.links.for_user(member.user.id.get()) {
        reply.respond(&sctx.http, ephemeral_defer()).await?;

        let report = service::apply(
            sctx,
            data,
            guild_id,
            member,
            Some(&existing),
            &audit_reason::automatic("Roblox verification: /verify by an already linked member"),
            false,
        )
        .await;

        reply
            .edit(
                &sctx.http,
                serenity::EditInteractionResponse::new().embed(linked_embed(
                    "You're already verified",
                    &existing,
                    Some(&report),
                    Some("To switch Roblox accounts, run `/unverify` and then `/verify` again."),
                )),
            )
            .await?;
        return Ok(());
    }

    let settings = data.config().roblox_verify.clone();
    if !settings.is_configured() {
        reply
            .respond(
                &sctx.http,
                serenity::CreateInteractionResponse::Message(
                    serenity::CreateInteractionResponseMessage::new()
                        .ephemeral(true)
                        .embed(embeds::error(
                            "Verification unavailable",
                            "Whoever runs the bot hasn't turned on Roblox verification yet. Nothing is wrong on your end.",
                        )),
                ),
            )
            .await?;
        return Ok(());
    }

    let buttons = serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new_link(settings.start_url).label("Open verification"),
        serenity::CreateButton::new(CHECK_ID)
            .style(serenity::ButtonStyle::Secondary)
            .label("I've authorized"),
    ]);
    reply
        .respond(
            &sctx.http,
            serenity::CreateInteractionResponse::Message(
                serenity::CreateInteractionResponseMessage::new()
                    .ephemeral(true)
                    .embed(prompt_embed())
                    .components(vec![buttons]),
            ),
        )
        .await?;

    start_polling(sctx, data, reply, guild_id, member.user.id.get());
    Ok(())
}

/// Clique num botao "verify:". Quem chama ja conferiu o prefixo.
pub fn on_component(
    sctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::ComponentInteraction,
) {
    let sctx = sctx.clone();
    let data = data.clone();
    let interaction = interaction.clone();

    tokio::spawn(async move {
        let id = interaction.data.custom_id.as_str();
        let reply = Reply::new(interaction.id, &interaction.token);
        if let Err(cause) = handle_component(&sctx, &data, &reply, &interaction).await {
            service::log_failure(
                &format!("falha ao tratar '{id}' de {}", interaction.user.id),
                &cause,
            );
            reply
                .try_fail(
                    &sctx.http,
                    "Something broke while processing this action. Let whoever runs the bot know.",
                )
                .await;
        }
    });
}

async fn handle_component(
    sctx: &serenity::Context,
    data: &Data,
    reply: &Reply,
    interaction: &serenity::ComponentInteraction,
) -> Result<(), Error> {
    let id = interaction.data.custom_id.as_str();

    // O Unlink desfaz um vinculo GLOBAL: vale em DM e como app pessoal. So
    // mexe em cargo e apelido quando o bot esta no servidor - num servidor
    // sem ele, nao ha o que mexer.
    if id == UNLINK_ID {
        let bot_guild = interaction
            .guild_id
            .filter(|&g| sctx.cache.guild(g).is_some());
        let target = bot_guild.zip(interaction.member.as_ref());
        return unlink(sctx, data, reply, interaction.user.id.get(), target).await;
    }

    let (Some(guild_id), Some(member)) = (interaction.guild_id, interaction.member.as_ref()) else {
        reply
            .try_fail(&sctx.http, "This only works inside a server.")
            .await;
        return Ok(());
    };

    match id {
        START_ID => begin(sctx, data, reply, guild_id, member).await,
        CHECK_ID => check(sctx, data, reply, guild_id, member).await,
        _ => {
            info!("[verificacao] custom id desconhecido: '{id}'");
            reply
                .try_fail(
                    &sctx.http,
                    "This button is from an old version. Run `/verify` again.",
                )
                .await;
            Ok(())
        }
    }
}

pub fn linked_embed(
    title: &str,
    link: &RobloxLink,
    report: Option<&ApplyReport>,
    footer: Option<&str>,
) -> serenity::CreateEmbed {
    let mut embed = serenity::CreateEmbed::new()
        .title(title)
        .colour(embeds::colors::SPRING_GREEN)
        .description(format!("Roblox account: {}", describe(link)));

    match report {
        Some(report) if report.configured => {
            if let Some(refusal) = &report.refusal {
                embed = embed.field(
                    "Not approved in this server",
                    embeds::trim(&format!("The account is linked, but {refusal}"), 1024),
                    false,
                );
            }
            embed = embed.field("In this server", report.describe(), false);
        }
        _ => {
            embed = embed.field(
                "In this server",
                "This server hasn't set up verification roles yet. The link already applies in every server that uses Sollarety.",
                false,
            );
        }
    }

    if let Some(footer) = footer {
        embed = embed.footer(serenity::CreateEmbedFooter::new(footer));
    }
    embed
}

/// "**Exibicao** (@usuario)", com link para o perfil.
pub fn describe(link: &RobloxLink) -> String {
    let name = embeds::safe(&link.roblox_name);
    let display = embeds::safe(if link.display_name.trim().is_empty() {
        &link.roblox_name
    } else {
        &link.display_name
    });

    // O link mascarado aqui e seguro: a URL e montada pelo bot com o id
    // numerico, nao vem de ninguem.
    format!(
        "**{display}** ([@{name}]({}))",
        api::profile_url(link.roblox_id)
    )
}

// ------------------------------------------------------------------
// Conclusao
// ------------------------------------------------------------------

enum Completion {
    Pending,
    Linked(RobloxLink, Option<ApplyReport>),
    Failed(String),
}

fn gate(user_id: u64) -> Arc<Semaphore> {
    GATES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(user_id)
        .or_insert_with(|| Arc::new(Semaphore::new(1)))
        .clone()
}

/// Busca o resultado no Worker e, se houver, grava e aplica. `wait` falso e
/// o polling: se ja ha uma conclusao em andamento, ele so pula a rodada.
async fn try_complete(
    sctx: &serenity::Context,
    data: &Data,
    guild_id: serenity::GuildId,
    user_id: u64,
    wait: bool,
) -> Result<Completion, Error> {
    let gate = gate(user_id);
    let permit: Option<OwnedSemaphorePermit> = if wait {
        tokio::time::timeout(GATE_WAIT, gate.acquire_owned())
            .await
            .ok()
            .and_then(Result::ok)
    } else {
        gate.try_acquire_owned().ok()
    };
    let Some(_permit) = permit else {
        return Ok(Completion::Pending);
    };

    let account = match worker::take(&data.http, &data.config().roblox_verify, user_id).await {
        Take::Failed(error) => return Ok(Completion::Failed(error)),
        Take::Pending => return Ok(Completion::Pending),
        Take::Found(account) => account,
    };

    // Do Worker vale so o id, que e o que o OAuth prova. Nome, exibicao e
    // data de criacao vem da API publica, que e a mesma fonte do /update:
    // assim os dois nunca discordam. Se ela falhar, fica o que o OAuth disse.
    let user = api::get_user(&data.http, account.roblox_id, true).await;
    let user = user.value();
    let link = data
        .links
        .set(RobloxLink {
            discord_id: user_id,
            roblox_id: account.roblox_id,
            roblox_name: user.map_or(account.name, |u| u.name.clone()),
            display_name: user.map_or(account.display_name, |u| u.display_name.clone()),
            roblox_created_utc: user.and_then(|u| u.created).or(account.created),
            verified_at_utc: time::now(),
            refreshed_at_utc: None,
        })
        .await?;

    info!(
        "[verificacao] {user_id} vinculado a {} ({})",
        link.roblox_name, link.roblox_id
    );

    let mut report = None;
    if sctx.cache.guild(guild_id).is_some()
        && let Some(member) =
            crate::hierarchy::try_get_member(sctx, guild_id, serenity::UserId::new(user_id)).await?
    {
        let reason = audit_reason::automatic(&format!(
            "Roblox verification: linked to {} ({})",
            link.roblox_name, link.roblox_id
        ));
        report =
            Some(service::apply(sctx, data, guild_id, &member, Some(&link), &reason, false).await);
    }

    Ok(Completion::Linked(link, report))
}

fn start_polling(
    sctx: &serenity::Context,
    data: &Data,
    reply: &Reply,
    guild_id: serenity::GuildId,
    user_id: u64,
) {
    let (stop, mut stopped) = watch::channel(false);
    let poll_id = NEXT_POLL.fetch_add(1, Ordering::Relaxed);

    // Um /verify novo substitui o anterior: a resposta que vale editar e a
    // mais recente, e dois pollings pela mesma pessoa so gastariam o Worker.
    if let Some(old) = polls().insert(user_id, Poll { id: poll_id, stop }) {
        old.stop.send_replace(true);
    }

    let sctx = sctx.clone();
    let data = data.clone();
    let reply = reply.clone();
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + POLL_WINDOW;
        let mut failure_logged = false;

        // Tempo esgotado ou substituido por um /verify novo: o botao "I've
        // authorized" continua funcionando sem polling nenhum.
        while !*stopped.borrow() && tokio::time::Instant::now() < deadline {
            match try_complete(&sctx, &data, guild_id, user_id, false).await {
                Ok(Completion::Linked(link, report)) => {
                    edit_quietly(
                        &sctx,
                        &reply,
                        linked_embed("Account verified", &link, report.as_ref(), None),
                    )
                    .await;
                    break;
                }
                // Falha do Worker nao encerra o polling: pode ser uma
                // instabilidade passageira. Vai para o log uma vez so.
                Ok(Completion::Failed(error)) if !failure_logged => {
                    warn!("[verificacao] aviso: polling de {user_id}: {error}");
                    failure_logged = true;
                }
                Ok(_) => {}
                Err(cause) => {
                    service::log_failure(&format!("falha no polling de {user_id}"), &cause);
                    break;
                }
            }

            let wake = (tokio::time::Instant::now() + POLL_INTERVAL).min(deadline);
            tokio::select! {
                () = tokio::time::sleep_until(wake) => {}
                _ = stopped.changed() => break,
            }
        }

        let mut polls = polls();
        if polls.get(&user_id).is_some_and(|p| p.id == poll_id) {
            polls.remove(&user_id);
        }
    });
}

fn stop_polling(user_id: u64) {
    if let Some(poll) = polls().remove(&user_id) {
        poll.stop.send_replace(true);
    }
}

/// Desligamento: nenhum polling sobrevive ao gateway.
pub fn stop_all() {
    for (_, poll) in polls().drain() {
        poll.stop.send_replace(true);
    }
}

/// "I've authorized". Funciona mesmo sem polling ativo - depois de o bot
/// reiniciar, por exemplo - porque tudo o que precisa vem do clique.
async fn check(
    sctx: &serenity::Context,
    data: &Data,
    reply: &Reply,
    guild_id: serenity::GuildId,
    member: &serenity::Member,
) -> Result<(), Error> {
    let user_id = member.user.id.get();

    // Atualiza a propria mensagem efemera onde o botao esta.
    reply
        .respond(&sctx.http, serenity::CreateInteractionResponse::Acknowledge)
        .await?;

    match try_complete(sctx, data, guild_id, user_id, true).await? {
        Completion::Linked(link, report) => {
            stop_polling(user_id);
            reply
                .replace(
                    &sctx.http,
                    linked_embed("Account verified", &link, report.as_ref(), None),
                )
                .await?;
        }
        Completion::Failed(error) => {
            reply
                .follow_up(
                    &sctx.http,
                    embeds::error(
                        "Couldn't check",
                        format!(
                            "Something went wrong while fetching the result: {error}. Try again in a few seconds."
                        ),
                    ),
                )
                .await;
        }
        Completion::Pending => {
            // Nada pendente, mas o polling pode ter concluido um instante antes.
            if let Some(link) = data.links.for_user(user_id) {
                stop_polling(user_id);
                let report = service::apply(
                    sctx,
                    data,
                    guild_id,
                    member,
                    Some(&link),
                    &audit_reason::automatic("Roblox verification: checked via button"),
                    false,
                )
                .await;
                reply
                    .replace(
                        &sctx.http,
                        linked_embed("Account verified", &link, Some(&report), None),
                    )
                    .await?;
            } else {
                reply
                    .follow_up(
                        &sctx.http,
                        embeds::info(
                            "Nothing received yet",
                            "Finish both logins on the page that opened — Discord first, then Roblox — and click here again.\n\n\
                             If the page said it worked and still nothing arrives, check that it showed **this** Discord \
                             account: your browser may be logged into a different one.",
                        ),
                    )
                    .await;
            }
        }
    }
    Ok(())
}

async fn unlink(
    sctx: &serenity::Context,
    data: &Data,
    reply: &Reply,
    user_id: u64,
    target: Option<(serenity::GuildId, &serenity::Member)>,
) -> Result<(), Error> {
    reply
        .respond(&sctx.http, serenity::CreateInteractionResponse::Acknowledge)
        .await?;

    stop_polling(user_id);
    let Some(removed) = data.links.remove(user_id).await? else {
        reply
            .replace(
                &sctx.http,
                embeds::info(
                    "Nothing to undo",
                    "Your Discord account was no longer linked to any Roblox account.",
                ),
            )
            .await?;
        return Ok(());
    };

    info!(
        "[verificacao] {user_id} desvinculou {} ({})",
        removed.roblox_name, removed.roblox_id
    );

    let mut report = None;
    if let Some((guild_id, member)) = target {
        let reason = audit_reason::automatic("Roblox verification: /unverify");
        let mut applied = service::apply(sctx, data, guild_id, member, None, &reason, false).await;
        service::clear_nickname_if_ours(
            sctx,
            data,
            guild_id,
            member,
            &removed,
            &reason,
            &mut applied,
        )
        .await;
        report = Some(applied);
    }

    let elsewhere = if target.is_none() {
        "In servers that use Sollarety, verification roles are removed on the next `/update`."
    } else {
        "In other servers, verification roles are removed on the next `/update`."
    };
    let mut embed = serenity::CreateEmbed::new()
        .title("Account unlinked")
        .colour(embeds::colors::SPRING_GREEN)
        .description(format!(
            "{} is no longer linked to your Discord account, in any server.\n\n{elsewhere}",
            describe(&removed)
        ));

    if let Some(report) = report.filter(|r| r.configured) {
        embed = embed.field("In this server", report.describe(), false);
    }

    reply.replace(&sctx.http, embed).await?;
    Ok(())
}

fn prompt_embed() -> serenity::CreateEmbed {
    serenity::CreateEmbed::new()
        .title("Verify your Roblox account")
        .colour(embeds::colors::BLURPLE)
        .description(
            "1. Click **Open verification**. Sign in with **this same Discord account**, then with your Roblox account.\n\
             2. Come back here. I'll check automatically every few seconds for 10 minutes; if nothing changes, click **I've authorized**.\n\n\
             The login is Roblox's official one: your password never goes through the bot. Only the Roblox account's id, \
             name and creation date are stored, until you run `/unverify`.",
        )
        .footer(serenity::CreateEmbedFooter::new(
            "Roblox only lets accounts aged 13 or older authorize apps.",
        ))
}

async fn edit_quietly(sctx: &serenity::Context, reply: &Reply, embed: serenity::CreateEmbed) {
    if let Err(cause) = reply.replace(&sctx.http, embed).await {
        // A pessoa pode ter dispensado a mensagem efemera. O vinculo ja foi
        // gravado e aplicado; so o aviso visual se perdeu.
        warn!("[verificacao] nao consegui editar a resposta do /verify: {cause}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descricao_escapa_e_cai_no_nome() {
        let mut link = RobloxLink {
            roblox_id: 42,
            roblox_name: "bob".into(),
            display_name: "[x](evil)".into(),
            ..RobloxLink::default()
        };
        assert_eq!(
            describe(&link),
            "**\\[x](evil)** ([@bob](https://www.roblox.com/users/42/profile))"
        );
        link.display_name = " ".into();
        assert_eq!(
            describe(&link),
            "**bob** ([@bob](https://www.roblox.com/users/42/profile))"
        );
    }

    #[test]
    fn ids_com_prefixo() {
        for id in [START_ID, CHECK_ID, UNLINK_ID] {
            assert!(id.starts_with(PREFIX));
        }
    }

    #[tokio::test]
    async fn polling_novo_substitui_o_anterior() {
        let (stop, mut first) = watch::channel(false);
        polls().insert(777, Poll { id: 1, stop });
        let (stop, second) = watch::channel(false);
        if let Some(old) = polls().insert(777, Poll { id: 2, stop }) {
            old.stop.send_replace(true);
        }
        assert!(*first.borrow_and_update());
        stop_polling(777);
        assert!(*second.borrow());
        assert!(!polls().contains_key(&777));
    }
}
