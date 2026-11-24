//! Checagem de hierarquia antes de qualquer punicao ou cargo distribuido
//! (Hierarchy.cs e VerificationService.CheckAssignableRole).
//!
//! As permissoes do Discord ja barram quem nao e moderador; o que elas NAO
//! fazem e impedir um moderador de agir sobre alguem do mesmo nivel ou acima.
//! Tudo aqui falha FECHADO: sem saber onde alguem esta, a resposta e recusar.
//!
//! As funcoes recebem o `Guild` do cache (`ctx.guild()` / `cache.guild(id)`)
//! e os membros ja buscados. O cache nao pode atravessar um `.await`: copie o
//! que precisar antes.

use poise::serenity_prelude as serenity;

use crate::{Error, embeds};

/// Posicao do cargo mais alto. Membro sem cargo fica em -1, abaixo do
/// @everyone (posicao 0). Cargo fora do cache nao conta.
pub fn top_role(guild: &serenity::Guild, member: &serenity::Member) -> i32 {
    member
        .roles
        .iter()
        .filter_map(|id| guild.roles.get(id))
        .map(|role| i32::from(role.position))
        .max()
        .unwrap_or(-1)
}

/// None quando a acao e permitida, ou o embed de recusa. `bot` e o membro do
/// proprio bot; sem ele nao da para conferir, e a resposta e recusar.
pub fn check(
    guild: &serenity::Guild,
    actor: &serenity::Member,
    target: &serenity::Member,
    bot: Option<&serenity::Member>,
) -> Option<serenity::CreateEmbed> {
    let bot = bot.map(|b| (b.user.id.get(), top_role(guild, b)));
    check_positions(
        guild.owner_id.get(),
        (actor.user.id.get(), top_role(guild, actor)),
        (target.user.id.get(), top_role(guild, target)),
        bot,
    )
}

/// O nucleo puro do `check`: (id, posicao do cargo mais alto) de cada um.
pub fn check_positions(
    owner_id: u64,
    actor: (u64, i32),
    target: (u64, i32),
    bot: Option<(u64, i32)>,
) -> Option<serenity::CreateEmbed> {
    let Some(bot) = bot else {
        return Some(embeds::error(
            "Role hierarchy unavailable",
            "I couldn't read my own role in this server right now, so I can't check the role hierarchy. Try again in a few seconds.",
        ));
    };

    if target.0 == actor.0 {
        return Some(embeds::error(
            "Invalid target",
            "You can't use this on yourself.",
        ));
    }
    if target.0 == bot.0 {
        return Some(embeds::error(
            "Invalid target",
            "I can't use this on myself.",
        ));
    }
    if target.0 == owner_id {
        return Some(embeds::error(
            "Invalid target",
            "The server owner can't be moderated.",
        ));
    }

    let mention = format!("<@{}>", target.0);
    // O dono passa por cima da comparacao de cargo.
    if actor.0 != owner_id && target.1 >= actor.1 {
        return Some(embeds::error(
            "Role hierarchy",
            format!(
                "{mention} has a role equal to or higher than yours, so you can't moderate them."
            ),
        ));
    }
    if target.1 >= bot.1 {
        return Some(embeds::error(
            "Role hierarchy",
            format!(
                "{mention} is above my highest role. Move my role higher in the server's role list."
            ),
        ));
    }
    None
}

/// `actor` esta ESTRITAMENTE acima de `other`? Para o /delwarn: mexer no
/// registro que um moderador escreveu exige estar acima dele.
pub fn outranks(
    guild: &serenity::Guild,
    actor: &serenity::Member,
    other: &serenity::Member,
) -> bool {
    outranks_positions(
        guild.owner_id.get(),
        (actor.user.id.get(), top_role(guild, actor)),
        (other.user.id.get(), top_role(guild, other)),
    )
}

pub fn outranks_positions(owner_id: u64, actor: (u64, i32), other: (u64, i32)) -> bool {
    if actor.0 == owner_id {
        return true;
    }
    if other.0 == owner_id {
        return false;
    }
    actor.1 > other.1
}

