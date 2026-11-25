//! /send: o bot escreve num canal o texto que a staff digitou (o
//! send_message do painel do ccore).
//!
//! O bot costuma ter mais acesso que quem pede. Por isso a conta e feita no
//! CANAL DE DESTINO: quem nao pode ver e escrever la nao usa o bot como
//! atalho, e a mencao de cargo segue o que a pessoa poderia mencionar com as
//! proprias maos.

use std::collections::HashSet;

use poise::serenity_prelude as serenity;

use super::{gate, modal};
use crate::{Context, Error, embeds, modlog};

/// Limite do Discord por mensagem (unidades UTF-16).
const DISCORD_MESSAGE: usize = 2000;
/// O maximo de um campo paragrafo de modal.
const MAX_TEXT: u16 = 4000;
/// O Discord aceita no maximo 100 ids em allowed_mentions.roles.
const MAX_ALLOWED_ROLES: usize = 100;

/// Posts a message as the bot in a channel
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_MESSAGES",
    required_permissions = "MANAGE_MESSAGES",
    required_bot_permissions = "SEND_MESSAGES"
)]
pub async fn send(
    ctx: Context<'_>,
    #[description = "Where to post the message"]
    #[channel_types("Text", "News")]
    channel: serenity::Channel,
) -> Result<(), Error> {
    if !gate::ensure(ctx).await? {
        return Ok(());
    }
    let guild_id = ctx
        .guild_id()
        .ok_or("comando de servidor fora de servidor")?;
    let channel_id = channel.id();

    // Antes do modal, para ninguem escrever 4000 caracteres e so entao
    // descobrir que nao pode postar la.
    if let Some(refusal) = check(ctx, guild_id, channel_id).await? {
        ctx.send(
            poise::CreateReply::default()
                .embed(refusal.embed())
                .ephemeral(true),
        )
        .await?;
        refusal.record(ctx, guild_id, channel_id).await;
        return Ok(());
    }

    let Some((submit, text)) = modal::ask(
        ctx,
        modal::Ask {
            title: "Send a message",
            label: "Message",
            placeholder: "Posted as the bot. Users can be pinged; @everyone and @here never are.",
            max_length: MAX_TEXT,
        },
    )
    .await?
    else {
        return Ok(());
    };

    modal::guard(ctx, &submit, async {
        post(ctx, &submit, guild_id, channel_id, &text).await
    })
    .await;
    Ok(())
}

async fn post(
    ctx: Context<'_>,
    submit: &serenity::ModalInteraction,
    guild_id: serenity::GuildId,
    channel_id: serenity::ChannelId,
    text: &str,
) -> Result<(), Error> {
    if text.trim().is_empty() {
        return modal::finish(
            ctx.http(),
            submit,
            embeds::error("Empty message", "There's nothing to send."),
        )
        .await;
    }

    // De novo: o modal pode ter ficado aberto ate 15 minutos, e cargo ou
    // permissao do canal podem ter mudado nesse meio tempo.
    if let Some(refusal) = check(ctx, guild_id, channel_id).await? {
        refusal.record(ctx, guild_id, channel_id).await;
        return modal::finish(ctx.http(), submit, refusal.embed()).await;
    }
    let allowed = allowed_mentions(ctx, guild_id, channel_id, text).await?;

    let parts = split_for_discord(text);
    let mut first: Option<serenity::Message> = None;
    for (i, part) in parts.iter().enumerate() {
        let message = serenity::CreateMessage::new()
            .content(part)
            .allowed_mentions(allowed.clone());
        match channel_id.send_message(ctx.http(), message).await {
            Ok(sent) => {
                first.get_or_insert(sent);
            }
            Err(cause) => {
                // Parte postada fica postada: dizer so "falhou" esconderia
                // que metade da mensagem ja esta no canal.
                let posted = if i == 0 {
                    "Nothing was posted.".to_owned()
                } else {
                    format!("Only {i} of {} part(s) were posted.", parts.len())
                };
                tracing::warn!(
                    "[admin] /send de {} para {channel_id} falhou na parte {}: {cause}",
                    ctx.author().id,
                    i + 1
                );
                return modal::finish(
                    ctx.http(),
                    submit,
                    embeds::error(
                        "Failed to send",
                        format!(
                            "{posted}\n\nDetails: {}",
                            embeds::trim(&cause.to_string(), 400)
                        ),
                    ),
                )
                .await;
            }
        }
    }

    let link = first
        .as_ref()
        .map(serenity::Message::link)
        .unwrap_or_default();
    let parts_note = if parts.len() > 1 {
        format!(
            " in {} parts (Discord's limit is 2000 characters each)",
            parts.len()
        )
    } else {
        String::new()
    };
    modal::finish(
        ctx.http(),
        submit,
        embeds::ok(
            "Message sent",
            format!("Posted in <#{channel_id}>{parts_note}.\n[Jump to the message]({link})"),
        ),
    )
    .await?;

    let extra = format!(
        "Channel: <#{channel_id}> · {} part(s) · {link}\n\n{}",
        parts.len(),
        embeds::trim(text, 600)
    );
    modlog::record(
        ctx,
        &ctx.data().guild_settings,
        guild_id,
        modlog::Entry::new("Message sent", ctx.author())
            .no_target_label("—")
            .extra(&extra),
    )
    .await;
    Ok(())
}

