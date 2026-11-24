//! Botoes e modais dos tickets (TicketComponents.cs).
//!
//! So mexe em custom id que comeca com "ticket:"; o resto e de outro modulo
//! (a paginacao do /logs, a musica, a verificacao...) e passa reto.

use poise::serenity_prelude as serenity;
use serenity::{
    ActionRowComponent, ComponentInteraction, CreateActionRow, CreateEmbed, CreateInputText,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateModal,
    EditInteractionResponse, InputTextStyle, Interaction, ModalInteraction,
};
use tracing::{error, warn};

use super::{
    service::{self, CloseOutcome, PREFIX, TicketSettings},
    types,
};
use crate::{Data, Error, embeds};

/// Botao ou envio de modal: os dois respondem do mesmo jeito.
#[derive(Clone)]
enum Ix {
    Component(Box<ComponentInteraction>),
    Modal(Box<ModalInteraction>),
}

impl Ix {
    fn custom_id(&self) -> &str {
        match self {
            Self::Component(c) => &c.data.custom_id,
            Self::Modal(m) => &m.data.custom_id,
        }
    }

    fn user(&self) -> &serenity::User {
        match self {
            Self::Component(c) => &c.user,
            Self::Modal(m) => &m.user,
        }
    }

    fn guild_id(&self) -> Option<serenity::GuildId> {
        match self {
            Self::Component(c) => c.guild_id,
            Self::Modal(m) => m.guild_id,
        }
    }

    fn member(&self) -> Option<&serenity::Member> {
        match self {
            Self::Component(c) => c.member.as_ref(),
            Self::Modal(m) => m.member.as_ref(),
        }
    }

    fn channel_id(&self) -> serenity::ChannelId {
        match self {
            Self::Component(c) => c.channel_id,
            Self::Modal(m) => m.channel_id,
        }
    }

    async fn create_response(
        &self,
        ctx: &serenity::Context,
        response: CreateInteractionResponse,
    ) -> Result<(), serenity::Error> {
        match self {
            Self::Component(c) => c.create_response(ctx, response).await,
            Self::Modal(m) => m.create_response(ctx, response).await,
        }
    }

    async fn edit_response(
        &self,
        ctx: &serenity::Context,
        embed: CreateEmbed,
    ) -> Result<(), serenity::Error> {
        let edit = EditInteractionResponse::new().embed(embed);
        match self {
            Self::Component(c) => c.edit_response(ctx, edit).await.map(drop),
            Self::Modal(m) => m.edit_response(ctx, edit).await.map(drop),
        }
    }

    /// Valor de um campo de modal pelo custom id; vazio vira None.
    fn modal_value(&self, custom_id: &str) -> Option<String> {
        let Self::Modal(modal) = self else {
            return None;
        };
        modal_value(&modal.data.components, custom_id)
    }
}

/// Procura dentro das action rows: e la que o Discord devolve o campo de um
/// modal montado com action rows (o unico formato que o serenity 0.12 monta).
fn modal_value(rows: &[serenity::ActionRow], custom_id: &str) -> Option<String> {
    rows.iter()
        .flat_map(|row| &row.components)
        .find_map(|component| match component {
            ActionRowComponent::InputText(input) if input.custom_id == custom_id => {
                Some(input.value.clone().unwrap_or_default())
            }
            _ => None,
        })
        .filter(|value| !value.trim().is_empty())
}

/// Ponto de entrada do `on_event`. Volta na hora: o trabalho vai numa tarefa
/// propria, para nao segurar os outros modulos.
pub fn on_interaction(ctx: &serenity::Context, interaction: &Interaction, data: &Data) {
    let ix = match interaction {
        Interaction::Component(c) if c.data.custom_id.starts_with(PREFIX) => {
            Ix::Component(Box::new(c.clone()))
        }
        Interaction::Modal(m) if m.data.custom_id.starts_with(PREFIX) => {
            Ix::Modal(Box::new(m.clone()))
        }
        _ => return,
    };

    let (ctx, data) = (ctx.clone(), data.clone());
    tokio::spawn(async move {
        if let Err(cause) = route(&ctx, &data, &ix).await {
            error!(
                "[tickets] falha ao tratar '{}' de {}: {cause}",
                ix.custom_id(),
                ix.user().id
            );
            try_fail(
                &ctx,
                &ix,
                "Something broke while processing this action. Let the bot's admin know.",
            )
            .await;
        }
    });
}

