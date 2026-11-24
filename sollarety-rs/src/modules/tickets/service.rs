//! Sistema de tickets: canal privado por assunto, com quem abriu e a equipe
//! dentro, e um transcript arquivado quando fecha (TicketService.cs).
//!
//! TODO O ESTADO VIVE NO DISCO. O custom id de cada botao carrega o id do
//! ticket, e o resto sai do `TicketStore` - nao ha mapa em memoria em lugar
//! nenhum. E isso que faz um painel publicado continuar funcionando depois de
//! o bot reiniciar; a mesma razao pela qual o /poll deixa os votos nas
//! reacoes em vez de no processo.
//!
//! ESQUEMA DOS CUSTOM IDS:
//!   ticket:open:<tipo>   botao do painel        -> abre o modal
//!   ticket:new:<tipo>    envio daquele modal    -> cria o canal
//!   ticket:claim:<id>    botao dentro do ticket -> assume o atendimento
//!   ticket:close:<id>    botao dentro do ticket -> abre o modal de motivo
//!   ticket:closed:<id>   envio daquele modal    -> fecha de verdade

use std::{fmt::Write as _, sync::Mutex};

use chrono::{DateTime, Duration, Utc};
use poise::serenity_prelude as serenity;
use serenity::{
    ChannelId, ChannelType, CreateEmbed, GuildId, PermissionOverwrite, PermissionOverwriteType,
    Permissions, RoleId, UserId,
};
use tracing::{error, info, warn};

use super::types::{self, TicketType};
use crate::{
    Data, Error, audit_reason, duration, embeds,
    store::{
        GuildSettings, Ticket,
        time::{self, Time},
    },
};

pub const PREFIX: &str = "ticket:";

/// Teto de mensagens no transcript. Acima disso o arquivo vira ilegivel e a
/// coleta, cara.
const TRANSCRIPT_LIMIT: usize = 500;

/// Uma reserva sem canal vale por este tempo (ver `blocking_ticket`).
const RESERVATION_WINDOW_MINUTES: i64 = 2;

/// O que quem participa de um ticket pode fazer no canal dele.
pub const PARTICIPANT_PERMISSIONS: Permissions = Permissions::VIEW_CHANNEL
    .union(Permissions::SEND_MESSAGES)
    .union(Permissions::READ_MESSAGE_HISTORY)
    .union(Permissions::ATTACH_FILES)
    .union(Permissions::EMBED_LINKS)
    .union(Permissions::ADD_REACTIONS);

// ------------------------------------------------------------------
// Config
// ------------------------------------------------------------------

/// Config dos tickets ja validado: os dois ids obrigatorios presentes (e
/// diferentes de zero, o que deixa construir ChannelId/RoleId sem panico).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TicketSettings {
    pub category_id: u64,
    pub staff_role_id: u64,
    pub log_channel_id: Option<u64>,
}

impl TicketSettings {
    pub fn category(&self) -> ChannelId {
        ChannelId::new(self.category_id)
    }

    pub fn staff_role(&self) -> RoleId {
        RoleId::new(self.staff_role_id)
    }
}

/// None quando o sistema esta desligado no servidor: categoria e cargo sao
/// os dois obrigatorios.
pub fn settings_from(settings: Option<&GuildSettings>) -> Option<TicketSettings> {
    let settings = settings?;
    let category = settings.ticket_category_id.unwrap_or(0);
    let staff = settings.ticket_staff_role_id.unwrap_or(0);
    if category == 0 || staff == 0 {
        return None;
    }
    Some(TicketSettings {
        category_id: category,
        staff_role_id: staff,
        log_channel_id: settings.ticket_log_channel_id.filter(|&id| id > 0),
    })
}

/// A configuracao de tickets DAQUELE servidor. Sem I/O: le do snapshot.
pub fn try_get_settings(data: &Data, guild_id: GuildId) -> Option<TicketSettings> {
    settings_from(data.guild_settings.for_guild(guild_id.get()).as_ref())
}

/// Embed de recusa quando o sistema nao esta configurado.
pub fn not_configured() -> CreateEmbed {
    embeds::error(
        "Tickets are off",
        "The ticket system hasn't been set up in this server yet.\n\n\
         Someone with **Manage Server** can turn it on with `/config tickets-category` and \
         `/config tickets-role`. `/config view` shows what's missing.",
    )
}

/// Diz se o membro faz parte da equipe que atende tickets. As permissoes
/// vem da interacao (ja calculadas pelo Discord); sem elas, do cache.
pub fn is_staff(
    cache: &serenity::Cache,
    guild_id: GuildId,
    member: &serenity::Member,
    settings: &TicketSettings,
) -> bool {
    if member.roles.contains(&settings.staff_role()) {
        return true;
    }
    let permissions = member.permissions.or_else(|| {
        cache
            .guild(guild_id)
            .map(|guild| guild.member_permissions(member))
    });
    permissions.is_some_and(|p| p.administrator())
}

// ------------------------------------------------------------------
// Overwrites
// ------------------------------------------------------------------

/// Quem recebe um overwrite: um membro ou um cargo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Member(UserId),
    Role(RoleId),
}

impl Target {
    fn matches(self, kind: &PermissionOverwriteType) -> bool {
        match (self, kind) {
            (Self::Member(a), PermissionOverwriteType::Member(b)) => a == *b,
            (Self::Role(a), PermissionOverwriteType::Role(b)) => a == *b,
            _ => false,
        }
    }
}

/// O (allow, deny) atual de `target`, ou vazio se nao houver linha.
pub fn current_bits(
    overwrites: &[PermissionOverwrite],
    target: Target,
) -> (Permissions, Permissions) {
    overwrites
        .iter()
        .find(|o| target.matches(&o.kind))
        .map_or((Permissions::empty(), Permissions::empty()), |o| {
            (o.allow, o.deny)
        })
}

/// Libera `bits`, preservando o resto da linha.
pub fn grant(current: (Permissions, Permissions), bits: Permissions) -> (Permissions, Permissions) {
    (current.0 | bits, current.1 - bits)
}

/// Nega `bits`, preservando o resto da linha.
pub fn revoke(
    current: (Permissions, Permissions),
    bits: Permissions,
) -> (Permissions, Permissions) {
    (current.0 - bits, current.1 | bits)
}