/// Por que o /send nao pode postar no canal.
enum Refusal {
    WrongChannel,
    Actor,
    Bot,
}

impl Refusal {
    fn embed(&self) -> serenity::CreateEmbed {
        match self {
            Self::WrongChannel => embeds::error(
                "Invalid channel",
                "That channel isn't a text channel of this server.",
            ),
            Self::Actor => embeds::error(
                "Not allowed there",
                "You can't view and send messages in that channel yourself, so I won't post there for you.",
            ),
            Self::Bot => embeds::error(
                "I can't post there",
                "I don't have View Channel and Send Messages in that channel. Check my role and the channel's permissions.",
            ),
        }
    }

    /// So a recusa de QUEM PEDE vai para o registro: e a tentativa de usar o
    /// bot para escrever onde a pessoa nao escreve. Falta de permissao do bot
    /// e configuracao, nao conduta.
    async fn record(
        &self,
        ctx: Context<'_>,
        guild_id: serenity::GuildId,
        channel_id: serenity::ChannelId,
    ) {
        if !matches!(self, Self::Actor) {
            return;
        }
        let extra = format!(
            "Refused /send to <#{channel_id}>: they can't view and send messages there themselves."
        );
        modlog::record(
            ctx,
            &ctx.data().guild_settings,
            guild_id,
            modlog::Entry::new("Send refused", ctx.author())
                .no_target_label("—")
                .extra(&extra),
        )
        .await;
    }
}

/// Permissoes efetivas de quem pede e do bot NO CANAL de destino.
async fn check(
    ctx: Context<'_>,
    guild_id: serenity::GuildId,
    channel_id: serenity::ChannelId,
) -> Result<Option<Refusal>, Error> {
    let actor = gate::actor(ctx).await?;
    let bot = gate::bot_member(ctx.serenity_context(), guild_id).await;

    // O cache nao atravessa .await: daqui para baixo e sincrono.
    let guild = ctx.guild().ok_or("servidor fora do cache")?;
    let Some(channel) = guild.channels.get(&channel_id) else {
        return Ok(Some(Refusal::WrongChannel));
    };
    if channel.guild_id != guild_id
        || !matches!(
            channel.kind,
            serenity::ChannelType::Text | serenity::ChannelType::News
        )
    {
        return Ok(Some(Refusal::WrongChannel));
    }
    if !gate::can_post(guild.user_permissions_in(channel, &actor)) {
        return Ok(Some(Refusal::Actor));
    }
    match bot {
        Some(bot) if gate::can_post(guild.user_permissions_in(channel, &bot)) => Ok(None),
        _ => Ok(Some(Refusal::Bot)),
    }
}

/// Usuarios sempre; cargo so o que a pessoa poderia mencionar (mencionavel,
/// ou qualquer um com Mencionar @everyone no canal); @everyone/@here nunca.
async fn allowed_mentions(
    ctx: Context<'_>,
    guild_id: serenity::GuildId,
    channel_id: serenity::ChannelId,
    text: &str,
) -> Result<serenity::CreateAllowedMentions, Error> {
    let actor = gate::actor(ctx).await?;
    let roles = {
        let guild = ctx.guild().ok_or("servidor fora do cache")?;
        let everyone_perm = guild
            .channels
            .get(&channel_id)
            .is_some_and(|c| guild.user_permissions_in(c, &actor).mention_everyone());
        let mentionable: HashSet<u64> = guild
            .roles
            .values()
            .filter(|r| r.mentionable)
            .map(|r| r.id.get())
            .collect();
        allowed_roles(
            &role_mentions(text),
            &mentionable,
            everyone_perm,
            guild_id.get(),
        )
    };
    Ok(serenity::CreateAllowedMentions::new()
        .all_users(true)
        .roles(roles.into_iter().map(serenity::RoleId::new)))
}

