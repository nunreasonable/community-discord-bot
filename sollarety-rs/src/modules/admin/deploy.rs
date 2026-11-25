//! /deployment e /dmdeployment role|user: a convocacao para uma partida, com
//! o codigo de acesso e o link de entrada rapida (o deployment do ccore).
//!
//! Portado de (ccore-rs/):
//! - src/deployment.rs (build, sanitize, escape, quick_launch, channel) -> aqui
//! - src/commands/deployment.rs (/deployment) -> `deployment`
//! - src/commands/massdm.rs (dmdeployment, deployment_dm) -> `dm_deployment`,
//!   rodando na maquina de envio do dm.rs
//!
//! Diferencas: nada de titulo, emoji, canal de voz ou link do regimento como
//! padrao (sem config, o titulo e "Deployment" e a linha de voz some); o aviso
//! de imagem invalida vai so para quem pediu, e nao para o post publico; link
//! que nao seja http(s) e descartado, porque o Discord recusaria a mensagem
//! inteira por causa de um botao.

use std::collections::HashSet;

use poise::serenity_prelude as serenity;

use super::{
    dm::{self, Payload, Reply},
    gate,
};
use crate::{
    Context, Error,
    config::{self, DeploymentSettings},
    embeds::{self, colors},
    modlog,
};

const FALLBACK_TITLE: &str = "Deployment";
/// Teto de um campo de embed.
const FIELD: usize = 1024;
/// O Discord aceita no maximo 100 ids em allowed_mentions.roles.
const MAX_PINGS: usize = 100;
/// Cargos listados na recusa de permissao; o resto vira "...".
const MAX_LISTED_ROLES: usize = 15;

// ---------------------------------------------------------------------------
// Pecas puras
// ---------------------------------------------------------------------------

fn non_blank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

/// So http(s): link de botao e link mascarado com outro esquema fazem o
/// Discord recusar a mensagem inteira.
fn web_url(value: &str) -> Option<String> {
    url::Url::parse(value.trim())
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https"))
        .map(String::from)
}