/// Os overwrites atuais do canal (cache primeiro, REST se faltar).
pub async fn channel_overwrites(
    ctx: &serenity::Context,
    channel_id: ChannelId,
) -> Result<Vec<PermissionOverwrite>, Error> {
    match channel_id.to_channel(ctx).await?.guild() {
        Some(channel) => Ok(channel.permission_overwrites),
        None => Err("o canal nao e de servidor".into()),
    }
}

/// Grava a linha INTEIRA de `target` (o PUT do Discord substitui), com o
/// motivo no audit log. O metodo do serenity nao aceita motivo, por isso a
/// chamada crua.
pub async fn put_overwrite(
    ctx: &serenity::Context,
    channel_id: ChannelId,
    target: Target,
    bits: (Permissions, Permissions),
    reason: &str,
) -> Result<(), serenity::Error> {
    let (id, kind) = match target {
        Target::Member(user) => (user.get(), 1),
        Target::Role(role) => (role.get(), 0),
    };
    let body = serde_json::json!({
        "allow": bits.0.bits().to_string(),
        "deny": bits.1.bits().to_string(),
        "type": kind,
    });
    ctx.http
        .create_permission(channel_id, serenity::TargetId::new(id), &body, Some(reason))
        .await
}

// ------------------------------------------------------------------
// Abertura
// ------------------------------------------------------------------

/// O ticket aberto que impede `opener` de abrir outro, se houver.
///
/// `channel_id != 0` no teste: entre reservar e gravar o id do canal ha uma
/// janela. Se o processo morre ali (deploy, OOM) ou se a segunda escrita
/// falha, sobra um registro aberto apontando para o canal 0 - e sem esta
/// condicao ele bloquearia aquela pessoa de abrir ticket PARA SEMPRE, com um
/// "continue in <#0>" que o Discord mostra como canal apagado. Passados dois
/// minutos, uma reserva sem canal deixa de valer.
pub fn blocking_ticket(
    tickets: &[Ticket],
    guild_id: u64,
    opener_id: u64,
    now: Time,
) -> Option<&Ticket> {
    tickets.iter().find(|t| {
        t.guild_id == guild_id
            && t.opener_id == opener_id
            && t.is_open()
            && (t.channel_id != 0
                || now - t.opened_at_utc < Duration::minutes(RESERVATION_WINDOW_MINUTES))
    })
}

/// O que o cache diz do servidor, copiado antes de qualquer `.await`.
struct GuildView {
    category_kind: Option<ChannelType>,
    staff_role_exists: bool,
}