/// Recusa um cargo que o bot nao deveria distribuir, ou None se ele serve.
/// Usado pelo /config e pelo /bind. A checagem do ATOR impede escalada: sem
/// ela, quem tem Gerenciar Servidor apontaria o cargo de verificado para o
/// de admin e se verificaria.
pub fn check_assignable_role(
    guild: &serenity::Guild,
    actor: &serenity::Member,
    role: &serenity::Role,
    bot: Option<&serenity::Member>,
) -> Option<serenity::CreateEmbed> {
    // O id do @everyone e o id do servidor.
    if role.id.get() == guild.id.get() {
        return Some(embeds::error(
            "Invalid role",
            "Everyone already has @everyone.",
        ));
    }
    if role.managed {
        return Some(embeds::error(
            "Invalid role",
            format!(
                "<@&{}> is managed by an integration (a bot or boosts), so no one can assign it manually.",
                role.id
            ),
        ));
    }
    let Some(bot) = bot else {
        return Some(embeds::error(
            "Role hierarchy unavailable",
            "I couldn't read my own role right now. Try again in a few seconds.",
        ));
    };

    let position = i32::from(role.position);
    if position >= top_role(guild, bot) {
        return Some(embeds::error(
            "Role above mine",
            format!(
                "<@&{}> is at or above my highest role, so I wouldn't be able to assign it. Move my role higher in the server's role list.",
                role.id
            ),
        ));
    }
    if guild.owner_id != actor.user.id && position >= top_role(guild, actor) {
        return Some(embeds::error(
            "Role above yours",
            format!(
                "<@&{}> is at or above your highest role. You can't have me hand it out.",
                role.id
            ),
        ));
    }
    None
}

/// Busca o membro. None SO quando ele de fato nao esta no servidor (404);
/// rate limit, 5xx e rede sobem como erro. Engolir tudo faria a checagem de
/// hierarquia ser pulada numa instabilidade qualquer - falha aberta.
pub async fn try_get_member(
    http: impl serenity::CacheHttp,
    guild_id: serenity::GuildId,
    user_id: serenity::UserId,
) -> Result<Option<serenity::Member>, Error> {
    match guild_id.member(http, user_id).await {
        Ok(member) => Ok(Some(member)),
        Err(serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(response)))
            if response.status_code == serenity::StatusCode::NOT_FOUND =>
        {
            Ok(None)
        }
        Err(cause) => Err(cause.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: u64 = 1;
    const BOT: (u64, i32) = (2, 10);

    fn refused(embed: Option<serenity::CreateEmbed>) -> bool {
        embed.is_some()
    }

    #[test]
    fn recusa_alvo_igual_ou_acima() {
        assert!(refused(check_positions(OWNER, (5, 3), (6, 3), Some(BOT))));
        assert!(refused(check_positions(OWNER, (5, 3), (6, 4), Some(BOT))));
        assert!(!refused(check_positions(OWNER, (5, 3), (6, 2), Some(BOT))));
    }

    #[test]
    fn recusa_a_si_mesmo_o_bot_e_o_dono() {
        assert!(refused(check_positions(OWNER, (5, 3), (5, 3), Some(BOT))));
        assert!(refused(check_positions(
            OWNER,
            (5, 3),
            (BOT.0, 1),
            Some(BOT)
        )));
        assert!(refused(check_positions(
            OWNER,
            (5, 9),
            (OWNER, -1),
            Some(BOT)
        )));
    }

    #[test]
    fn dono_passa_por_cima_do_cargo_mas_nao_do_bot() {
        assert!(!refused(check_positions(
            OWNER,
            (OWNER, -1),
            (6, 5),
            Some(BOT)
        )));
        assert!(refused(check_positions(
            OWNER,
            (OWNER, -1),
            (6, 10),
            Some(BOT)
        )));
    }

    #[test]
    fn sem_o_bot_recusa() {
        assert!(refused(check_positions(OWNER, (OWNER, 9), (6, 1), None)));
    }

    #[test]
    fn outranks_estrito() {
        assert!(outranks_positions(OWNER, (5, 4), (6, 3)));
        assert!(!outranks_positions(OWNER, (5, 3), (6, 3)));
        assert!(outranks_positions(OWNER, (OWNER, -1), (6, 3)));
        assert!(!outranks_positions(OWNER, (5, 9), (OWNER, -1)));
    }
}