async fn route(ctx: &serenity::Context, data: &Data, ix: &Ix) -> Result<(), Error> {
    let id = ix.custom_id();
    let parts: Vec<&str> = id.split(':').collect();
    if parts.len() < 3 {
        // Sem ninguem mais para responder, sair calado deixaria o Discord
        // mostrar "essa interacao falhou" sem uma linha de log. Um painel
        // publicado ha meses pode carregar um id de um esquema antigo.
        warn!("[tickets] custom id fora do formato: '{id}'");
        try_fail(
            ctx,
            ix,
            "This button is from an old version of the panel. Ask for it to be reposted.",
        )
        .await;
        return Ok(());
    }
    let (action, argument) = (parts[1], parts[2]);

    let (Some(guild_id), Some(member)) = (ix.guild_id(), ix.member()) else {
        try_fail(ctx, ix, "This only works inside a server.").await;
        return Ok(());
    };

    let Some(settings) = service::try_get_settings(data, guild_id) else {
        respond(ctx, ix, service::not_configured()).await?;
        return Ok(());
    };

    match action {
        "open" => open_modal(ctx, ix, argument).await,
        "new" => create_from_modal(ctx, data, ix, guild_id, argument, &settings).await,
        "claim" => claim(ctx, data, ix, guild_id, member, argument, &settings).await,
        "close" => close_modal(ctx, ix, argument).await,
        "closed" => close_from_modal(ctx, data, ix, guild_id, member, argument, &settings).await,
        _ => {
            warn!("[tickets] acao desconhecida no custom id '{id}'");
            try_fail(
                ctx,
                ix,
                "This button is from an old version of the panel. Ask for it to be reposted.",
            )
            .await;
            Ok(())
        }
    }
}

// ------------------------------------------------------------------
// Abertura
// ------------------------------------------------------------------

async fn open_modal(ctx: &serenity::Context, ix: &Ix, type_key: &str) -> Result<(), Error> {
    let Some(kind) = types::find(type_key) else {
        respond(
            ctx,
            ix,
            embeds::error(
                "Type unavailable",
                "This ticket type no longer exists. Ask the bot's admin to repost the panel.",
            ),
        )
        .await?;
        return Ok(());
    };

    // CreateInputText::new e (estilo, ROTULO, CUSTOM ID): trocar os dois
    // compila sem um pio - sao duas strings - e todo ticket nasceria sem
    // assunto.
    let modal = CreateModal::new(
        format!("{PREFIX}new:{}", kind.key),
        format!("Open ticket — {}", kind.label),
    )
    .components(vec![
        CreateActionRow::InputText(
            CreateInputText::new(InputTextStyle::Short, "Subject", "assunto")
                .placeholder("In a few words, what this is about")
                .min_length(3)
                .max_length(200)
                .required(true),
        ),
        CreateActionRow::InputText(
            CreateInputText::new(InputTextStyle::Paragraph, "Details", "descricao")
                .placeholder("Tell us what happened, in as much detail as you can")
                .max_length(1500)
                .required(false),
        ),
    ]);

    ix.create_response(ctx, CreateInteractionResponse::Modal(modal))
        .await?;
    Ok(())
}

async fn create_from_modal(
    ctx: &serenity::Context,
    data: &Data,
    ix: &Ix,
    guild_id: serenity::GuildId,
    type_key: &str,
    settings: &TicketSettings,
) -> Result<(), Error> {
    defer(ctx, ix, true).await?;

    let subject = ix
        .modal_value("assunto")
        .unwrap_or_else(|| "(no subject)".into());
    let description = ix.modal_value("descricao");

    let opened = service::open(
        ctx,
        data,
        guild_id,
        ix.user(),
        settings,
        type_key,
        &subject,
        description.as_deref(),
    )
    .await?;

    let embed = match opened {
        Ok(channel) => embeds::ok("Ticket opened", format!("Continue in <#{channel}>.")),
        Err(refusal) => refusal,
    };
    ix.edit_response(ctx, embed).await?;
    Ok(())
}