/// Cria o canal do ticket e registra. Devolve o canal, ou o embed de recusa
/// quando nao da para abrir. Erro so para falha de disco ao reservar.
///
/// A reserva no arquivo acontece ANTES de criar o canal, e num `update` so:
/// e la dentro que "um ticket aberto por pessoa" e o numero sequencial sao
/// decididos, sob o mesmo lock. Conferindo fora, dois cliques rapidos no
/// botao viram duas tarefas paralelas que passam as duas pela checagem e
/// abrem dois canais.
#[allow(clippy::too_many_arguments)]
pub async fn open(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: GuildId,
    opener: &serenity::User,
    settings: &TicketSettings,
    type_key: &str,
    subject: &str,
    description: Option<&str>,
) -> Result<Result<ChannelId, CreateEmbed>, Error> {
    let Some(kind) = types::find(type_key) else {
        return Ok(Err(embeds::error(
            "Invalid type",
            "This ticket type no longer exists.",
        )));
    };

    // Servidor fora do cache e o caso do "@everyone anulavel com o cache
    // frio" do C#: sem ele nao da para conferir nada, e a resposta honesta e
    // "tente de novo".
    let view = ctx.cache.guild(guild_id).map(|guild| GuildView {
        category_kind: guild.channels.get(&settings.category()).map(|c| c.kind),
        staff_role_exists: guild.roles.contains_key(&settings.staff_role()),
    });
    let Some(view) = view else {
        return Ok(Err(embeds::error(
            "Role not found",
            "I couldn't read this server's @everyone role right now. Try again in a few seconds.",
        )));
    };
    if view.category_kind != Some(ChannelType::Category) {
        return Ok(Err(embeds::error(
            "Invalid category",
            "The configured category no longer exists in this server. Set it again with `/config tickets-category`.",
        )));
    }
    if !view.staff_role_exists {
        return Ok(Err(embeds::error(
            "Invalid role",
            "The configured staff role no longer exists in this server. Set it again with `/config tickets-role`.",
        )));
    }

    // Reserva atomica: ou esta pessoa ja tem um ticket aberto, ou sai daqui
    // com numero e id proprios.
    let reserved = data
        .tickets
        .update(|edit| {
            let now = time::now();
            if let Some(open) =
                blocking_ticket(&edit.file.tickets, guild_id.get(), opener.id.get(), now)
            {
                return Err(open.channel_id);
            }
            // Campo a campo: o Ticket tem um campo privado (o IsOpen do
            // disco), e isso impede a sintaxe `..Default::default()` daqui.
            let mut ticket = Ticket::default();
            ticket.id = edit.file.new_unique_id();
            ticket.number = edit.file.next_number(guild_id.get());
            ticket.guild_id = guild_id.get();
            ticket.opener_id = opener.id.get();
            ticket.kind = kind.key.to_owned();
            ticket.subject = embeds::trim(subject, 200);
            ticket.opened_at_utc = now;
            edit.file.tickets.push(ticket.clone());
            edit.mark_changed();
            Ok(ticket)
        })
        .await?;

    let created = match reserved {
        Ok(ticket) => ticket,
        Err(channel_id) => {
            return Ok(Err(embeds::error(
                "You already have an open ticket",
                format!("Continue in <#{channel_id}>. Close it before opening another one."),
            )));
        }
    };

    let name = format!("{}-{:04}", kind.channel_prefix, created.number);
    let everyone = RoleId::new(guild_id.get());
    let bot_id = ctx.cache.current_user().id;
    let overwrites = vec![
        PermissionOverwrite {
            allow: Permissions::empty(),
            deny: Permissions::VIEW_CHANNEL,
            kind: PermissionOverwriteType::Role(everyone),
        },
        PermissionOverwrite {
            allow: PARTICIPANT_PERMISSIONS,
            deny: Permissions::empty(),
            kind: PermissionOverwriteType::Member(opener.id),
        },
        PermissionOverwrite {
            allow: PARTICIPANT_PERMISSIONS,
            deny: Permissions::empty(),
            kind: PermissionOverwriteType::Role(settings.staff_role()),
        },
        // O proprio bot, explicitamente: se a categoria negar o canal ao bot,
        // o ticket nasceria inacessivel para quem tem de fecha-lo.
        PermissionOverwrite {
            allow: PARTICIPANT_PERMISSIONS
                | Permissions::MANAGE_CHANNELS
                | Permissions::MANAGE_MESSAGES,
            deny: Permissions::empty(),
            kind: PermissionOverwriteType::Member(bot_id),
        },
    ];

    let reason = audit_reason::for_actor(
        &opener.tag(),
        Some(&format!("opened ticket {}", created.id)),
    );
    let builder = serenity::CreateChannel::new(name)
        .kind(ChannelType::Text)
        .category(settings.category())
        // O topic e o encosto para o caso de o tickets.json se perder: da para
        // reconstruir a mao de quem e o canal.
        .topic(format!(
            "Ticket {} — opened by {} ({})",
            created.id,
            opener.tag(),
            opener.id
        ))
        .permissions(overwrites)
        .audit_log_reason(&reason);

    let channel = match guild_id.create_channel(ctx, builder).await {
        Ok(channel) => channel,
        Err(cause) => {
            // O canal nao existe, entao o registro reservado nao pode ficar:
            // ele bloquearia a pessoa de abrir outro, apontando para um canal
            // que nunca nasceu. Falha aqui so vira log, para nao trocar o erro
            // original por um de disco.
            if let Err(inner) = forget(data, &created.id).await {
                error!(
                    "[tickets] nao consegui limpar a reserva {}: {inner}",
                    created.id
                );
            }
            error!("[tickets] falha ao criar o canal de {}: {cause}", opener.id);
            return Ok(Err(embeds::error(
                "Couldn't open the ticket",
                embeds::trim(&cause.to_string(), 500),
            )));
        }
    };

    let recorded = data
        .tickets
        .update(|edit| {
            let Some(stored) = edit.file.tickets.iter_mut().find(|t| t.id == created.id) else {
                return false;
            };
            stored.channel_id = channel.id.get();
            edit.mark_changed();
            true
        })
        .await;
    if let Err(cause) = recorded {
        // O canal existe mas o registro nao sabe o id dele - ninguem
        // conseguiria fechar por ali. Desfaz o canal em vez de deixar um orfao
        // com quem abriu bloqueado atras dele.
        error!(
            "[tickets] canal {} criado, mas nao consegui gravar o id: {cause}",
            channel.id
        );
        let reason = audit_reason::automatic(&format!("ticket {}: failed to register", created.id));
        if let Err(inner) = ctx.http.delete_channel(channel.id, Some(&reason)).await {
            error!("[tickets] e nem consegui apagar {}: {inner}", channel.id);
        }
        if let Err(inner) = forget(data, &created.id).await {
            error!(
                "[tickets] nao consegui limpar a reserva {}: {inner}",
                created.id
            );
        }
        return Ok(Err(embeds::error(
            "Couldn't open the ticket",
            "I couldn't register the ticket. Try again in a few seconds.",
        )));
    }

    if let Err(cause) = channel
        .id
        .send_message(
            ctx,
            opening_message(&created, kind, opener, settings, description),
        )
        .await
    {
        // O canal existe e o ticket esta registrado; so a mensagem de
        // abertura falhou. Nao e motivo para desfazer nada.
        warn!(
            "[tickets] canal {} criado, mas a mensagem de abertura falhou: {cause}",
            channel.id
        );
    }

    log(
        ctx,
        guild_id,
        settings,
        embeds::info(
            format!("Ticket #{:04} opened", created.number),
            format!(
                "<@{}> opened a **{}** ticket in <#{}>.\n**Subject:** {}",
                opener.id,
                kind.label,
                channel.id,
                embeds::safe(&created.subject)
            ),
        ),
    )
    .await;

    Ok(Ok(channel.id))
}

fn opening_message(
    ticket: &Ticket,
    kind: &TicketType,
    opener: &serenity::User,
    settings: &TicketSettings,
    description: Option<&str>,
) -> serenity::CreateMessage {
    let mut text = format!(
        "Opened by <@{}>.\n\n**Subject:** {}",
        opener.id,
        embeds::safe(&ticket.subject)
    );
    if let Some(description) = description.filter(|d| !d.trim().is_empty()) {
        let _ = write!(text, "\n\n{}", embeds::safe_trim(description, 1500));
    }

    let embed = CreateEmbed::new()
        .title(format!(
            "{} Ticket #{:04} — {}",
            kind.emoji, ticket.number, kind.label
        ))
        .description(text)
        .colour(embeds::colors::BLURPLE)
        .footer(serenity::CreateEmbedFooter::new(format!(
            "id {}",
            ticket.id
        )))
        .timestamp(serenity::Timestamp::from(ticket.opened_at_utc));

    serenity::CreateMessage::new()
        // A mencao ao cargo vai no CONTENT: embed nao notifica ninguem, e um
        // ticket que a equipe so descobre por acaso nao serve.
        .content(format!("<@&{}>", settings.staff_role_id))
        .embed(embed)
        .components(vec![serenity::CreateActionRow::Buttons(vec![
            serenity::CreateButton::new(format!("{PREFIX}claim:{}", ticket.id))
                .style(serenity::ButtonStyle::Secondary)
                .label("Claim")
                .emoji(serenity::ReactionType::Unicode("🙋".into())),
            serenity::CreateButton::new(format!("{PREFIX}close:{}", ticket.id))
                .style(serenity::ButtonStyle::Danger)
                .label("Close")
                .emoji(serenity::ReactionType::Unicode("🔒".into())),
        ])])
}

/// Apaga um registro reservado cujo canal nunca chegou a existir.
async fn forget(data: &Data, ticket_id: &str) -> Result<bool, Error> {
    data.tickets
        .update(|edit| {
            let before = edit.file.tickets.len();
            edit.file.tickets.retain(|t| t.id != ticket_id);
            if edit.file.tickets.len() == before {
                return false;
            }
            edit.mark_changed();
            true
        })
        .await
}

// ------------------------------------------------------------------
// Fechamento
// ------------------------------------------------------------------

