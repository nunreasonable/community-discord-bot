//! A trava de servidor do modulo e as regras de permissao que os comandos
//! repetem. As decisoes sao funcoes puras (posicoes, ids e bits de
//! permissao), testadas sem Discord; quem chama copia o que precisa do cache
//! antes de qualquer `.await`.

use poise::{CreateReply, serenity_prelude as serenity};

use crate::{Context, Error, audit_reason, config, embeds};

/// Prefixo de todo custom id do modulo (botoes e modais).
pub const PREFIX: &str = "admin:";

// ---------------------------------------------------------------------------
// Trava de servidor
// ---------------------------------------------------------------------------

/// O servidor esta em admin.guildIds AGORA? Lido do config a cada uso: a
/// lista pode mudar sem novo registro dos comandos, e um comando que ficou
/// no menu de um servidor tirado da lista tem de parar de funcionar na hora.
pub fn guild_allowed(guild_id: Option<serenity::GuildId>) -> bool {
    guild_id.is_some_and(|g| config::current().admin.allows(g.get()))
}

pub fn locked_embed() -> serenity::CreateEmbed {
    embeds::error(
        "Not available here",
        "Admin commands are only enabled in the servers configured for them.",
    )
}

/// Primeira linha de todo comando: fora dos servidores do admin, recusa em
/// efemero e devolve false. Vem antes de qualquer defer ou modal.
pub async fn ensure(ctx: Context<'_>) -> Result<bool, Error> {
    if guild_allowed(ctx.guild_id()) {
        return Ok(true);
    }
    tracing::warn!(
        "[admin] /{} de {} recusado: o servidor {} nao esta em admin.guildIds",
        ctx.command().qualified_name,
        ctx.author().id,
        ctx.guild_id()
            .map_or("(nenhum)".to_owned(), |g| g.to_string())
    );
    ctx.send(CreateReply::default().embed(locked_embed()).ephemeral(true))
        .await?;
    Ok(false)
}

// ---------------------------------------------------------------------------
// Pecas de Discord
// ---------------------------------------------------------------------------

/// O `UsernameWithDiscriminator` do DisCatSharp ("nome#0"), igual ao da
/// moderacao, para o audit log sair no mesmo formato.
pub fn tag(user: &serenity::User) -> String {
    match user.discriminator {
        Some(d) => format!("{}#{:04}", user.name, d.get()),
        None => format!("{}#0", user.name),
    }
}

/// Motivo do audit log com quem mandou fazer.
pub fn audit(ctx: Context<'_>, reason: Option<&str>) -> String {
    audit_reason::for_actor(&tag(ctx.author()), reason)
}

/// O membro do proprio bot: cache, e API quando o cache esta frio. None faz
/// quem chama recusar - sem ele nao ha hierarquia para conferir.
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

/// O membro de quem usou o comando, pronto na interacao.
pub async fn actor(ctx: Context<'_>) -> Result<serenity::Member, Error> {
    ctx.author_member()
        .await
        .map(std::borrow::Cow::into_owned)
        .ok_or_else(|| "nao consegui ler o membro de quem usou o comando".into())
}

/// O texto de um embed pronto (titulo e descricao), para o registro de uma
/// recusa que veio pronta do `hierarchy`.
pub fn embed_text(embed: &serenity::CreateEmbed) -> String {
    let value = serde_json::to_value(embed).unwrap_or_default();
    let field = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned()
    };
    match (field("title"), field("description")) {
        (t, d) if t.is_empty() => d,
        (t, d) if d.is_empty() => t,
        (t, d) => format!("{t}: {d}"),
    }
}