/// Tira o que quebraria o markdown do embed: a crase fecha o code span, e
/// colchete/parentese forjariam um segundo link no rotulo, justo na mensagem
/// que todo mundo clica. Quebra de linha e controle tambem saem.
pub fn sanitize(code: &str) -> String {
    code.chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(
                    c,
                    '`' | '[' | ']' | '(' | ')' | '*' | '_' | '~' | '|' | '\\'
                )
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Igual ao Uri.EscapeDataString: so as letras, digitos e -_.~ passam.
pub fn escape(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// O link do jogo do post (botao quando nao ha entrada rapida).
pub fn game_link(settings: &DeploymentSettings) -> Option<String> {
    non_blank(settings.game_link.as_deref()).and_then(web_url)
}

/// O link do texto da DM: dmGameLink, senao gameLink.
pub fn dm_link(settings: &DeploymentSettings) -> Option<String> {
    non_blank(settings.dm_game_link.as_deref())
        .and_then(web_url)
        .or_else(|| game_link(settings))
}

/// Entrada rapida: o modelo proprio (CODE_HERE ou {CODE}), senao o do
/// placeId, senao o link do jogo. O codigo vai escapado; o que nao virar
/// link http(s) valido some.
pub fn quick_launch(settings: &DeploymentSettings, code: &str) -> Option<String> {
    let template = non_blank(settings.quick_launch_link.as_deref())
        .map(str::to_owned)
        .or_else(|| {
            non_blank(settings.place_id.as_deref()).map(|place| {
                format!(
                    "https://www.roblox.com/games/start?launchData=CODE_HERE&placeId={}",
                    escape(place)
                )
            })
        })
        .or_else(|| game_link(settings))?;
    let code = escape(code);
    web_url(
        &template
            .replace("CODE_HERE", &code)
            .replace("{CODE}", &code),
    )
}

/// Os cargos padrao que existem NESTE servidor, sem repetir, sem o
/// @everyone (cujo id e o do servidor) e no teto do Discord.
pub fn ping_roles(configured: &[u64], existing: &HashSet<u64>, guild_id: u64) -> Vec<u64> {
    let mut out: Vec<u64> = Vec::new();
    for &id in configured {
        if id != guild_id && existing.contains(&id) && !out.contains(&id) {
            out.push(id);
        }
    }
    out.truncate(MAX_PINGS);
    out
}

/// Cargos que aparecem no post mas nao notificam: nao mencionaveis, com o
/// bot sem Mencionar @everyone no canal.
pub fn silent_roles(
    pinged: &[u64],
    mentionable: &HashSet<u64>,
    bot_mentions_all: bool,
) -> Vec<u64> {
    if bot_mentions_all {
        return Vec::new();
    }
    pinged
        .iter()
        .copied()
        .filter(|id| !mentionable.contains(id))
        .collect()
}

/// `:nome:` vira o emoji do servidor com esse nome (sem diferenciar
/// maiusculas), a versao generica do :Resenha: do ccore. Mencao ou emoji ja
/// pronto (`<...>`) passa intacto; nome sem emoji fica como esta.
pub fn resolve_emojis(text: &str, emojis: &[(String, String)]) -> String {
    let find = |name: &str| {
        emojis
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, rendered)| rendered.as_str())
    };
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(['<', ':']) {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        if rest.starts_with('<') {
            let end = rest.find('>').map_or(rest.len(), |e| e + 1);
            out.push_str(&rest[..end]);
            rest = &rest[end..];
            continue;
        }
        let after = &rest[1..];
        let name_len = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        let name = &after[..name_len];
        match find(name) {
            Some(rendered) if name.len() >= 2 && after[name_len..].starts_with(':') => {
                out.push_str(rendered);
                rest = &after[name_len + 1..];
            }
            _ => {
                out.push(':');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A linha do canal de voz, ou None quando nao ha nem nome nem link.
pub fn voice_line(settings: &DeploymentSettings) -> Option<String> {
    let label = non_blank(settings.voice_channel.as_deref());
    let link = non_blank(settings.voice_chat_link.as_deref()).and_then(web_url);
    match (label, link) {
        (Some(label), Some(link)) => Some(format!("[{label}]({link})")),
        (Some(label), None) => Some(label.to_owned()),
        (None, Some(link)) => Some(format!("[Join voice]({link})")),
        (None, None) => None,
    }
}

/// A descricao do embed do post. `safe` ja passou pelo `sanitize`.
pub fn description(roles: &[u64], voice: Option<&str>, safe: &str, launch: Option<&str>) -> String {
    let mut lines = Vec::new();
    if !roles.is_empty() {
        lines.push(mentions(roles));
        lines.push(String::new());
    }
    if let Some(voice) = voice {
        lines.push(format!("**Voice Channel:** {voice}"));
    }
    lines.push(format!("**Code:** `{safe}`"));
    lines.push(String::new());
    lines.push(match launch {
        Some(link) => format!("**Quick Launch Link:** [Join & Enter {safe}]({link})"),
        None => format!("**Quick Launch Link:** Join & Enter {safe}"),
    });
    lines.join("\n")
}

fn mentions(roles: &[u64]) -> String {
    roles
        .iter()
        .map(|id| format!("<@&{id}>"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Quem pode usar: o dono e Administrator sempre; com allowedRoleIds, quem
/// tem um deles; sem a lista, quem tem Gerenciar Servidor.
pub fn may_deploy(
    allowed: &[u64],
    member_roles: &[u64],
    is_owner: bool,
    permissions: serenity::Permissions,
) -> bool {
    if is_owner || permissions.administrator() {
        return true;
    }
    if allowed.is_empty() {
        return permissions.manage_guild();
    }
    member_roles.iter().any(|r| allowed.contains(r))
}

/// Quem ve os comandos no menu. Com cargos proprios, todo mundo (o Discord
/// nao filtra por cargo aqui; a conta e feita na execucao); sem, so quem tem
/// Gerenciar Servidor. Lido na subida: mudar a lista pede reinicio para o
/// menu acompanhar.
pub fn menu_permissions(allowed: &[u64]) -> serenity::Permissions {
    if allowed.is_empty() {
        serenity::Permissions::MANAGE_GUILD
    } else {
        serenity::Permissions::empty()
    }
}

/// O que o post precisa saber do servidor, copiado do cache.
#[derive(Clone, Debug, Default)]
pub struct GuildFacts {
    pub guild_id: u64,
    pub roles: HashSet<u64>,
    pub mentionable: HashSet<u64>,
    /// (nome, `<:nome:id>`)
    pub emojis: Vec<(String, String)>,
}

pub struct Post {
    pub message: serenity::CreateMessage,
    pub title: String,
    pub roles: Vec<u64>,
    /// A imagem configurada nao e um link http(s) valido e ficou de fora.
    pub bad_image: bool,
}

/// Monta o post. `safe` e o codigo ja limpo; nao vazio.
pub fn build(settings: &DeploymentSettings, safe: &str, facts: &GuildFacts) -> Post {
    let title = embeds::trim(
        &resolve_emojis(
            non_blank(settings.title.as_deref()).unwrap_or(FALLBACK_TITLE),
            &facts.emojis,
        ),
        256,
    );
    let roles = ping_roles(&settings.default_roles, &facts.roles, facts.guild_id);
    let launch = quick_launch(settings, safe);
    let voice = voice_line(settings);

    let mut embed = serenity::CreateEmbed::new()
        .title(&title)
        .colour(colors::BLURPLE)
        .description(description(
            &roles,
            voice.as_deref(),
            safe,
            launch.as_deref(),
        ));
    let mut bad_image = false;
    if let Some(image) = non_blank(settings.image_url.as_deref()) {
        match web_url(image) {
            Some(image) => embed = embed.image(image),
            None => bad_image = true,
        }
    }

    // As mencoes vao no conteudo: dentro do embed elas aparecem mas nao
    // notificam ninguem. allowed_mentions com a lista EXATA: nada de
    // usuario, @everyone ou @here, mesmo que o titulo traga um.
    let mut message = serenity::CreateMessage::new()
        .embed(embed)
        .allowed_mentions(
            serenity::CreateAllowedMentions::new()
                .roles(roles.iter().map(|&id| serenity::RoleId::new(id))),
        );
    if !roles.is_empty() {
        message = message.content(mentions(&roles));
    }
    if let Some(link) = launch.or_else(|| game_link(settings)) {
        message = message.components(vec![serenity::CreateActionRow::Buttons(vec![
            serenity::CreateButton::new_link(link).label("Quick Launch"),
        ])]);
    }

    Post {
        message,
        title,
        roles,
        bad_image,
    }
}

/// O embed da DM (o deployment_dm do ccore), com o rodape de sempre das DMs
/// do modulo dizendo de que servidor veio. Campos cortados em 1024: texto
/// maior faz o Discord recusar TODAS as DMs do envio.
pub fn dm_embed(
    target: &str,
    server: &str,
    icon: Option<String>,
    safe: &str,
    message: Option<&str>,
) -> serenity::CreateEmbed {
    let body = match non_blank(message) {
        Some(text) => embeds::safe_trim(text, FIELD),
        None => "(no extra message)".to_owned(),
    };
    let mut embed = serenity::CreateEmbed::new()
        .title(embeds::trim(
            &format!("Message for {}", embeds::plain(target, 100)),
            256,
        ))
        .colour(colors::BLURPLE)
        .field("Code", embeds::trim(&format!("`{safe}`"), FIELD), true)
        .field("Message", body, false)
        .footer(serenity::CreateEmbedFooter::new(embeds::trim(
            &format!("Sent by the staff of {server} through this bot. Replies here aren't read."),
            2048,
        )))
        .timestamp(serenity::Timestamp::now());
    if let Some(icon) = icon {
        embed = embed.thumbnail(icon);
    }
    embed
}

/// A DM inteira: o link no texto (o Discord mostra o preview do jogo) e o
/// embed. Sem mencao nenhuma.
pub fn dm_message(link: Option<String>, embed: serenity::CreateEmbed) -> serenity::CreateMessage {
    let mut message = serenity::CreateMessage::new()
        .embed(embed)
        .allowed_mentions(serenity::CreateAllowedMentions::new());
    if let Some(link) = link {
        message = message.content(link);
    }
    message
}

// ---------------------------------------------------------------------------
// Discord
// ---------------------------------------------------------------------------

/// None quando pode; senao a recusa pronta. A permissao conta no SERVIDOR.
async fn authorize(
    ctx: Context<'_>,
    settings: &DeploymentSettings,
) -> Result<Option<serenity::CreateEmbed>, Error> {
    let actor = gate::actor(ctx).await?;
    let allowed = {
        let guild = ctx.guild().ok_or("servidor fora do cache")?;
        let roles: Vec<u64> = actor.roles.iter().map(|r| r.get()).collect();
        may_deploy(
            &settings.allowed_role_ids,
            &roles,
            guild.owner_id == actor.user.id,
            guild.member_permissions(&actor),
        )
    };
    if allowed {
        return Ok(None);
    }
    let why = if settings.allowed_role_ids.is_empty() {
        "You need the Manage Server permission to use this.".to_owned()
    } else {
        let mut listed: Vec<String> = settings
            .allowed_role_ids
            .iter()
            .take(MAX_LISTED_ROLES)
            .map(|id| format!("<@&{id}>"))
            .collect();
        if settings.allowed_role_ids.len() > MAX_LISTED_ROLES {
            listed.push("...".to_owned());
        }
        format!(
            "You need one of these roles to use this: {}.",
            listed.join(", ")
        )
    };
    Ok(Some(embeds::error("Not allowed", why)))
}

/// Ensure + defer + permissao + codigo: o comeco dos tres comandos. None
/// quando ja respondeu com a recusa.
async fn begin(ctx: Context<'_>, code: &str) -> Result<Option<Begun>, Error> {
    if !gate::ensure(ctx).await? {
        return Ok(None);
    }
    ctx.defer_ephemeral().await?;
    let guild_id = ctx
        .guild_id()
        .ok_or("comando de servidor fora de servidor")?;
    let settings = config::current().admin.deployment.clone();
    if let Some(denied) = authorize(ctx, &settings).await? {
        gate::respond(ctx, denied).await?;
        return Ok(None);
    }
    let safe = sanitize(code);
    if safe.is_empty() {
        gate::respond(
            ctx,
            embeds::error(
                "Invalid code",
                "The code is empty once formatting characters are removed.",
            ),
        )
        .await?;
        return Ok(None);
    }
    Ok(Some(Begun {
        guild_id,
        settings,
        safe,
    }))
}

struct Begun {
    guild_id: serenity::GuildId,
    settings: DeploymentSettings,
    safe: String,
}

/// O canal de destino e o que o bot pode fazer nele. Err e a recusa pronta.
struct Target {
    channel_id: serenity::ChannelId,
    mentions_all: bool,
}

async fn target(
    ctx: Context<'_>,
    guild_id: serenity::GuildId,
    settings: &DeploymentSettings,
) -> Result<Result<Target, serenity::CreateEmbed>, Error> {
    let Some(id) = settings.channel_id.filter(|&id| id != 0) else {
        return Ok(Err(embeds::error(
            "No deployment channel",
            "Set `admin.deployment.channelId` in the bot's config first.",
        )));
    };
    let bot = gate::bot_member(ctx.serenity_context(), guild_id).await;

    // O cache nao atravessa .await: daqui para baixo e sincrono.
    let guild = ctx.guild().ok_or("servidor fora do cache")?;
    let channel_id = serenity::ChannelId::new(id);
    let Some(channel) = guild.channels.get(&channel_id).filter(|c| {
        c.guild_id == guild_id
            && matches!(
                c.kind,
                serenity::ChannelType::Text | serenity::ChannelType::News
            )
    }) else {
        return Ok(Err(embeds::error(
            "Invalid deployment channel",
            format!(
                "The configured deployment channel (`{id}`) isn't a text channel of this server."
            ),
        )));
    };
    let Some(bot) = bot else {
        return Ok(Err(embeds::error(
            "I can't post there",
            "I couldn't read my own member in this server.",
        )));
    };
    let permissions = guild.user_permissions_in(channel, &bot);
    let needed = serenity::Permissions::VIEW_CHANNEL
        | serenity::Permissions::SEND_MESSAGES
        | serenity::Permissions::EMBED_LINKS;
    let missing = needed - permissions;
    if !missing.is_empty() {
        return Ok(Err(embeds::error(
            "I can't post there",
            format!(
                "I'm missing {} in <#{id}>. Check my role and the channel's permissions.",
                gate::permission_names(missing)
            ),
        )));
    }
    Ok(Ok(Target {
        channel_id,
        mentions_all: permissions.mention_everyone(),
    }))
}

fn guild_facts(ctx: Context<'_>, guild_id: serenity::GuildId) -> Result<GuildFacts, Error> {
    let guild = ctx.guild().ok_or("servidor fora do cache")?;
    Ok(GuildFacts {
        guild_id: guild_id.get(),
        roles: guild.roles.keys().map(|id| id.get()).collect(),
        mentionable: guild
            .roles
            .values()
            .filter(|r| r.mentionable)
            .map(|r| r.id.get())
            .collect(),
        emojis: guild
            .emojis
            .values()
            .map(|e| (e.name.clone(), e.to_string()))
            .collect(),
    })
}

/// Posts a deployment call with the access code in the configured channel
#[poise::command(slash_command, guild_only)]
pub async fn deployment(
    ctx: Context<'_>,
    // O codigo vai em code span, rotulo de link e campo de embed: curto.
    #[description = "Game access code"]
    #[max_length = 100]
    code: String,
) -> Result<(), Error> {
    let Some(Begun {
        guild_id,
        settings,
        safe,
    }) = begin(ctx, &code).await?
    else {
        return Ok(());
    };
    let target = match target(ctx, guild_id, &settings).await? {
        Ok(target) => target,
        Err(refusal) => return gate::respond(ctx, refusal).await,
    };
    let facts = guild_facts(ctx, guild_id)?;
    let post = build(&settings, &safe, &facts);
    let silent = silent_roles(&post.roles, &facts.mentionable, target.mentions_all);

    let sent = match target
        .channel_id
        .send_message(ctx.http(), post.message)
        .await
    {
        Ok(sent) => sent,
        Err(cause) => {
            tracing::warn!(
                "[admin] /deployment de {} para {} falhou: {cause}",
                ctx.author().id,
                target.channel_id
            );
            return gate::respond(
                ctx,
                embeds::error(
                    "Failed to post",
                    format!(
                        "Nothing was posted.\n\nDetails: {}",
                        embeds::trim(&cause.to_string(), 400)
                    ),
                ),
            )
            .await;
        }
    };

    let link = sent.link();
    let channel_id = target.channel_id;
    let mut text = format!(
        "Posted in <#{channel_id}>: **{}**\nCode: `{safe}` · Roles pinged: {}\n[Jump to the message]({link})",
        embeds::plain(&post.title, 200),
        post.roles.len()
    );
    if post.bad_image {
        text.push_str(
            "\n\n⚠️ `admin.deployment.imageUrl` isn't a valid http(s) link, so the post went out without the image.",
        );
    }
    if !silent.is_empty() {
        text.push_str(&format!(
            "\n\n⚠️ {} aren't mentionable and I don't have Mention @everyone there, so they were shown but not notified.",
            mentions(&silent)
        ));
    }
    gate::respond(ctx, embeds::ok("Deployment posted", text)).await?;

    let extra = format!(
        "Channel: <#{channel_id}> · {} role(s) pinged · {link}\nCode: `{safe}`",
        post.roles.len()
    );
    modlog::record(
        ctx,
        &ctx.data().guild_settings,
        guild_id,
        modlog::Entry::new("Deployment posted", ctx.author())
            .no_target_label("—")
            .extra(&extra),
    )
    .await;
    Ok(())
}

/// Sends the deployment code by DM
#[poise::command(
    slash_command,
    rename = "dmdeployment",
    guild_only,
    subcommand_required,
    subcommands("dm_role", "dm_user")
)]
pub async fn dm_deployment(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// DMs the deployment code to every member with a role (not bots)
#[poise::command(
    slash_command,
    rename = "role",
    guild_only,
    required_bot_permissions = "VIEW_CHANNEL | SEND_MESSAGES | EMBED_LINKS"
)]
async fn dm_role(
    ctx: Context<'_>,
    #[description = "Everyone with this role gets the DM"] role: serenity::Role,
    #[description = "Game access code"]
    #[max_length = 100]
    code: String,
    #[description = "Extra message (instructions, time...)"]
    #[max_length = 1024]
    message: Option<String>,
) -> Result<(), Error> {
    let Some(begun) = begin(ctx, &code).await? else {
        return Ok(());
    };
    if let Some(refusal) = dm::role_refusal(ctx, begun.guild_id, role.id) {
        return gate::respond(ctx, refusal).await;
    }
    let payload = payload(ctx, &begun, &role.name, message.as_deref());
    dm::start_role_job(ctx, Reply::Command, begun.guild_id, role.id, payload).await
}

/// DMs the deployment code to one member of this server
#[poise::command(slash_command, rename = "user", guild_only)]
async fn dm_user(
    ctx: Context<'_>,
    #[description = "Who gets the DM (must be in this server)"] user: serenity::User,
    #[description = "Game access code"]
    #[max_length = 100]
    code: String,
    #[description = "Extra message (instructions, time...)"]
    #[max_length = 1024]
    message: Option<String>,
) -> Result<(), Error> {
    let Some(begun) = begin(ctx, &code).await? else {
        return Ok(());
    };
    if let Some(refusal) = dm::user_refusal(ctx, begun.guild_id, &user).await? {
        return gate::respond(ctx, refusal).await;
    }
    let payload = payload(ctx, &begun, user.display_name(), message.as_deref());
    dm::send_single(ctx, Reply::Command, begun.guild_id, &user, payload).await
}

fn payload(ctx: Context<'_>, begun: &Begun, target: &str, message: Option<&str>) -> Payload {
    let (server, icon) = dm::server_info(ctx.serenity_context(), begun.guild_id);
    let embed = dm_embed(target, &server, icon, &begun.safe, message);
    let body = non_blank(message).map_or("(none)".to_owned(), |m| embeds::trim(m, 600));
    Payload {
        message: dm_message(dm_link(&begun.settings), embed),
        labels: dm::DEPLOYMENT,
        details: format!("Code: `{}`\n\nMessage:\n{body}", begun.safe),
    }
}

/// Os dois comandos, com a visibilidade do menu tirada do config da subida.
pub fn commands() -> Vec<poise::Command<crate::Data, Error>> {
    let menu = menu_permissions(&config::current().admin.deployment.allowed_role_ids);
    [deployment(), dm_deployment()]
        .into_iter()
        .map(|mut command| {
            command.default_member_permissions = menu;
            command
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use poise::serenity_prelude::Permissions as P;

    fn settings(json: &str) -> DeploymentSettings {
        let full = format!(r#"{{ "admin": {{ "deployment": {json} }} }}"#);
        config::Config::parse(&full).unwrap().admin.deployment
    }

    #[test]
    fn codigo_nao_forja_link_nem_fecha_crase() {
        assert_eq!(sanitize("ab`c](https://x)"), "abchttps://x");
        assert_eq!(sanitize(" a*b_c~d|e\\f\n "), "abcdef");
        assert_eq!(sanitize("``[]"), "");
        assert_eq!(sanitize("ÁB-12"), "ÁB-12");
    }

    #[test]
    fn escape_como_o_uri_escape_data_string() {
        assert_eq!(escape("a b&c"), "a%20b%26c");
        assert_eq!(escape("AZaz09-_.~"), "AZaz09-_.~");
        assert_eq!(escape("é/?"), "%C3%A9%2F%3F");
    }

    #[test]
    fn link_rapido_usa_o_place_id_com_codigo_escapado() {
        let s = settings(r#"{ "placeId": 123 }"#);
        assert_eq!(
            quick_launch(&s, "a b&c").unwrap(),
            "https://www.roblox.com/games/start?launchData=a%20b%26c&placeId=123"
        );
    }

    #[test]
    fn modelo_proprio_tem_prioridade_e_aceita_os_dois_marcadores() {
        let s = settings(
            r#"{ "quickLaunchLink": "https://x.test/{CODE}?again=CODE_HERE", "placeId": "9" }"#,
        );
        assert_eq!(
            quick_launch(&s, "AB").unwrap(),
            "https://x.test/AB?again=AB"
        );
    }

    #[test]
    fn link_invalido_ou_sem_http_some() {
        assert!(
            quick_launch(
                &settings(r#"{ "quickLaunchLink": "nao e url CODE_HERE" }"#),
                "AB"
            )
            .is_none()
        );
        assert!(
            quick_launch(
                &settings(r#"{ "quickLaunchLink": "roblox://x?c={CODE}" }"#),
                "AB"
            )
            .is_none()
        );
        // Sem modelo nem placeId: o link do jogo; sem ele, nada.
        assert!(quick_launch(&settings("{}"), "AB").is_none());
        assert_eq!(
            quick_launch(&settings(r#"{ "gameLink": "https://g.test/1" }"#), "AB").unwrap(),
            "https://g.test/1"
        );
        assert!(game_link(&settings(r#"{ "gameLink": "javascript:alert(1)" }"#)).is_none());
    }

    #[test]
    fn link_da_dm_prefere_o_proprio() {
        let s = settings(r#"{ "gameLink": "https://g.test/1", "dmGameLink": "https://d.test/2" }"#);
        assert_eq!(dm_link(&s).unwrap(), "https://d.test/2");
        let s = settings(r#"{ "gameLink": "https://g.test/1", "dmGameLink": "lixo" }"#);
        assert_eq!(dm_link(&s).unwrap(), "https://g.test/1");
        assert!(dm_link(&settings("{}")).is_none());
    }

    #[test]
    fn so_pinga_cargo_que_existe_sem_repetir_e_sem_everyone() {
        let existing: HashSet<u64> = [1, 2, 100].into();
        assert_eq!(ping_roles(&[1, 3, 2, 1, 100], &existing, 100), [1, 2]);
        assert!(ping_roles(&[], &existing, 100).is_empty());
        let many: Vec<u64> = (1..=150).collect();
        let all: HashSet<u64> = many.iter().copied().collect();
        assert_eq!(ping_roles(&many, &all, 0).len(), MAX_PINGS);
    }

    #[test]
    fn cargos_que_nao_notificam() {
        let mentionable: HashSet<u64> = [1].into();
        assert_eq!(silent_roles(&[1, 2], &mentionable, false), [2]);
        assert!(silent_roles(&[1, 2], &mentionable, true).is_empty());
    }

    #[test]
    fn emoji_do_servidor_pelo_nome() {
        let emojis = vec![("Resenha".to_owned(), "<:Resenha:42>".to_owned())];
        assert_eq!(resolve_emojis(":resenha: GO", &emojis), "<:Resenha:42> GO");
        assert_eq!(
            resolve_emojis("<:Resenha:42> :x: 10:30", &emojis),
            "<:Resenha:42> :x: 10:30"
        );
        assert_eq!(
            resolve_emojis(":Resenha:Resenha:", &emojis),
            "<:Resenha:42>Resenha:"
        );
        assert_eq!(resolve_emojis("sem nada", &[]), "sem nada");
        assert_eq!(resolve_emojis("a:", &emojis), "a:");
    }

    #[test]
    fn linha_de_voz_some_sem_config() {
        assert!(voice_line(&settings("{}")).is_none());
        assert_eq!(
            voice_line(&settings(
                r#"{ "voiceChannel": "VC", "voiceChatLink": "https://v.test/" }"#
            ))
            .unwrap(),
            "[VC](https://v.test/)"
        );
        assert_eq!(
            voice_line(&settings(
                r#"{ "voiceChannel": "VC", "voiceChatLink": "x" }"#
            ))
            .unwrap(),
            "VC"
        );
        assert_eq!(
            voice_line(&settings(r#"{ "voiceChatLink": "https://v.test/" }"#)).unwrap(),
            "[Join voice](https://v.test/)"
        );
    }

    #[test]
    fn descricao_igual_a_do_ccore() {
        assert_eq!(
            description(&[1, 2], Some("VC"), "AB", Some("https://l.test/AB")),
            "<@&1> <@&2>\n\n**Voice Channel:** VC\n**Code:** `AB`\n\n**Quick Launch Link:** [Join & Enter AB](https://l.test/AB)"
        );
        assert_eq!(
            description(&[], None, "AB", None),
            "**Code:** `AB`\n\n**Quick Launch Link:** Join & Enter AB"
        );
    }

    #[test]
    fn post_pinga_so_os_cargos_e_marca_imagem_ruim() {
        let s = settings(
            r#"{ "defaultRoles": [1, 9], "title": ":Sword: Go", "imageUrl": "nao e link" }"#,
        );
        let facts = GuildFacts {
            guild_id: 100,
            roles: [1].into(),
            mentionable: [1].into(),
            emojis: vec![("sword".to_owned(), "<:sword:5>".to_owned())],
        };
        let post = build(&s, "AB", &facts);
        assert_eq!(post.roles, [1]);
        assert_eq!(post.title, "<:sword:5> Go");
        assert!(post.bad_image);

        let json = serde_json::to_value(&post.message).unwrap();
        assert_eq!(json["content"], "<@&1>");
        assert_eq!(json["allowed_mentions"]["parse"], serde_json::json!([]));
        assert_eq!(json["allowed_mentions"]["roles"], serde_json::json!(["1"]));
        assert!(json["embeds"][0].get("image").is_none());
        // Sem link nenhum, sem botao.
        assert!(
            json.get("components")
                .is_none_or(|c| c.as_array().is_none_or(Vec::is_empty))
        );

        let plain = build(&settings("{}"), "AB", &GuildFacts::default());
        assert_eq!(plain.title, FALLBACK_TITLE);
        assert!(!plain.bad_image);
        let json = serde_json::to_value(&plain.message).unwrap();
        assert!(json.get("content").is_none_or(serde_json::Value::is_null));
    }

    #[test]
    fn post_com_imagem_e_botao() {
        let s = settings(r#"{ "imageUrl": "https://i.test/a.png", "placeId": 7 }"#);
        let post = build(&s, "AB", &GuildFacts::default());
        let json = serde_json::to_value(&post.message).unwrap();
        assert_eq!(json["embeds"][0]["image"]["url"], "https://i.test/a.png");
        assert_eq!(
            json["components"][0]["components"][0]["url"],
            "https://www.roblox.com/games/start?launchData=AB&placeId=7"
        );
    }

    #[test]
    fn quem_pode() {
        let roles = [5, 6];
        // Sem lista: Gerenciar Servidor.
        assert!(may_deploy(&[], &roles, false, P::MANAGE_GUILD));
        assert!(!may_deploy(&[], &roles, false, P::MANAGE_MESSAGES));
        // Com lista: o cargo, e Gerenciar Servidor sozinho nao basta.
        assert!(may_deploy(&[6], &roles, false, P::empty()));
        assert!(!may_deploy(&[7], &roles, false, P::MANAGE_GUILD));
        // Dono e Administrator sempre.
        assert!(may_deploy(&[7], &roles, true, P::empty()));
        assert!(may_deploy(&[7], &roles, false, P::ADMINISTRATOR));

        assert_eq!(menu_permissions(&[]), P::MANAGE_GUILD);
        assert!(menu_permissions(&[1]).is_empty());
    }

    #[test]
    fn dm_corta_os_campos_em_1024() {
        let long = "[".repeat(900);
        let embed = dm_embed("Some_Role", "Server", None, "AB", Some(&long));
        let json = serde_json::to_value(&embed).unwrap();
        assert_eq!(json["title"], "Message for Some\\_Role");
        assert_eq!(json["fields"][0]["value"], "`AB`");
        let body = json["fields"][1]["value"].as_str().unwrap();
        assert!(embeds::utf16_len(body) <= FIELD);
        let empty = serde_json::to_value(dm_embed("x", "S", None, "AB", Some("  "))).unwrap();
        assert_eq!(empty["fields"][1]["value"], "(no extra message)");

        let message =
            serde_json::to_value(dm_message(Some("https://g.test/".into()), embed)).unwrap();
        assert_eq!(message["content"], "https://g.test/");
        assert_eq!(message["allowed_mentions"]["parse"], serde_json::json!([]));
    }
}