/// O que o fechamento conseguiu fazer. Cada estado pede uma resposta
/// diferente.
#[derive(Debug, PartialEq, Eq)]
pub enum CloseOutcome {
    /// Transcript arquivado e canal apagado.
    Archived,
    /// Ticket fechado (ou nao), mas o canal continua de pe; o texto explica.
    KeptChannel(String),
}

/// Arquiva e fecha.
///
/// A ORDEM NAO E NEGOCIAVEL: o transcript sobe primeiro, e so depois o canal
/// e apagado. Se o arquivamento falhar, o canal FICA - apagar mesmo assim
/// destruiria a conversa inteira sem deixar nada no lugar, que e o oposto do
/// que arquivar quer dizer. Sem canal de log configurado nao ha arquivamento
/// possivel, e o mesmo raciocinio vale: tranca e avisa.
#[allow(clippy::too_many_arguments)]
pub async fn close(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: GuildId,
    channel_id: ChannelId,
    ticket: &Ticket,
    closed_by: &serenity::User,
    reason: Option<&str>,
    settings: &TicketSettings,
) -> Result<CloseOutcome, Error> {
    // O canal tem de ser o do ticket. Os dois chamadores passam o canal da
    // interacao, entao hoje bate sempre - e justamente por isso a
    // divergencia so apareceria como um canal errado apagado.
    if channel_id.get() != ticket.channel_id {
        warn!(
            "[tickets] recusado: pedido de fechar o ticket {} (canal {}) veio do canal {channel_id}.",
            ticket.id, ticket.channel_id
        );
        return Ok(CloseOutcome::KeptChannel(
            "This channel doesn't belong to that ticket.".into(),
        ));
    }

    // RESERVA, e nao fechamento. O resultado E conferido: descartar o bool
    // aqui deixava dois cliques simultaneos rodarem o fechamento inteiro em
    // paralelo - dois transcripts no log, e o segundo delete batendo num
    // canal ja apagado, o que virava um "couldn't delete" assustador para um
    // fechamento que deu certo.
    let reserved = data
        .tickets
        .update(|edit| {
            let now = time::now();
            let Some(stored) = edit.file.tickets.iter_mut().find(|t| t.id == ticket.id) else {
                return false;
            };
            if !stored.is_open() || stored.is_closing(now) {
                return false;
            }
            stored.closing_since = Some(now);
            edit.mark_changed();
            true
        })
        .await?;
    if !reserved {
        return Ok(CloseOutcome::KeptChannel(
            "Someone else is already closing this ticket, or it has already been closed.".into(),
        ));
    }

    let Some(log_channel) = settings.log_channel_id else {
        release_closing(data, &ticket.id).await?;
        lock(ctx, guild_id, channel_id, ticket, closed_by).await;
        return Ok(CloseOutcome::KeptChannel(
            "No ticket log channel is set up (`/config tickets-log`), so there was nowhere to archive the conversation. \
             The channel was locked and has to be deleted manually."
                .into(),
        ));
    };

    let (transcript, message_count) = match build_transcript(ctx, channel_id, ticket).await {
        Ok(built) => built,
        Err(cause) => {
            error!(
                "[tickets] falha ao montar o transcript de {}: {cause}",
                ticket.id
            );
            release_closing(data, &ticket.id).await?;
            lock(ctx, guild_id, channel_id, ticket, closed_by).await;
            return Ok(CloseOutcome::KeptChannel(format!(
                "I couldn't read the messages to archive them: {}. The channel was locked instead of deleted.",
                embeds::trim(&cause.to_string(), 300)
            )));
        }
    };

    let archived = archive(
        ctx,
        guild_id,
        ChannelId::new(log_channel),
        ticket,
        closed_by,
        reason,
        transcript,
        message_count,
    )
    .await;
    if let Err(cause) = archived {
        error!(
            "[tickets] falha ao arquivar o ticket {}: {cause}",
            ticket.id
        );
        release_closing(data, &ticket.id).await?;
        lock(ctx, guild_id, channel_id, ticket, closed_by).await;
        return Ok(CloseOutcome::KeptChannel(format!(
            "I couldn't send the transcript to the log channel: {}. \
             The channel was locked instead of deleted so the conversation isn't lost.",
            embeds::trim(&cause.to_string(), 300)
        )));
    }

    // A conversa esta salva: agora sim o ticket esta fechado. Antes deste
    // ponto qualquer falha devolve um ticket ABERTO, que da para tentar
    // fechar de novo.
    data.tickets
        .update(|edit| {
            let Some(stored) = edit.file.tickets.iter_mut().find(|t| t.id == ticket.id) else {
                return false;
            };
            stored.closed_at_utc = Some(time::now());
            stored.closed_by_id = Some(closed_by.id.get());
            stored.closing_since = None;
            edit.mark_changed();
            true
        })
        .await?;

    let reason_text = audit_reason::for_actor(
        &closed_by.tag(),
        Some(&format!("closed ticket {}", ticket.id)),
    );
    if let Err(cause) = ctx
        .http
        .delete_channel(channel_id, Some(&reason_text))
        .await
    {
        // O transcript esta no log e o ticket esta fechado; so o canal sobrou.
        // Trancar e obrigatorio: sem isso, gente continua conversando num
        // canal que o bot ja arquivou e nao reconhece mais, e essa conversa se
        // perde quando alguem o apagar.
        error!("[tickets] transcript salvo, mas nao consegui apagar {channel_id}: {cause}");
        lock(ctx, guild_id, channel_id, ticket, closed_by).await;
        return Ok(CloseOutcome::KeptChannel(format!(
            "The transcript was archived and the ticket is closed, but I couldn't delete the channel: {}. Delete it manually.",
            embeds::trim(&cause.to_string(), 300)
        )));
    }

    Ok(CloseOutcome::Archived)
}

/// Manda o resumo e o transcript ao canal de log, conferindo o servidor.
#[allow(clippy::too_many_arguments)]
async fn archive(
    ctx: &serenity::Context,
    guild_id: GuildId,
    log_channel: ChannelId,
    ticket: &Ticket,
    closed_by: &serenity::User,
    reason: Option<&str>,
    transcript: String,
    message_count: usize,
) -> Result<(), Error> {
    let log = log_channel.to_channel(ctx).await?.guild();
    let Some(log) = log.filter(|c| c.guild_id == guild_id) else {
        return Err("the log channel isn't in this server".into());
    };

    let embed = closing_embed(
        ticket,
        closed_by.id.get(),
        reason,
        message_count,
        Utc::now(),
    );
    let file = serenity::CreateAttachment::bytes(
        transcript.into_bytes(),
        format!("ticket-{:04}-{}.txt", ticket.number, ticket.id),
    );
    log.id
        .send_message(
            ctx,
            serenity::CreateMessage::new().embed(embed).add_file(file),
        )
        .await?;
    Ok(())
}