// ------------------------------------------------------------------
// Assumir
// ------------------------------------------------------------------

/// O que o clique em "Claim" encontrou no arquivo.
enum Claim {
    Unknown,
    Closed,
    AlreadyTaken(Option<u64>),
    Taken,
}

#[allow(clippy::too_many_arguments)]
async fn claim(
    ctx: &serenity::Context,
    data: &Data,
    ix: &Ix,
    guild_id: serenity::GuildId,
    member: &serenity::Member,
    ticket_id: &str,
    settings: &TicketSettings,
) -> Result<(), Error> {
    if !service::is_staff(&ctx.cache, guild_id, member, settings) {
        respond(
            ctx,
            ix,
            embeds::error("Staff only", "Only staff can claim a ticket."),
        )
        .await?;
        return Ok(());
    }

    // Defer ANTES de mexer no arquivo. O Discord da tres segundos para a
    // interacao ser reconhecida, e o arquivo pode estar travado por uma
    // abertura ou um fechamento. Estourando o prazo, a pessoa via "a
    // aplicacao nao respondeu" com o claim JA GRAVADO, clicava de novo e
    // ouvia "Already claimed". Publico de proposito: a equipe ve quem assumiu.
    defer(ctx, ix, false).await?;

    let user_id = member.user.id.get();
    let claimed =
        data.tickets
            .update(|edit| {
                // Preso ao servidor, como o by_id: duas regras de busca diferentes
                // para a mesma chave deixam de dar no mesmo um dia.
                let Some(stored) =
                    edit.file.tickets.iter_mut().find(|t| {
                        t.guild_id == guild_id.get() && t.id.eq_ignore_ascii_case(ticket_id)
                    })
                else {
                    return Claim::Unknown;
                };
                if !stored.is_open() {
                    return Claim::Closed;
                }
                // Ja assumido, inclusive pela propria pessoa: nao rouba, e
                // reclicar nao reescreve o arquivo nem posta um segundo "Ticket
                // claimed". A conferencia mora dentro do lock porque dois cliques
                // simultaneos passariam os dois por uma checagem feita fora.
                if stored.claimed_by_id.is_some() {
                    return Claim::AlreadyTaken(stored.claimed_by_id);
                }
                stored.claimed_by_id = Some(user_id);
                edit.mark_changed();
                Claim::Taken
            })
            .await?;

    let embed = match claimed {
        Claim::Unknown => embeds::error(
            "Unknown ticket",
            "I couldn't find this ticket in the records. It may have been closed.",
        ),
        Claim::Closed => embeds::error("Ticket closed", "This ticket has already been closed."),
        Claim::AlreadyTaken(by) => embeds::info(
            "Already claimed",
            format!(
                "<@{}> is already handling this ticket.",
                by.unwrap_or_default()
            ),
        ),
        Claim::Taken => embeds::ok(
            "Ticket claimed",
            format!("<@{user_id}> will handle this ticket."),
        ),
    };
    ix.edit_response(ctx, embed).await?;
    Ok(())
}

// ------------------------------------------------------------------
// Fechamento
// ------------------------------------------------------------------

