//! Pecas que os comandos de moderacao, as advertencias e o /config repetem:
//! responder editando a resposta deferida, o motivo do audit log, o membro do
//! proprio bot e a checagem de hierarquia contra um alvo.

use poise::serenity_prelude as serenity;

use crate::{Context, Error, audit_reason, hierarchy};

/// Defer efemero: todo comando daqui responde so para quem usou, e o defer
/// vem antes de qualquer chamada de rede para nao estourar os 3 s.
pub async fn defer(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    Ok(())
}

/// O EditResponseAsync do C#: troca o "pensando..." pelo embed. Editar, e nao
/// mandar um followup, para a resposta continuar sendo UMA mensagem.
pub async fn respond(ctx: Context<'_>, embed: serenity::CreateEmbed) -> Result<(), Error> {
    match ctx {
        poise::Context::Application(app) => {
            app.interaction
                .edit_response(
                    ctx.http(),
                    serenity::EditInteractionResponse::new().embed(embed),
                )
                .await?;
        }
        poise::Context::Prefix(_) => {
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
        }
    }
    Ok(())
}

/// Texto que vai para o Audit Log do Discord, com quem mandou fazer. A
/// limpeza e o corte moram no audit_reason, que os tickets tambem usam.
pub fn reason(ctx: Context<'_>, reason: Option<&str>) -> String {
    audit_reason::for_actor(&tag(ctx.author()), reason)
}

/// O `UsernameWithDiscriminator` do DisCatSharp: "nome#0" para conta no
/// sistema novo de nomes, "nome#1234" para conta antiga. O `tag()` do
/// serenity tira o "#0", e o texto do audit log e o moderatorTag gravado no
/// warnings.json mudariam de formato entre o bot em C# e este.
pub fn tag(user: &serenity::User) -> String {
    match user.discriminator {
        Some(d) => format!("{}#{:04}", user.name, d.get()),
        None => format!("{}#0", user.name),
    }
}

/// O servidor de um comando `guild_only`. Erro so se o poise deixar passar
/// um comando fora de servidor, o que o `guild_only` ja impede.
pub fn guild_id(ctx: Context<'_>) -> Result<serenity::GuildId, Error> {
    ctx.guild_id()
        .ok_or_else(|| "comando de servidor fora de servidor".into())
}

/// O membro do proprio bot: do cache, e da API quando o cache ainda esta
/// frio (logo depois de reconectar). None quando nem um nem outro responde -
/// e quem chama recusa, porque sem ele nao ha hierarquia para conferir.
pub async fn bot_member(
    ctx: &serenity::Context,
    guild_id: serenity::GuildId,
) -> Option<serenity::Member> {
    let me = ctx.cache.current_user().id;
    let cached = ctx
        .cache
        .guild(guild_id)
        .and_then(|g| g.members.get(&me).cloned());
    if cached.is_some() {
        return cached;
    }
    guild_id.member(ctx, me).await.ok()
}

/// O membro de quem usou o comando, como o `ctx.Member` do C#: vem pronto na
/// interacao, sem chamada de rede.
pub async fn actor(ctx: Context<'_>) -> Result<serenity::Member, Error> {
    ctx.author_member()
        .await
        .map(std::borrow::Cow::into_owned)
        .ok_or_else(|| "nao consegui ler o membro de quem usou o comando".into())
}

/// `Hierarchy.Check(ctx.Guild, ctx.Member, alvo, ctx.Guild.CurrentMember)`:
/// None quando pode agir, ou o embed de recusa.
pub async fn blocked(
    ctx: Context<'_>,
    target: &serenity::Member,
) -> Result<Option<serenity::CreateEmbed>, Error> {
    let guild_id = guild_id(ctx)?;
    let actor = actor(ctx).await?;
    let bot = bot_member(ctx.serenity_context(), guild_id).await;

    // O cache nao atravessa .await: tudo daqui para baixo e sincrono.
    let guild = ctx.guild().ok_or("servidor fora do cache")?;
    Ok(hierarchy::check(&guild, &actor, target, bot.as_ref()))
}

/// O titulo de um embed pronto. O CreateEmbed do serenity nao expoe os
/// campos, mas serializa; serve para logar o motivo de uma recusa.
pub fn embed_title(embed: &serenity::CreateEmbed) -> String {
    serde_json::to_value(embed)
        .ok()
        .and_then(|v| v.get("title").and_then(|t| t.as_str()).map(str::to_owned))
        .unwrap_or_default()
}

/// Mencao de canal, como o `channel.Mention` do C#.
pub fn channel_mention(id: impl Into<u64>) -> String {
    format!("<#{}>", id.into())
}

/// Mencao de usuario, como o `user.Mention` do C#.
pub fn user_mention(id: impl Into<u64>) -> String {
    format!("<@{}>", id.into())
}

/// Mencao de cargo, como o `role.Mention` do C#.
pub fn role_mention(id: impl Into<u64>) -> String {
    format!("<@&{}>", id.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titulo_do_embed() {
        let embed = crate::embeds::error("Role hierarchy", "x");
        assert_eq!(embed_title(&embed), "Role hierarchy");
        assert_eq!(embed_title(&serenity::CreateEmbed::new()), "");
    }

    #[test]
    fn tag_como_o_discatsharp() {
        let mut user = serenity::User::default();
        user.name = "fulano".into();
        assert_eq!(tag(&user), "fulano#0");
        user.discriminator = std::num::NonZeroU16::new(42);
        assert_eq!(tag(&user), "fulano#0042");
    }

    #[test]
    fn mencoes() {
        assert_eq!(channel_mention(5u64), "<#5>");
        assert_eq!(user_mention(serenity::UserId::new(7)), "<@7>");
        assert_eq!(role_mention(9u64), "<@&9>");
    }
}