fn closing_embed(
    ticket: &Ticket,
    closed_by: u64,
    reason: Option<&str>,
    message_count: usize,
    now: DateTime<Utc>,
) -> CreateEmbed {
    let reason = match reason.filter(|r| !r.trim().is_empty()) {
        Some(reason) => embeds::safe_trim(reason, 900),
        None => "*not provided*".into(),
    };
    let claimed = ticket
        .claimed_by_id
        .map_or_else(|| "*nobody*".to_owned(), |id| format!("<@{id}>"));

    CreateEmbed::new()
        .title(format!(
            "Ticket #{:04} closed — {}",
            ticket.number,
            types::describe(&ticket.kind)
        ))
        .colour(embeds::colors::ORANGE)
        .timestamp(serenity::Timestamp::from(now))
        .field(
            "Opened by",
            format!("<@{0}>\n`{0}`", ticket.opener_id),
            true,
        )
        .field("Closed by", format!("<@{closed_by}>\n`{closed_by}`"), true)
        .field("Claimed by", claimed, true)
        .field("Subject", embeds::safe_trim(&ticket.subject, 900), false)
        .field("Closing reason", reason, false)
        .field("Duration", describe_duration(ticket, now), true)
        .field("Messages", message_count.to_string(), true)
        .footer(serenity::CreateEmbedFooter::new(format!(
            "id {}",
            ticket.id
        )))
}

fn describe_duration(ticket: &Ticket, now: DateTime<Utc>) -> String {
    let span = ticket.closed_at_utc.unwrap_or(now) - ticket.opened_at_utc;
    duration::describe(span.to_std().unwrap_or_default())
}

/// Solta a reserva de fechamento. Chamado em toda saida que deixa o canal
/// de pe: o ticket volta a ser um ticket ABERTO, e da para tentar fechar de
/// novo depois de arrumar o que falhou.
async fn release_closing(data: &Data, ticket_id: &str) -> Result<bool, Error> {
    data.tickets
        .update(|edit| {
            let Some(stored) = edit.file.tickets.iter_mut().find(|t| t.id == ticket_id) else {
                return false;
            };
            if stored.closing_since.is_none() {
                return false;
            }
            stored.closing_since = None;
            edit.mark_changed();
            true
        })
        .await
}

/// Tranca o canal quando nao deu para arquivar. Preserva o resto do
/// overwrite de cada um, mexendo so no bit de escrita - o mesmo cuidado do
/// /lock. Nunca falha: o pior caso vira log.
async fn lock(
    ctx: &serenity::Context,
    guild_id: GuildId,
    channel_id: ChannelId,
    ticket: &Ticket,
    closed_by: &serenity::User,
) {
    if let Err(cause) = lock_inner(ctx, guild_id, channel_id, ticket, closed_by).await {
        error!("[tickets] nao consegui trancar {channel_id}: {cause}");
    }
}

async fn lock_inner(
    ctx: &serenity::Context,
    guild_id: GuildId,
    channel_id: ChannelId,
    ticket: &Ticket,
    closed_by: &serenity::User,
) -> Result<(), Error> {
    let reason = audit_reason::for_actor(
        &closed_by.tag(),
        Some(&format!("ticket {} closed", ticket.id)),
    );
    let overwrites = channel_overwrites(ctx, channel_id).await?;

    // Quem abriu, se ainda estiver no servidor. Uma falha AQUI nao pode
    // impedir o passo do @everyone: no C# antigo, quem abriu ter saido do
    // servidor fazia a busca lancar e o canal nao era trancado DE JEITO
    // NENHUM, que era o pior dos casos.
    if ticket.opener_id != 0 {
        let opener = UserId::new(ticket.opener_id);
        match crate::hierarchy::try_get_member(ctx, guild_id, opener).await {
            Ok(Some(_)) => {
                let target = Target::Member(opener);
                let bits = revoke(
                    current_bits(&overwrites, target),
                    Permissions::SEND_MESSAGES,
                );
                put_overwrite(ctx, channel_id, target, bits, &reason).await?;
            }
            Ok(None) => {}
            Err(cause) => {
                warn!(
                    "[tickets] aviso: nao consegui ler quem abriu o ticket {} para trancar: {cause}",
                    ticket.id
                );
            }
        }
    }

    // O @everyone tambem, sempre. Trancar so quem abriu deixava o canal
    // aberto para todo mundo que tivesse sido chamado com /ticket-add.
    let everyone = Target::Role(RoleId::new(guild_id.get()));
    let bits = revoke(
        current_bits(&overwrites, everyone),
        Permissions::SEND_MESSAGES,
    );
    put_overwrite(ctx, channel_id, everyone, bits, &reason).await?;
    Ok(())
}

// ------------------------------------------------------------------
// Transcript
// ------------------------------------------------------------------

/// Uma mensagem como entra no transcript; separado do `Message` do serenity
/// para o formato poder ser testado.
pub struct TranscriptMessage {
    pub at: DateTime<Utc>,
    pub author: String,
    pub author_id: u64,
    pub content: String,
    pub attachments: Vec<(String, String)>,
    pub embeds: Vec<String>,
}

impl From<&serenity::Message> for TranscriptMessage {
    fn from(m: &serenity::Message) -> Self {
        Self {
            at: *m.timestamp,
            author: m.author.tag(),
            author_id: m.author.id.get(),
            content: m.content.clone(),
            attachments: m
                .attachments
                .iter()
                .map(|a| (a.filename.clone(), a.url.clone()))
                .collect(),
            embeds: m
                .embeds
                .iter()
                .map(|e| e.title.clone().unwrap_or_default())
                .collect(),
        }
    }
}