async fn close_modal(ctx: &serenity::Context, ix: &Ix, ticket_id: &str) -> Result<(), Error> {
    // O modal e a confirmacao. Fechar apaga o canal, e um botao vermelho que
    // destroi a conversa com um clique so e cilada.
    let modal =
        CreateModal::new(format!("{PREFIX}closed:{ticket_id}"), "Close ticket").components(vec![
            CreateActionRow::InputText(
                CreateInputText::new(InputTextStyle::Paragraph, "Reason (optional)", "motivo")
                    .placeholder("How was it resolved?")
                    .max_length(500)
                    .required(false),
            ),
        ]);
    ix.create_response(ctx, CreateInteractionResponse::Modal(modal))
        .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn close_from_modal(
    ctx: &serenity::Context,
    data: &Data,
    ix: &Ix,
    guild_id: serenity::GuildId,
    member: &serenity::Member,
    ticket_id: &str,
    settings: &TicketSettings,
) -> Result<(), Error> {
    defer(ctx, ix, true).await?;

    let reason = ix.modal_value("motivo");
    let ticket = data
        .tickets
        .by_id(guild_id.get(), ticket_id)
        .await?
        .filter(|t| t.is_open());
    let Some(ticket) = ticket else {
        ix.edit_response(
            ctx,
            embeds::error(
                "Ticket unavailable",
                "This ticket no longer exists or has already been closed.",
            ),
        )
        .await?;
        return Ok(());
    };

    if ticket.opener_id != member.user.id.get()
        && !service::is_staff(&ctx.cache, guild_id, member, settings)
    {
        ix.edit_response(
            ctx,
            embeds::error(
                "No permission",
                "Only the person who opened the ticket or the staff can close it.",
            ),
        )
        .await?;
        return Ok(());
    }

    let outcome = service::close(
        ctx,
        data,
        guild_id,
        ix.channel_id(),
        &ticket,
        &member.user,
        reason.as_deref(),
        settings,
    )
    .await?;

    match outcome {
        CloseOutcome::KeptChannel(detail) => {
            ix.edit_response(ctx, embeds::error("Closed, but not archived", detail))
                .await?;
        }
        CloseOutcome::Archived => {
            // O canal acabou de ser apagado: nao ha resposta que sobreviva, e
            // a falha da edicao e esperada, nao do fechamento.
            let _ = ix
                .edit_response(
                    ctx,
                    embeds::ok(
                        "Ticket closed",
                        "The conversation was archived in the log channel.",
                    ),
                )
                .await;
        }
    }
    Ok(())
}

// ------------------------------------------------------------------
// Respostas
// ------------------------------------------------------------------

async fn defer(ctx: &serenity::Context, ix: &Ix, ephemeral: bool) -> Result<(), Error> {
    ix.create_response(
        ctx,
        CreateInteractionResponse::Defer(
            CreateInteractionResponseMessage::new().ephemeral(ephemeral),
        ),
    )
    .await?;
    Ok(())
}

async fn respond(ctx: &serenity::Context, ix: &Ix, embed: CreateEmbed) -> Result<(), Error> {
    ix.create_response(
        ctx,
        CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new()
                .embed(embed)
                .ephemeral(true),
        ),
    )
    .await?;
    Ok(())
}

/// Ultimo recurso quando algo estourou. Sem isto o clique morre em "a
/// aplicacao nao respondeu" e a pessoa nao sabe se funcionou.
async fn try_fail(ctx: &serenity::Context, ix: &Ix, message: &str) {
    if respond(ctx, ix, embeds::error("Error", message))
        .await
        .is_ok()
    {
        return;
    }
    // Ja tinha deferido: editar e o unico caminho que resta.
    if let Err(cause) = ix.edit_response(ctx, embeds::error("Error", message)).await {
        warn!("[tickets] nao consegui avisar sobre a falha: {cause}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(json: serde_json::Value) -> Vec<serenity::ActionRow> {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn le_o_campo_certo_e_vazio_vira_none() {
        let rows = rows(serde_json::json!([
            { "type": 1, "components": [
                { "type": 4, "custom_id": "assunto", "value": "Preciso de ajuda" }
            ]},
            { "type": 1, "components": [
                { "type": 4, "custom_id": "descricao", "value": "   " }
            ]}
        ]));
        assert_eq!(
            modal_value(&rows, "assunto").as_deref(),
            Some("Preciso de ajuda")
        );
        assert_eq!(modal_value(&rows, "descricao"), None);
        assert_eq!(modal_value(&rows, "motivo"), None);
    }
}