/// Responde ao `respond` de um comando que ja deferiu: edita a resposta, que
/// continua sendo UMA mensagem.
pub async fn respond(ctx: Context<'_>, embed: serenity::CreateEmbed) -> Result<(), Error> {
    if let poise::Context::Application(app) = ctx {
        app.interaction
            .edit_response(
                ctx.http(),
                serenity::EditInteractionResponse::new()
                    .embed(embed)
                    .components(Vec::new()),
            )
            .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Regras de cargo (puras)
// ---------------------------------------------------------------------------

/// O que importa de um cargo para decidir se ele pode ser mexido.
#[derive(Clone, Debug)]
pub struct RoleFacts {
    pub id: u64,
    pub position: i32,
    pub managed: bool,
    pub permissions: serenity::Permissions,
}

impl RoleFacts {
    pub fn of(role: &serenity::Role) -> Self {
        Self {
            id: role.id.get(),
            position: i32::from(role.position),
            managed: role.managed,
            permissions: role.permissions,
        }
    }
}

/// Quem pede: id, posicao do cargo mais alto e as permissoes de SERVIDOR
/// (sem sobrescrita de canal - cargo e coisa do servidor inteiro).
#[derive(Clone, Copy, Debug)]
pub struct ActorFacts {
    pub id: u64,
    pub top: i32,
    pub permissions: serenity::Permissions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Add,
    Remove,
}

/// Por que um cargo nao pode ser mexido. As regras do painel do ccore
/// (role_refusal + bot_top_position), trocando as patentes pela hierarquia e
/// pelas permissoes do proprio Discord.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoleRefusal {
    Everyone,
    Managed,
    AboveBot,
    AboveActor,
    Administrator,
    /// So ao DAR o cargo: ele carrega permissao que quem pede nao tem.
    Escalation(serenity::Permissions),
}

impl RoleRefusal {
    /// A frase inteira, para a recusa de um cargo so.
    pub fn sentence(&self, role_id: u64) -> String {
        let role = format!("<@&{role_id}>");
        match self {
            Self::Everyone => "@everyone can't be added or removed.".to_owned(),
            Self::Managed => format!(
                "{role} is managed by an integration (a bot or boosts), so no one can add or remove it manually."
            ),
            Self::AboveBot => format!(
                "{role} is at or above my highest role. Move my role higher in the server's role list."
            ),
            Self::AboveActor => format!("{role} is at or above your highest role."),
            Self::Administrator => {
                format!("{role} has Administrator. Only the server owner can add or remove it.")
            }
            Self::Escalation(missing) => format!(
                "{role} grants permissions you don't have yourself: {}.",
                permission_names(*missing)
            ),
        }
    }

    /// O motivo curto, para a lista de cargos pulados do /role strip.
    pub fn short(&self) -> String {
        match self {
            Self::Everyone => "@everyone".to_owned(),
            Self::Managed => "managed by an integration".to_owned(),
            Self::AboveBot => "at or above my role".to_owned(),
            Self::AboveActor => "at or above your role".to_owned(),
            Self::Administrator => "has Administrator (owner only)".to_owned(),
            Self::Escalation(_) => "grants permissions you don't have".to_owned(),
        }
    }
}

/// "Ban Members, Manage Server" - nomes do proprio Discord.
pub fn permission_names(permissions: serenity::Permissions) -> String {
    permissions.get_permission_names().join(", ")
}

/// None quando `actor` (e o bot) podem dar/tirar o cargo.
///
/// - @everyone e cargo de integracao: nem o Discord deixa.
/// - Cargo na altura do bot ou acima: o Discord recusaria.
/// - Cargo na altura de quem pede ou acima: o Discord nao deixaria a pessoa
///   fazer isso com as proprias maos; o bot nao pode ser o atalho.
/// - Administrator: so o dono, nos dois sentidos - dar e escalar tudo, e
///   tirar de alguem e derrubar outro admin.
/// - Ao dar: o cargo nao pode carregar permissao que quem pede nao tem. Sem
///   isso, um mod com Gerenciar Cargos e sem Banir daria a si mesmo (ou a um
///   comparsa) um cargo mais baixo que bane - a mesma escalada que o ccore
///   barrava listando cargos perigosos.
///
/// O dono passa por cima das regras de quem pede, mas nao das do bot.
pub fn role_refusal(
    guild_id: u64,
    owner_id: u64,
    actor: &ActorFacts,
    bot_top: i32,
    role: &RoleFacts,
    change: Change,
) -> Option<RoleRefusal> {
    if role.id == guild_id {
        return Some(RoleRefusal::Everyone);
    }
    if role.managed {
        return Some(RoleRefusal::Managed);
    }
    if role.position >= bot_top {
        return Some(RoleRefusal::AboveBot);
    }
    if actor.id == owner_id {
        return None;
    }
    if role.position >= actor.top {
        return Some(RoleRefusal::AboveActor);
    }
    if role.permissions.administrator() {
        return Some(RoleRefusal::Administrator);
    }
    if change == Change::Add && !actor.permissions.administrator() {
        let missing = role.permissions - actor.permissions;
        if !missing.is_empty() {
            return Some(RoleRefusal::Escalation(missing));
        }
    }
    None
}

/// Divide os cargos de um membro entre os que podem sair e os que ficam (com
/// o motivo). Ordem: do mais alto para o mais baixo, como no perfil.
pub fn plan_strip(
    guild_id: u64,
    owner_id: u64,
    actor: &ActorFacts,
    bot_top: i32,
    roles: &[RoleFacts],
) -> (Vec<RoleFacts>, Vec<(RoleFacts, RoleRefusal)>) {
    let mut sorted = roles.to_vec();
    sorted.sort_by(|a, b| b.position.cmp(&a.position).then(a.id.cmp(&b.id)));
    let mut removable = Vec::new();
    let mut skipped = Vec::new();
    for role in sorted {
        match role_refusal(guild_id, owner_id, actor, bot_top, &role, Change::Remove) {
            None => removable.push(role),
            Some(refusal) => skipped.push((role, refusal)),
        }
    }
    (removable, skipped)
}

// ---------------------------------------------------------------------------
// Regras de canal (puras)
// ---------------------------------------------------------------------------

/// Ver o canal e escrever nele: o minimo para o /send. Administrator ja vem
/// expandido em todas as permissoes pelo calculo do serenity.
pub fn can_post(permissions: serenity::Permissions) -> bool {
    permissions.contains(serenity::Permissions::VIEW_CHANNEL | serenity::Permissions::SEND_MESSAGES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use poise::serenity_prelude::Permissions as P;

    const GUILD: u64 = 100;
    const OWNER: u64 = 1;
    const BOT_TOP: i32 = 10;

    fn actor(top: i32, permissions: P) -> ActorFacts {
        ActorFacts {
            id: 5,
            top,
            permissions,
        }
    }

    fn role(id: u64, position: i32, permissions: P) -> RoleFacts {
        RoleFacts {
            id,
            position,
            managed: false,
            permissions,
        }
    }

    fn mod_perms() -> P {
        P::MANAGE_ROLES | P::SEND_MESSAGES | P::VIEW_CHANNEL
    }

    #[test]
    fn everyone_e_integracao_nunca() {
        let a = actor(8, mod_perms());
        assert_eq!(
            role_refusal(
                GUILD,
                OWNER,
                &a,
                BOT_TOP,
                &role(GUILD, 0, P::empty()),
                Change::Add
            ),
            Some(RoleRefusal::Everyone)
        );
        let mut managed = role(7, 2, P::empty());
        managed.managed = true;
        assert_eq!(
            role_refusal(GUILD, OWNER, &a, BOT_TOP, &managed, Change::Remove),
            Some(RoleRefusal::Managed)
        );
    }

    #[test]
    fn hierarquia_do_bot_e_de_quem_pede() {
        let a = actor(8, mod_perms());
        let check = |position| {
            role_refusal(
                GUILD,
                OWNER,
                &a,
                BOT_TOP,
                &role(7, position, P::empty()),
                Change::Add,
            )
        };
        assert_eq!(check(10), Some(RoleRefusal::AboveBot));
        assert_eq!(check(9), Some(RoleRefusal::AboveActor));
        assert_eq!(check(8), Some(RoleRefusal::AboveActor));
        assert_eq!(check(7), None);
    }

    #[test]
    fn dono_passa_por_cima_de_quem_pede_mas_nao_do_bot() {
        let owner = ActorFacts {
            id: OWNER,
            top: -1,
            permissions: P::empty(),
        };
        let admin_role = role(7, 9, P::ADMINISTRATOR);
        assert_eq!(
            role_refusal(GUILD, OWNER, &owner, BOT_TOP, &admin_role, Change::Add),
            None
        );
        assert_eq!(
            role_refusal(
                GUILD,
                OWNER,
                &owner,
                BOT_TOP,
                &role(7, 10, P::empty()),
                Change::Add
            ),
            Some(RoleRefusal::AboveBot)
        );
    }

    #[test]
    fn administrator_so_o_dono_nos_dois_sentidos() {
        // Mesmo quem e Administrator nao mexe em cargo de Administrator.
        let a = actor(9, P::ADMINISTRATOR);
        let admin_role = role(7, 3, P::ADMINISTRATOR | P::SEND_MESSAGES);
        for change in [Change::Add, Change::Remove] {
            assert_eq!(
                role_refusal(GUILD, OWNER, &a, BOT_TOP, &admin_role, change),
                Some(RoleRefusal::Administrator)
            );
        }
    }

    #[test]
    fn dar_cargo_com_permissao_que_nao_tem_e_escalada() {
        let a = actor(8, mod_perms());
        let bans = role(7, 3, P::BAN_MEMBERS | P::SEND_MESSAGES);
        assert_eq!(
            role_refusal(GUILD, OWNER, &a, BOT_TOP, &bans, Change::Add),
            Some(RoleRefusal::Escalation(P::BAN_MEMBERS))
        );
        // Tirar nao escala nada.
        assert_eq!(
            role_refusal(GUILD, OWNER, &a, BOT_TOP, &bans, Change::Remove),
            None
        );
        // Quem e Administrator tem tudo.
        let admin = actor(8, P::ADMINISTRATOR);
        assert_eq!(
            role_refusal(GUILD, OWNER, &admin, BOT_TOP, &bans, Change::Add),
            None
        );
        assert!(
            RoleRefusal::Escalation(P::BAN_MEMBERS)
                .sentence(7)
                .contains("Ban Members")
        );
    }

    #[test]
    fn strip_separa_e_ordena() {
        let a = actor(8, mod_perms());
        let mut managed = role(4, 1, P::empty());
        managed.managed = true;
        let roles = vec![
            role(2, 2, P::empty()),
            role(3, 9, P::empty()),
            managed,
            role(5, 6, P::BAN_MEMBERS),
            role(6, 4, P::ADMINISTRATOR),
        ];
        let (removable, skipped) = plan_strip(GUILD, OWNER, &a, BOT_TOP, &roles);
        // Tirar cargo com permissao que a pessoa nao tem e permitido; o de
        // Administrator, nao.
        assert_eq!(removable.iter().map(|r| r.id).collect::<Vec<_>>(), [5, 2]);
        let skipped: Vec<(u64, RoleRefusal)> =
            skipped.into_iter().map(|(r, why)| (r.id, why)).collect();
        assert_eq!(
            skipped,
            [
                (3, RoleRefusal::AboveActor),
                (6, RoleRefusal::Administrator),
                (4, RoleRefusal::Managed),
            ]
        );
    }

    #[test]
    fn postar_exige_ver_e_escrever() {
        assert!(can_post(
            P::VIEW_CHANNEL | P::SEND_MESSAGES | P::EMBED_LINKS
        ));
        assert!(!can_post(P::VIEW_CHANNEL));
        assert!(!can_post(P::SEND_MESSAGES));
    }

    #[test]
    fn tag_e_texto_do_embed() {
        let mut user = serenity::User::default();
        user.name = "fulano".into();
        assert_eq!(tag(&user), "fulano#0");
        assert_eq!(
            embed_text(&embeds::error("Role hierarchy", "x is above")),
            "Role hierarchy: x is above"
        );
        assert_eq!(embed_text(&serenity::CreateEmbed::new()), "");
    }
}