/// Monta o transcript em ordem cronologica. Pagina para tras a partir da
/// mensagem mais nova, com teto: um canal de ticket que virou conversa de mil
/// mensagens nao pode transformar um clique em minutos de REST.
///
/// O conteudo vem completo mesmo sem o intent MessageContent: este e um GET
/// REST em /channels/{id}/messages, e aquele intent so restringe o que o
/// GATEWAY entrega.
async fn build_transcript(
    ctx: &serenity::Context,
    channel_id: ChannelId,
    ticket: &Ticket,
) -> Result<(String, usize), Error> {
    let mut collected: Vec<serenity::Message> = Vec::new();
    let mut batch = channel_id
        .messages(&ctx.http, serenity::GetMessages::new().limit(100))
        .await?;
    let mut truncated = false;

    while !batch.is_empty() {
        let full = batch.len() >= 100;
        let oldest = batch.last().map(|m| m.id);
        collected.extend(batch);

        // Fim do canal: nao ha mais o que buscar.
        if !full {
            break;
        }

        // Bateu no teto E ainda havia mais. So `collected >= limite` como
        // condicao do laco nunca deixava a contagem ULTRAPASSAR o teto, e o
        // aviso de truncagem virava codigo morto - um ticket de cinco mil
        // mensagens saia cortado em silencio.
        if collected.len() >= TRANSCRIPT_LIMIT {
            truncated = true;
            break;
        }

        let Some(oldest) = oldest else { break };
        batch = channel_id
            .messages(
                &ctx.http,
                serenity::GetMessages::new().before(oldest).limit(100),
            )
            .await?;
    }

    collected.truncate(TRANSCRIPT_LIMIT);
    collected.sort_by_key(|m| m.id);
    let messages: Vec<TranscriptMessage> = collected.iter().map(Into::into).collect();
    Ok((
        render_transcript(ticket, &messages, truncated, Utc::now()),
        messages.len(),
    ))
}

pub fn render_transcript(
    ticket: &Ticket,
    messages: &[TranscriptMessage],
    truncated: bool,
    now: DateTime<Utc>,
) -> String {
    const STAMP: &str = "%Y-%m-%d %H:%M:%S";
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Ticket #{:04} ({}) — {}",
        ticket.number,
        ticket.id,
        types::describe(&ticket.kind)
    );
    let _ = writeln!(out, "Subject: {}", ticket.subject);
    let _ = writeln!(out, "Opened by: {}", ticket.opener_id);
    let _ = writeln!(out, "Opened at: {} UTC", ticket.opened_at_utc.format(STAMP));
    let _ = writeln!(out, "Closed at: {} UTC", now.format(STAMP));
    if truncated {
        let _ = writeln!(
            out,
            "WARNING: only the {TRANSCRIPT_LIMIT} most recent messages are included in this file."
        );
    }
    out.push_str(&"-".repeat(72));
    out.push_str("\n\n");

    for m in messages {
        let _ = writeln!(
            out,
            "[{}] {} ({}):",
            m.at.format(STAMP),
            m.author,
            m.author_id
        );
        // Mensagem sem texto (so anexo, so embed, so figurinha) e comum.
        if m.content.trim().is_empty() {
            out.push_str("    (no text content)\n");
        } else {
            let _ = writeln!(out, "    {}", m.content.replace('\n', "\n    "));
        }
        for (name, url) in &m.attachments {
            let _ = writeln!(out, "    [attachment] {name} — {url}");
        }
        for title in &m.embeds {
            let _ = writeln!(out, "    [embed] {title}");
        }
        out.push('\n');
    }
    out
}

// ------------------------------------------------------------------
// Diagnostico de subida
// ------------------------------------------------------------------

/// Sem isto, cada jeito de errar a configuracao produz o MESMO sintoma:
/// /ticket recusa e ninguem sabe qual das cinco coisas esta errada.
///
/// Junta TODOS os problemas numa lista em vez de parar no primeiro - a licao
/// do vigia de canal: quem arruma um, reinicia e descobre o seguinte, um de
/// cada vez.
///
/// Roda no CacheReady, que re-dispara a cada reconexao, entao o relatorio so
/// e escrito quando MUDA.
static LAST_REPORT: Mutex<Option<String>> = Mutex::new(None);

pub async fn report_status(ctx: &serenity::Context, data: &Data) {
    if let Err(cause) = reconcile(ctx, data).await {
        error!("[tickets] falha ao conferir a configuracao: {cause}");
        return;
    }

    let report = build_status_report(ctx, data);
    {
        let mut last = LAST_REPORT.lock().unwrap_or_else(|e| e.into_inner());
        if last.as_deref() == Some(report.as_str()) {
            return;
        }
        *last = Some(report.clone());
    }
    for line in report.lines() {
        if line.contains("AVISO") {
            warn!("{line}");
        } else {
            info!("{line}");
        }
    }
}

/// Decide se um ticket aberto perdeu o canal. `channel_exists` e None quando
/// o servidor nao esta no cache.
///
/// Servidor fora do cache pode ser indisponibilidade passageira do Discord,
/// e nao um servidor que sumiu: nao mexe - fechar por engano perderia o
/// ticket de alguem. Reserva sem canal dentro da janela tambem nao: outra
/// tarefa pode estar criando o canal neste instante.
pub fn is_orphan(ticket: &Ticket, channel_exists: Option<bool>, now: Time) -> bool {
    match channel_exists {
        None | Some(true) => false,
        Some(false) => {
            ticket.channel_id != 0
                || now - ticket.opened_at_utc >= Duration::minutes(RESERVATION_WINDOW_MINUTES)
        }
    }
}