/// Os ids de `<@&id>` no texto, na ordem, sem repetir.
pub fn role_mentions(text: &str) -> Vec<u64> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<@&") {
        rest = &rest[start + 3..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty()
            && rest[digits.len()..].starts_with('>')
            && let Ok(id) = digits.parse::<u64>()
            && !found.contains(&id)
        {
            found.push(id);
        }
    }
    found
}

/// Os cargos que podem tocar. `<@&id do servidor>` e o proprio @everyone
/// escrito como cargo: fica de fora sempre. Lista explicita (e nao
/// "all_roles"), e por isso o teto de 100 do Discord.
pub fn allowed_roles(
    mentioned: &[u64],
    mentionable: &HashSet<u64>,
    may_mention_everyone: bool,
    guild_id: u64,
) -> Vec<u64> {
    mentioned
        .iter()
        .copied()
        .filter(|&id| id != guild_id)
        .filter(|id| may_mention_everyone || mentionable.contains(id))
        .take(MAX_ALLOWED_ROLES)
        .collect()
}

/// Partes de ate 2000 unidades UTF-16 sem partir um caractere (o
/// split_for_discord do ccore). A diferenca: corta na ultima quebra de linha
/// (ou espaco) da segunda metade da parte, quando houver, para um anuncio
/// nao sair com uma palavra partida ao meio.
pub fn split_for_discord(message: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut rest = message;
    while embeds::utf16_len(rest) > DISCORD_MESSAGE {
        // O maior prefixo que cabe, em bytes.
        let mut units = 0;
        let mut end = 0;
        for (i, c) in rest.char_indices() {
            if units + c.len_utf16() > DISCORD_MESSAGE {
                break;
            }
            units += c.len_utf16();
            end = i + c.len_utf8();
        }
        let window = &rest[..end];
        let half = window.len() / 2;
        let cut = window
            .rfind('\n')
            .filter(|&i| i >= half)
            .or_else(|| window.rfind(' ').filter(|&i| i >= half))
            .map_or(end, |i| i + 1);
        parts.push(rest[..cut].to_owned());
        rest = &rest[cut..];
    }
    if !rest.is_empty() {
        parts.push(rest.to_owned());
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn divide_em_2000_sem_partir_emoji() {
        assert!(split_for_discord("").is_empty());
        assert_eq!(split_for_discord("oi"), ["oi"]);

        let plain = "a".repeat(4001);
        let parts = split_for_discord(&plain);
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            [2000, 2000, 1]
        );

        let emoji = format!("{}😀b", "a".repeat(1999));
        let parts = split_for_discord(&emoji);
        assert_eq!(parts[0], "a".repeat(1999));
        assert_eq!(parts[1], "😀b");
        assert!(
            parts
                .iter()
                .all(|p| embeds::utf16_len(p) <= DISCORD_MESSAGE)
        );
    }

    #[test]
    fn prefere_quebra_de_linha_e_espaco() {
        let text = format!("{}\n{}", "a".repeat(1500), "b".repeat(1000));
        let parts = split_for_discord(&text);
        assert_eq!(parts, [format!("{}\n", "a".repeat(1500)), "b".repeat(1000)]);

        let words = "palavra ".repeat(400);
        let parts = split_for_discord(&words);
        assert!(parts.len() == 2 && parts[0].ends_with(' '));
        assert_eq!(parts.concat(), words);

        // Quebra so na primeira metade nao conta: corta seco.
        let early = format!("x\n{}", "c".repeat(2500));
        let parts = split_for_discord(&early);
        assert_eq!(parts[0].len(), 2000);
        assert_eq!(parts.concat(), early);
    }

    #[test]
    fn acha_mencoes_de_cargo() {
        assert_eq!(
            role_mentions("oi <@&12> e <@&34>, <@&12> de novo, <@&> <@&5x> <@!7> <@&9"),
            [12, 34]
        );
    }

    #[test]
    fn cargos_liberados() {
        let mentionable: HashSet<u64> = [12].into();
        // Sem Mencionar @everyone: so o mencionavel. O id do servidor (o
        // @everyone como cargo) nunca.
        assert_eq!(
            allowed_roles(&[12, 34, 100], &mentionable, false, 100),
            [12]
        );
        assert_eq!(
            allowed_roles(&[12, 34, 100], &mentionable, true, 100),
            [12, 34]
        );

        let many: Vec<u64> = (1..=150).collect();
        assert_eq!(allowed_roles(&many, &mentionable, true, 0).len(), 100);
    }
}
