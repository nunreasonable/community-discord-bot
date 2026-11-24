//! Registro das acoes de moderacao no canal de log do servidor
//! (ModerationLog.cs). Usado pela moderacao, pelos avisos, pelo /xp e pelo
//! /say - por isso mora na base.
//!
//! Nunca falha: falhar em registrar nao pode desfazer nem mascarar uma
//! punicao que ja foi aplicada. A falha vai para o log, que o /logs le.

use poise::serenity_prelude as serenity;

use crate::{
    Error,
    embeds::{self, colors},
    store::GuildSettingsStore,
};

/// Uma linha do registro. `Entry::new(acao, moderador)` e o resto opcional.
pub struct Entry<'a> {
    pub action: &'a str,
    /// None para acao sem alvo, como o /purge sem filtro de usuario.
    pub target: Option<&'a serenity::User>,
    pub moderator: &'a serenity::User,
    pub reason: Option<&'a str>,
    pub extra: Option<&'a str>,
    /// O que aparece em "User" quando nao ha alvo.
    pub no_target_label: &'a str,
}

impl<'a> Entry<'a> {
    pub fn new(action: &'a str, moderator: &'a serenity::User) -> Self {
        Self {
            action,
            target: None,
            moderator,
            reason: None,
            extra: None,
            no_target_label: "*entire channel*",
        }
    }

    pub fn target(mut self, target: &'a serenity::User) -> Self {
        self.target = Some(target);
        self
    }

    pub fn reason(mut self, reason: &'a str) -> Self {
        self.reason = Some(reason);
        self
    }

    pub fn extra(mut self, extra: &'a str) -> Self {
        self.extra = Some(extra);
        self
    }

    pub fn no_target_label(mut self, label: &'a str) -> Self {
        self.no_target_label = label;
        self
    }
}

/// Registra no canal de log DAQUELE servidor, sem nunca falhar.
pub async fn record(
    http: impl serenity::CacheHttp,
    settings: &GuildSettingsStore,
    guild_id: serenity::GuildId,
    entry: Entry<'_>,
) {
    if let Err(cause) = write(http, settings, guild_id, &entry).await {
        tracing::error!("[modlog] falha ao registrar '{}': {cause}", entry.action);
    }
}

/// Aviso do proprio bot no canal de log (o disjuntor do vigia, por exemplo).
/// Nao e o registro de uma punicao, entao nao tem "User" nem "Moderator".
/// Vermelho: e uma condicao que exige alguem olhar.
pub async fn alert(
    http: impl serenity::CacheHttp,
    settings: &GuildSettingsStore,
    guild_id: serenity::GuildId,
    title: &str,
    description: &str,
) {
    let result: Result<(), Error> = async {
        let Some(channel) = log_channel(&http, settings, guild_id, title).await? else {
            return Ok(());
        };
        let embed = serenity::CreateEmbed::new()
            .title(format!("⚠️ {}", embeds::trim(title, 200)))
            .description(embeds::trim(description, 3500))
            .colour(colors::INDIAN_RED)
            .timestamp(serenity::Timestamp::now());
        channel
            .send_message(http.http(), serenity::CreateMessage::new().embed(embed))
            .await?;
        Ok(())
    }
    .await;

    if let Err(cause) = result {
        tracing::error!("[modlog] falha ao avisar '{title}': {cause}");
    }
}

async fn write(
    http: impl serenity::CacheHttp,
    settings: &GuildSettingsStore,
    guild_id: serenity::GuildId,
    entry: &Entry<'_>,
) -> Result<(), Error> {
    let Some(channel) = log_channel(&http, settings, guild_id, entry.action).await? else {
        return Ok(());
    };

    let user = match entry.target {
        Some(target) => format!("<@{}>\n`{}`", target.id, target.id),
        None => entry.no_target_label.to_owned(),
    };
    let reason = match entry.reason {
        Some(reason) if !reason.trim().is_empty() => embeds::safe_trim(reason, 1000),
        _ => "*no reason given*".to_owned(),
    };

    let mut embed = serenity::CreateEmbed::new()
        .title(format!("Moderation — {}", entry.action))
        .colour(colors::ORANGE)
        .timestamp(serenity::Timestamp::now())
        .field("User", user, true)
        .field(
            "Moderator",
            format!("<@{}>\n`{}`", entry.moderator.id, entry.moderator.id),
            true,
        )
        .field("Reason", reason, false);

    if let Some(target) = entry.target {
        embed = embed.thumbnail(target.face());
    }
    if let Some(extra) = entry.extra.filter(|e| !e.trim().is_empty()) {
        embed = embed.field("Details", embeds::safe_trim(extra, 1000), false);
    }

    channel
        .send_message(http.http(), serenity::CreateMessage::new().embed(embed))
        .await?;
    Ok(())
}

/// O canal configurado, se for MESMO do servidor da acao. O dono pode digitar
/// no /config um id de outro servidor, e o /ban de um ia parar no log do
/// outro. Na duvida nao publica.
async fn log_channel(
    http: &impl serenity::CacheHttp,
    settings: &GuildSettingsStore,
    guild_id: serenity::GuildId,
    what: &str,
) -> Result<Option<serenity::ChannelId>, Error> {
    let Some(id) = settings
        .for_guild(guild_id.get())
        .and_then(|s| s.moderation_log_channel_id)
        .filter(|&id| id > 0)
    else {
        return Ok(None);
    };

    let channel = serenity::ChannelId::new(id).to_channel(http).await?;
    let owner = channel.guild().map(|c| c.guild_id);
    if owner == Some(guild_id) {
        return Ok(Some(serenity::ChannelId::new(id)));
    }

    tracing::warn!(
        "[modlog] '{what}' NAO registrado: o canal de log configurado esta no servidor {}, e a acao aconteceu no {guild_id}. Configure um moderationLogChannelId do proprio servidor.",
        owner.map_or("(nenhum)".to_owned(), |g| g.to_string())
    );
    Ok(None)
}