/// Fecha registros de ticket cujo canal nao existe mais.
///
/// Sem isto ha duas maneiras de uma pessoa ficar PERMANENTEMENTE impedida de
/// abrir ticket, as duas invisiveis:
///
///   1. Um administrador apaga o canal do ticket na mao, ou apaga a
///      categoria inteira numa reorganizacao. O registro continua aberto
///      para sempre.
///   2. O processo morre entre criar o canal e gravar o id dele, e sobra um
///      registro aberto com channelId 0.
///
/// Nos dois casos a proxima tentativa daquela pessoa bate no "You already
/// have an open ticket" apontando para um canal que nao existe, e a unica
/// saida era editar o tickets.json a mao. Roda na subida, que e quando o
/// cache de canais esta completo.
async fn reconcile(ctx: &serenity::Context, data: &Data) -> Result<(), Error> {
    let cache = &ctx.cache;
    let closed = data
        .tickets
        .update(|edit| {
            let now = time::now();
            let mut orphans = Vec::new();
            for ticket in edit.file.tickets.iter_mut().filter(|t| t.is_open()) {
                let exists = (ticket.guild_id != 0)
                    .then(|| cache.guild(GuildId::new(ticket.guild_id)))
                    .flatten()
                    .map(|guild| {
                        ticket.channel_id != 0
                            && guild
                                .channels
                                .contains_key(&ChannelId::new(ticket.channel_id))
                    });
                if !is_orphan(ticket, exists, now) {
                    continue;
                }
                ticket.closed_at_utc = Some(now);
                ticket.closing_since = None;
                orphans.push((ticket.id.clone(), ticket.number, ticket.channel_id));
            }
            if !orphans.is_empty() {
                edit.mark_changed();
            }
            orphans
        })
        .await?;

    for (id, number, channel) in closed {
        info!(
            "[tickets] ticket {id} (#{number:04}) fechado na subida: o canal {channel} nao existe mais."
        );
    }
    Ok(())
}

fn build_status_report(ctx: &serenity::Context, data: &Data) -> String {
    // Ordenado: a ordem do cache muda de uma chamada para outra, e o
    // relatorio so e escrito quando muda.
    let mut guilds = ctx.cache.guilds();
    guilds.sort_unstable();
    let bot_id = ctx.cache.current_user().id;

    let lines: Vec<String> = guilds
        .into_iter()
        .filter_map(|guild_id| {
            let settings = try_get_settings(data, guild_id)?;
            let guild = ctx.cache.guild(guild_id)?;
            Some(describe_guild(&guild, bot_id, &settings))
        })
        .collect();

    if lines.is_empty() {
        "[tickets] sistema de tickets desligado em todos os servidores (use /config tickets-category e /config tickets-role).".into()
    } else {
        lines.join("\n")
    }
}

fn describe_guild(guild: &serenity::Guild, bot_id: UserId, settings: &TicketSettings) -> String {
    let mut problems = Vec::new();

    // O C# assumia que a categoria existia e derrubava o relatorio inteiro
    // com um NullReference quando ela tinha sido apagada.
    let category = guild.channels.get(&settings.category());
    match category {
        None => problems.push(format!(
            "a categoria {} nao existe neste servidor",
            settings.category_id
        )),
        Some(c) if c.kind != ChannelType::Category => problems.push(format!(
            "ticketCategoryId aponta para um canal do tipo {:?}, e nao para uma CATEGORIA",
            c.kind
        )),
        Some(_) => {}
    }

    match guild.roles.get(&settings.staff_role()) {
        None => problems.push(format!(
            "o cargo {} nao existe neste servidor",
            settings.staff_role_id
        )),
        // A mensagem de abertura poe a mencao ao cargo no content justamente
        // para NOTIFICAR a equipe. Com o cargo nao mencionavel ela vira texto
        // inerte e ninguem e avisado.
        Some(role) if !role.mentionable => problems.push(format!(
            "o cargo \"{}\" nao e mencionavel, entao a equipe nao recebe notificacao quando um ticket abre",
            role.name
        )),
        Some(_) => {}
    }

    let bot = guild.members.get(&bot_id);
    match bot {
        None => {
            problems.push("nao consegui ler o meu proprio membro para conferir permissoes".into())
        }
        Some(bot) => {
            let permissions = guild.member_permissions(bot);
            if !permissions.manage_channels() {
                problems.push(
                    "estou sem Manage Channels, entao nao consigo criar nem apagar o canal do ticket"
                        .into(),
                );
            }
            if !permissions.manage_roles() {
                problems.push(
                    "estou sem Manage Roles, e sem isso nao da para escrever as permissoes do canal"
                        .into(),
                );
            }
        }
    }

    match settings.log_channel_id {
        None => problems.push(
            "nao ha canal de log (/config tickets-log), entao o fechamento vai trancar o canal em vez de arquivar e apagar"
                .into(),
        ),
        Some(log_id) => match guild.channels.get(&ChannelId::new(log_id)) {
            None => problems.push(format!("o canal de log {log_id} nao esta neste servidor")),
            Some(log) => {
                if let Some(bot) = bot
                    && !guild.user_permissions_in(log, bot).attach_files()
                {
                    problems.push(format!(
                        "nao posso anexar arquivo em \"{}\", e o transcript e um anexo",
                        log.name
                    ));
                }
            }
        },
    }

    summarize(
        &guild.name,
        category.map(|c| c.name.as_str()).unwrap_or_default(),
        &problems,
    )
}

fn summarize(guild_name: &str, category_name: &str, problems: &[String]) -> String {
    if problems.is_empty() {
        format!(
            "[tickets] ligado em \"{guild_name}\": categoria \"{category_name}\", {} tipo(s) de ticket.",
            types::ALL.len()
        )
    } else {
        format!(
            "[tickets] AVISO: configurado em \"{guild_name}\", mas: {}.",
            problems.join("; ")
        )
    }
}

// ------------------------------------------------------------------
// Log
// ------------------------------------------------------------------

/// Manda um embed ao canal de log dos tickets. Nunca falha, e confere o
/// servidor: o bot esta em mais de um, e buscar o canal pelo id resolve
/// canal de qualquer um deles - publicar num canal de outro servidor
/// entregaria conversa privada a quem nao tem nada com aquilo.
pub async fn log(
    ctx: &serenity::Context,
    guild_id: GuildId,
    settings: &TicketSettings,
    embed: CreateEmbed,
) {
    let Some(log_id) = settings.log_channel_id else {
        return;
    };
    let channel = match ChannelId::new(log_id).to_channel(ctx).await {
        Ok(channel) => channel.guild(),
        Err(cause) => {
            error!("[tickets] falha ao registrar no canal de log: {cause}");
            return;
        }
    };
    let Some(channel) = channel.filter(|c| c.guild_id == guild_id) else {
        info!("[tickets] o ticketLogChannelId configurado nao e deste servidor; nada registrado.");
        return;
    };
    if let Err(cause) = channel
        .id
        .send_message(ctx, serenity::CreateMessage::new().embed(embed))
        .await
    {
        error!("[tickets] falha ao registrar no canal de log: {cause}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Time {
        time::parse(text).unwrap()
    }

    fn ticket(guild: u64, opener: u64, channel: u64, opened: &str) -> Ticket {
        let mut t = Ticket::default();
        t.id = "abcd1234".into();
        t.number = 7;
        t.guild_id = guild;
        t.opener_id = opener;
        t.channel_id = channel;
        t.kind = "denuncia".into();
        t.subject = "Spam".into();
        t.opened_at_utc = at(opened);
        t
    }

    #[test]
    fn config_exige_categoria_e_cargo() {
        assert!(settings_from(None).is_none());
        let mut s = GuildSettings::new(1);
        s.ticket_category_id = Some(10);
        assert!(settings_from(Some(&s)).is_none());
        s.ticket_staff_role_id = Some(0);
        assert!(settings_from(Some(&s)).is_none());
        s.ticket_staff_role_id = Some(20);
        s.ticket_log_channel_id = Some(0);
        assert_eq!(
            settings_from(Some(&s)),
            Some(TicketSettings {
                category_id: 10,
                staff_role_id: 20,
                log_channel_id: None
            })
        );
        s.ticket_log_channel_id = Some(30);
        assert_eq!(settings_from(Some(&s)).unwrap().log_channel_id, Some(30));
    }

    #[test]
    fn reserva_sem_canal_vence_em_dois_minutos() {
        let now = at("2026-11-23T12:00:00+00:00");
        let mut tickets = vec![ticket(1, 2, 0, "2026-11-23T11:59:00+00:00")];
        assert!(blocking_ticket(&tickets, 1, 2, now).is_some());
        // Outro servidor, outra pessoa: nao bloqueia.
        assert!(blocking_ticket(&tickets, 9, 2, now).is_none());
        assert!(blocking_ticket(&tickets, 1, 9, now).is_none());

        tickets[0].opened_at_utc = at("2026-11-23T11:57:00+00:00");
        assert!(blocking_ticket(&tickets, 1, 2, now).is_none());
        // Com canal, vale para sempre ate fechar.
        tickets[0].channel_id = 55;
        assert_eq!(blocking_ticket(&tickets, 1, 2, now).unwrap().channel_id, 55);
        tickets[0].closed_at_utc = Some(now);
        assert!(blocking_ticket(&tickets, 1, 2, now).is_none());
    }

    #[test]
    fn orfao_so_com_o_servidor_no_cache() {
        let now = at("2026-11-23T12:00:00+00:00");
        let with_channel = ticket(1, 2, 55, "2026-11-23T11:59:30+00:00");
        assert!(!is_orphan(&with_channel, None, now));
        assert!(!is_orphan(&with_channel, Some(true), now));
        assert!(is_orphan(&with_channel, Some(false), now));

        let fresh = ticket(1, 2, 0, "2026-11-23T11:59:30+00:00");
        assert!(!is_orphan(&fresh, Some(false), now));
        let stale = ticket(1, 2, 0, "2026-11-23T11:50:00+00:00");
        assert!(is_orphan(&stale, Some(false), now));
    }

    #[test]
    fn overwrite_mexe_so_nos_bits_pedidos() {
        let user = UserId::new(5);
        let overwrites = vec![PermissionOverwrite {
            allow: Permissions::USE_EXTERNAL_EMOJIS,
            deny: Permissions::SEND_MESSAGES,
            kind: PermissionOverwriteType::Member(user),
        }];
        let current = current_bits(&overwrites, Target::Member(user));
        assert_eq!(
            current_bits(&overwrites, Target::Role(RoleId::new(5))),
            (Permissions::empty(), Permissions::empty())
        );

        // Adicionar libera o pacote inteiro e tira o deny de escrita, mas
        // guarda o que um admin pos a mao.
        let (allow, deny) = grant(current, PARTICIPANT_PERMISSIONS);
        assert!(allow.contains(PARTICIPANT_PERMISSIONS | Permissions::USE_EXTERNAL_EMOJIS));
        assert!(deny.is_empty());

        let (allow, deny) = revoke((allow, deny), Permissions::VIEW_CHANNEL);
        assert!(!allow.view_channel());
        assert!(allow.send_messages());
        assert_eq!(deny, Permissions::VIEW_CHANNEL);
    }

    #[test]
    fn transcript_no_formato_do_csharp() {
        let t = ticket(1, 2, 55, "2026-11-23T10:00:00+00:00");
        let messages = vec![
            TranscriptMessage {
                at: at("2026-11-23T10:00:01+00:00"),
                author: "ana".into(),
                author_id: 2,
                content: "oi\ntudo bem".into(),
                attachments: vec![("a.png".into(), "https://x/a.png".into())],
                embeds: vec![],
            },
            TranscriptMessage {
                at: at("2026-11-23T10:00:02+00:00"),
                author: "Sollarety#1234".into(),
                author_id: 3,
                content: "  ".into(),
                attachments: vec![],
                embeds: vec!["Titulo".into()],
            },
        ];
        let text = render_transcript(&t, &messages, true, at("2026-11-23T11:00:00+00:00"));
        let expected = format!(
            "Ticket #0007 (abcd1234) — Report\n\
             Subject: Spam\n\
             Opened by: 2\n\
             Opened at: 2026-11-23 10:00:00 UTC\n\
             Closed at: 2026-11-23 11:00:00 UTC\n\
             WARNING: only the 500 most recent messages are included in this file.\n\
             {}\n\n\
             [2026-11-23 10:00:01] ana (2):\n    oi\n    tudo bem\n    [attachment] a.png — https://x/a.png\n\n\
             [2026-11-23 10:00:02] Sollarety#1234 (3):\n    (no text content)\n    [embed] Titulo\n\n",
            "-".repeat(72)
        );
        assert_eq!(text, expected);
    }

    #[test]
    fn duracao_conta_ate_agora_quando_aberto() {
        let t = ticket(1, 2, 55, "2026-11-23T10:00:00+00:00");
        assert_eq!(
            describe_duration(&t, at("2026-11-23T11:30:05+00:00")),
            "1 hour(s), 30 minute(s), 5 second(s)"
        );
    }

    #[test]
    fn resumo_do_relatorio() {
        assert_eq!(
            summarize("Casa", "Tickets", &[]),
            "[tickets] ligado em \"Casa\": categoria \"Tickets\", 4 tipo(s) de ticket."
        );
        assert_eq!(
            summarize("Casa", "", &["a".into(), "b".into()]),
            "[tickets] AVISO: configurado em \"Casa\", mas: a; b."
        );
    }
}
