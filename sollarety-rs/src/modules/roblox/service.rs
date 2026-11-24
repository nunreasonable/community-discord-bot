//! Aplica o estado de verificacao de UMA pessoa em UM servidor: cargo de
//! verificado ou de nao verificado, cargos de bind de grupo e apelido
//! (VerificationService.cs).
//!
//! Nunca falha por causa do Discord: cada coisa que nao deu certo vira um
//! aviso na lista, e o resto continua. Um cargo acima do bot nao pode impedir
//! o apelido de ser trocado, nem o contrario.
//!
//! Em duvida, nao tira nada. Se a leitura dos grupos falha, os cargos de bind
//! ficam como estao - uma instabilidade do Roblox nao pode rebaixar o
//! servidor inteiro.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use indexmap::{IndexMap, IndexSet};
use poise::serenity_prelude as serenity;
use tracing::{error, warn};

use super::api::{self, Lookup};
use crate::{
    Data, embeds, hierarchy,
    store::{GuildSettings, RobloxLink},
};

/// O que um `apply` fez e o que ele nao conseguiu fazer.
#[derive(Clone, Debug, Default)]
pub struct ApplyReport {
    /// Falso quando o servidor nao configurou nada de verificacao.
    pub configured: bool,
    /// Preenchido quando o filtro anti-alt reprovou a conta.
    pub refusal: Option<String>,
    pub done: Vec<String>,
    pub warnings: Vec<String>,
}

impl ApplyReport {
    /// As linhas para um campo de embed, cortadas no limite dele.
    pub fn describe(&self) -> String {
        let lines: Vec<String> = self
            .done
            .iter()
            .map(|d| format!("• {d}"))
            .chain(self.warnings.iter().map(|w| format!("⚠️ {w}")))
            .collect();

        if lines.is_empty() {
            "Nothing to change: everything was already in place.".into()
        } else {
            embeds::trim(&lines.join("\n"), 1024)
        }
    }
}

/// O que o `apply` precisa saber do servidor, copiado do cache (ou da API)
/// antes de qualquer `.await`: a referencia do cache nao atravessa um.
#[derive(Clone, Copy, Debug)]
pub struct RoleInfo {
    pub position: i32,
    pub managed: bool,
    pub permissions: serenity::Permissions,
}

#[derive(Clone, Debug)]
pub struct GuildView {
    pub id: u64,
    pub owner_id: u64,
    pub roles: HashMap<u64, RoleInfo>,
}

impl GuildView {
    fn new<'a>(id: u64, owner_id: u64, roles: impl Iterator<Item = &'a serenity::Role>) -> Self {
        Self {
            id,
            owner_id,
            roles: roles
                .map(|r| {
                    (
                        r.id.get(),
                        RoleInfo {
                            position: i32::from(r.position),
                            managed: r.managed,
                            permissions: r.permissions,
                        },
                    )
                })
                .collect(),
        }
    }

    /// Do cache, se o bot esta no servidor. Junto vem o membro do proprio bot,
    /// se o cache o tiver (o GUILD_CREATE sempre traz).
    pub fn cached(
        cache: &serenity::Cache,
        guild_id: serenity::GuildId,
    ) -> Option<(Self, Option<serenity::Member>)> {
        let bot_id = cache.current_user().id;
        let guild = cache.guild(guild_id)?;
        let view = Self::new(guild.id.get(), guild.owner_id.get(), guild.roles.values());
        let bot = guild.members.get(&bot_id).cloned();
        Some((view, bot))
    }

    /// Posicao do cargo mais alto; sem cargo nenhum, -1 (abaixo do
    /// @everyone). Cargo que sumiu do servidor nao conta.
    pub fn top_position(&self, roles: &[serenity::RoleId]) -> i32 {
        roles
            .iter()
            .filter_map(|id| self.roles.get(&id.get()))
            .map(|r| r.position)
            .max()
            .unwrap_or(-1)
    }

    /// Permissao de SERVIDOR (a soma dos cargos, com o @everyone), como o
    /// `DiscordMember.Permissions` do C#. Overwrite de canal nao entra.
    pub fn permissions(&self, user_id: u64, roles: &[serenity::RoleId]) -> serenity::Permissions {
        if user_id == self.owner_id {
            return serenity::Permissions::all();
        }
        let mut permissions = self
            .roles
            .get(&self.id)
            .map(|r| r.permissions)
            .unwrap_or_default();
        for role in roles.iter().filter_map(|id| self.roles.get(&id.get())) {
            permissions |= role.permissions;
        }
        permissions
    }

    pub fn has(&self, member: &serenity::Member, needed: serenity::Permissions) -> bool {
        let permissions = self.permissions(member.user.id.get(), &member.roles);
        permissions.administrator() || permissions.contains(needed)
    }
}

/// O servidor e o membro do bot. Fora do cache, pela API; sem o bot, None -
/// sem saber onde ele esta na hierarquia, nao da para prever nada.
async fn load_guild(
    sctx: &serenity::Context,
    guild_id: serenity::GuildId,
) -> Option<(GuildView, serenity::Member)> {
    let (view, bot) = match GuildView::cached(&sctx.cache, guild_id) {
        Some(found) => found,
        None => {
            let partial = guild_id.to_partial_guild(sctx).await.ok()?;
            let view = GuildView::new(
                partial.id.get(),
                partial.owner_id.get(),
                partial.roles.values(),
            );
            (view, None)
        }
    };

    let bot = match bot {
        Some(bot) => bot,
        None => {
            let bot_id = sctx.cache.current_user().id;
            hierarchy::try_get_member(sctx, guild_id, bot_id)
                .await
                .ok()
                .flatten()?
        }
    };
    Some((view, bot))
}

pub fn is_configured(settings: Option<&GuildSettings>) -> bool {
    settings.is_some_and(GuildSettings::verification_configured)
}

pub async fn apply(
    sctx: &serenity::Context,
    data: &Data,
    guild_id: serenity::GuildId,
    member: &serenity::Member,
    link: Option<&RobloxLink>,
    reason: &str,
    fresh: bool,
) -> ApplyReport {
    let mut report = ApplyReport::default();
    let Some(settings) = data
        .guild_settings
        .for_guild(guild_id.get())
        .filter(GuildSettings::verification_configured)
    else {
        return report;
    };

    report.configured = true;
    if member.user.bot {
        return report;
    }

    let Some((guild, bot)) = load_guild(sctx, guild_id).await else {
        // Falha fechada, como no hierarchy::check.
        report.warnings.push(
            "I couldn't read my own role in this server, so I didn't change anything. Try again in a few seconds."
                .into(),
        );
        return report;
    };

    let mut eligible = link.is_some();
    if let Some(link) = link
        && let Some(refusal) = age_refusal(
            settings.min_account_age_days,
            link.roblox_created_utc,
            Utc::now(),
        )
    {
        eligible = false;
        report.refusal = Some(refusal);
    }

    let mut ranks = None;
    if eligible
        && let Some(link) = link
        && !settings.group_binds.is_empty()
    {
        match api::get_group_roles(&data.http, link.roblox_id, fresh).await {
            Lookup::Found(groups) => ranks = Some(best_ranks(&groups)),
            other => report.warnings.push(format!(
                "I couldn't read the Roblox groups ({}), so group roles were left as they were.",
                other.error().unwrap_or("account not found")
            )),
        }
    }

    let (add, remove) = plan(&settings, eligible, ranks.as_ref());
    apply_roles(
        sctx,
        &guild,
        member,
        &bot,
        &add,
        &remove,
        reason,
        &mut report,
    )
    .await;

    if eligible
        && let Some(link) = link
        && let Some(nickname) = link.nickname(settings.nickname_format.as_deref())
    {
        set_nickname(
            sctx,
            &guild,
            member,
            &bot,
            Some(&nickname),
            reason,
            &mut report,
        )
        .await;
    }

    report
}

/// Depois de um /unverify: volta o apelido ao normal, mas SO se ele ainda for
/// o que o bot poria. Se a pessoa (ou um moderador) trocou depois, o apelido
/// e dela e fica.
pub async fn clear_nickname_if_ours(
    sctx: &serenity::Context,
    data: &Data,
    guild_id: serenity::GuildId,
    member: &serenity::Member,
    old_link: &RobloxLink,
    reason: &str,
    report: &mut ApplyReport,
) {
    let format = data
        .guild_settings
        .for_guild(guild_id.get())
        .and_then(|s| s.nickname_format);
    let Some(ours) = old_link.nickname(format.as_deref()) else {
        return;
    };
    if member.nick.as_deref() != Some(ours.as_str()) {
        return;
    }

    let Some((guild, bot)) = load_guild(sctx, guild_id).await else {
        return;
    };
    set_nickname(sctx, &guild, member, &bot, None, reason, report).await;
}

/// Entrada de membro: quem ja tem vinculo recebe os cargos na hora, e quem
/// nao tem recebe o de nao verificado. Fora do caminho do gateway.
pub fn on_member_added(sctx: &serenity::Context, data: &Data, member: &serenity::Member) {
    if member.user.bot
        || !is_configured(
            data.guild_settings
                .for_guild(member.guild_id.get())
                .as_ref(),
        )
    {
        return;
    }

    let sctx = sctx.clone();
    let data = data.clone();
    let member = member.clone();
    tokio::spawn(async move {
        let link = data.links.for_user(member.user.id.get());
        let reason = crate::audit_reason::automatic(&match &link {
            None => "Roblox verification: joined without a linked account".to_owned(),
            Some(l) => format!(
                "Roblox verification: joined already linked to {} ({})",
                l.roblox_name, l.roblox_id
            ),
        });

        let report = apply(
            &sctx,
            &data,
            member.guild_id,
            &member,
            link.as_ref(),
            &reason,
            false,
        )
        .await;
        if !report.warnings.is_empty() {
            warn!(
                "[verificacao] aviso: entrada de {} em {}: {}",
                member.user.id,
                member.guild_id,
                report.warnings.join(" | ")
            );
        }
    });
}

/// None se a conta passa no filtro de idade, ou o motivo da recusa.
pub(super) fn age_refusal(
    min_days: i32,
    created: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<String> {
    if min_days <= 0 {
        return None;
    }
    let Some(created) = created else {
        return Some(
            "I couldn't confirm the Roblox account's age, and this server requires a minimum age."
                .into(),
        );
    };

    let age = now - created;
    if age >= chrono::Duration::days(i64::from(min_days)) {
        return None;
    }
    let days = age.num_days().max(0);
    Some(format!(
        "the Roblox account is {days} day(s) old, and this server requires at least {min_days}."
    ))
}

/// O rank mais alto de cada grupo.
fn best_ranks(groups: &[api::RobloxGroupRole]) -> HashMap<i64, i32> {
    let mut ranks = HashMap::new();
    for g in groups {
        let rank = ranks.entry(g.group_id).or_insert(g.rank);
        *rank = (*rank).max(g.rank);
    }
    ranks
}

/// Os cargos a dar e a tirar. `ranks` None com binds e "a leitura dos
/// grupos falhou": os cargos de bind nao entram em nenhuma das listas.
/// Ordem de insercao, como o HashSet do C# na pratica: os avisos saem na
/// mesma ordem.
pub(super) fn plan(
    settings: &GuildSettings,
    eligible: bool,
    ranks: Option<&HashMap<i64, i32>>,
) -> (IndexSet<u64>, IndexSet<u64>) {
    let mut add = IndexSet::new();
    let mut remove = IndexSet::new();
    let verified = settings.verified_role_id.filter(|&id| id > 0);
    let unverified = settings.unverified_role_id.filter(|&id| id > 0);

    if eligible {
        add.extend(verified);
        remove.extend(unverified);

        // Por CARGO, nao por bind: dois binds podem apontar para o mesmo
        // cargo (rank 100-199 do grupo A OU qualquer rank do grupo B), e o
        // cargo fica se QUALQUER um casar.
        if let Some(ranks) = ranks {
            let mut earned: IndexMap<u64, bool> = IndexMap::new();
            for bind in &settings.group_binds {
                let rank = ranks.get(&bind.group_id).copied().unwrap_or(0);
                *earned.entry(bind.role_id).or_default() |= bind.matches(rank);
            }
            for (role, earned) in earned {
                if earned {
                    add.insert(role);
                } else {
                    remove.insert(role);
                }
            }
        }
    } else {
        add.extend(unverified);
        remove.extend(verified);
        remove.extend(settings.group_binds.iter().map(|b| b.role_id));
    }

    // Um cargo pedido pelos dois lados (um bind que aponta para o proprio
    // cargo de verificado, por exemplo) fica: tirar o que outra regra acabou
    // de dar so geraria dois eventos no Audit Log.
    remove.retain(|id| !add.contains(id));
    (add, remove)
}

#[allow(clippy::too_many_arguments)]
async fn apply_roles(
    sctx: &serenity::Context,
    guild: &GuildView,
    member: &serenity::Member,
    bot: &serenity::Member,
    add: &IndexSet<u64>,
    remove: &IndexSet<u64>,
    reason: &str,
    report: &mut ApplyReport,
) {
    let current: IndexSet<u64> = member.roles.iter().map(|r| r.get()).collect();
    let to_add: Vec<u64> = add
        .iter()
        .copied()
        .filter(|id| !current.contains(id))
        .collect();
    let to_remove: Vec<u64> = remove
        .iter()
        .copied()
        .filter(|id| current.contains(id))
        .collect();

    // Cargo configurado que sumiu do servidor: vale avisar mesmo sem ter o
    // que mudar nele, senao o /config fica apontando para o nada.
    for missing in add.iter().filter(|id| !guild.roles.contains_key(id)) {
        report.warnings.push(format!(
            "The configured role `{missing}` no longer exists in this server."
        ));
    }

    if to_add.is_empty() && to_remove.is_empty() {
        return;
    }

    if !guild.has(bot, serenity::Permissions::MANAGE_ROLES) {
        report
            .warnings
            .push("I'm missing **Manage Roles**, so I didn't change any roles.".into());
        return;
    }

    let bot_top = guild.top_position(&bot.roles);
    let changes = to_add
        .iter()
        .map(|&id| (id, true))
        .chain(to_remove.iter().map(|&id| (id, false)));

    for (id, grant) in changes {
        let Some(role) = guild.roles.get(&id) else {
            continue;
        };

        if role.managed || role.position >= bot_top {
            report.warnings.push(format!(
                "<@&{id}> is above my highest role, so I couldn't {}",
                if grant { "give it." } else { "remove it." }
            ));
            continue;
        }

        let role_id = serenity::RoleId::new(id);
        let guild_id = serenity::GuildId::new(guild.id);
        let result = if grant {
            sctx.http
                .add_member_role(guild_id, member.user.id, role_id, Some(reason))
                .await
        } else {
            sctx.http
                .remove_member_role(guild_id, member.user.id, role_id, Some(reason))
                .await
        };

        match result {
            Ok(()) => report.done.push(if grant {
                format!("Role <@&{id}> added")
            } else {
                format!("Role <@&{id}> removed")
            }),
            Err(cause) => {
                warn!(
                    "[verificacao] aviso: {} {id} de {} em {}: {cause}",
                    if grant { "dar" } else { "tirar" },
                    member.user.id,
                    guild.id
                );
                report.warnings.push(format!(
                    "Discord refused to {} <@&{id}>.",
                    if grant { "give" } else { "remove" }
                ));
            }
        }
    }
}

async fn set_nickname(
    sctx: &serenity::Context,
    guild: &GuildView,
    member: &serenity::Member,
    bot: &serenity::Member,
    nickname: Option<&str>,
    reason: &str,
    report: &mut ApplyReport,
) {
    if member.nick.as_deref() == nickname {
        return;
    }

    // Limites do proprio Discord, checados antes para a mensagem dizer o
    // motivo em vez de repassar um 403 cru.
    if guild.owner_id == member.user.id.get() {
        report
            .warnings
            .push("Discord doesn't let any bot change the server owner's nickname.".into());
        return;
    }

    if !guild.has(bot, serenity::Permissions::MANAGE_NICKNAMES) {
        report
            .warnings
            .push("I'm missing **Manage Nicknames**, so the nickname was left as it was.".into());
        return;
    }

    if guild.top_position(&member.roles) >= guild.top_position(&bot.roles) {
        report.warnings.push(
            "This member's highest role is at or above mine, so I can't change their nickname."
                .into(),
        );
        return;
    }

    // String vazia para limpar: o Discord trata como "volta ao nome da conta".
    let edit = serenity::EditMember::new()
        .nickname(nickname.unwrap_or_default())
        .audit_log_reason(reason);
    match serenity::GuildId::new(guild.id)
        .edit_member(sctx, member.user.id, edit)
        .await
    {
        Ok(_) => report.done.push(match nickname {
            None => "Nickname reset to their Discord name".into(),
            Some(nick) => format!("Nickname changed to **{}**", embeds::safe(nick)),
        }),
        Err(cause) => {
            warn!(
                "[verificacao] aviso: apelido de {} em {}: {cause}",
                member.user.id, guild.id
            );
            report
                .warnings
                .push("Discord refused the nickname change.".into());
        }
    }
}

/// Para quem so quer registrar uma falha inesperada no mesmo formato.
pub(super) fn log_failure(what: &str, cause: &dyn std::fmt::Display) {
    error!("[verificacao] {what}: {cause}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::GroupBind;

    fn bind(group: i64, role: u64, min: i32, max: i32) -> GroupBind {
        GroupBind {
            group_id: group,
            role_id: role,
            min_rank: min,
            max_rank: max,
            ..GroupBind::default()
        }
    }

    fn settings() -> GuildSettings {
        GuildSettings {
            verified_role_id: Some(10),
            unverified_role_id: Some(11),
            group_binds: vec![
                bind(1, 20, 100, 199),
                bind(2, 20, 1, 255),
                bind(1, 21, 1, 255),
            ],
            ..GuildSettings::new(5)
        }
    }

    fn list(set: &IndexSet<u64>) -> Vec<u64> {
        set.iter().copied().collect()
    }

    #[test]
    fn verificado_ganha_binds_por_cargo() {
        let ranks = HashMap::from([(2, 5)]);
        let (add, remove) = plan(&settings(), true, Some(&ranks));
        // 20 vem do grupo 2; 21 exige estar no grupo 1.
        assert_eq!(list(&add), vec![10, 20]);
        assert_eq!(list(&remove), vec![11, 21]);
    }

    #[test]
    fn falha_nos_grupos_nao_mexe_nos_binds() {
        let (add, remove) = plan(&settings(), true, None);
        assert_eq!(list(&add), vec![10]);
        assert_eq!(list(&remove), vec![11]);
    }

    #[test]
    fn nao_verificado_perde_tudo_e_ganha_o_cargo_de_fora() {
        let (add, remove) = plan(&settings(), false, None);
        assert_eq!(list(&add), vec![11]);
        assert_eq!(list(&remove), vec![10, 20, 21]);
    }

    #[test]
    fn cargo_pedido_pelos_dois_lados_fica() {
        let mut s = settings();
        s.group_binds = vec![bind(1, 10, 1, 255)];
        let (add, remove) = plan(&s, true, Some(&HashMap::new()));
        assert_eq!(list(&add), vec![10]);
        assert_eq!(list(&remove), vec![11]);
    }

    #[test]
    fn rank_mais_alto_por_grupo() {
        let role = |g, r| api::RobloxGroupRole {
            group_id: g,
            group_name: String::new(),
            rank: r,
            role_name: String::new(),
        };
        let ranks = best_ranks(&[role(1, 5), role(1, 200), role(2, 3)]);
        assert_eq!(ranks[&1], 200);
        assert_eq!(ranks[&2], 3);
    }

    #[test]
    fn filtro_de_idade() {
        let now = Utc::now();
        assert_eq!(age_refusal(0, None, now), None);
        assert!(
            age_refusal(7, None, now)
                .unwrap()
                .contains("couldn't confirm")
        );
        assert_eq!(
            age_refusal(7, Some(now - chrono::Duration::hours(80)), now).unwrap(),
            "the Roblox account is 3 day(s) old, and this server requires at least 7."
        );
        assert_eq!(
            age_refusal(7, Some(now + chrono::Duration::days(1)), now).unwrap(),
            "the Roblox account is 0 day(s) old, and this server requires at least 7."
        );
        assert_eq!(
            age_refusal(7, Some(now - chrono::Duration::days(7)), now),
            None
        );
    }

    #[test]
    fn relatorio() {
        let mut report = ApplyReport::default();
        assert_eq!(
            report.describe(),
            "Nothing to change: everything was already in place."
        );
        report.done.push("a".into());
        report.warnings.push("b".into());
        assert_eq!(report.describe(), "• a\n⚠️ b");
    }

    #[test]
    fn permissao_e_posicao_pelos_cargos() {
        let role = |position, permissions| RoleInfo {
            position,
            managed: false,
            permissions,
        };
        let view = GuildView {
            id: 1,
            owner_id: 9,
            roles: HashMap::from([
                (1, role(0, serenity::Permissions::SEND_MESSAGES)),
                (2, role(3, serenity::Permissions::MANAGE_ROLES)),
                (3, role(5, serenity::Permissions::ADMINISTRATOR)),
            ]),
        };
        let r = |ids: &[u64]| {
            ids.iter()
                .map(|&i| serenity::RoleId::new(i))
                .collect::<Vec<_>>()
        };

        assert_eq!(view.top_position(&r(&[])), -1);
        assert_eq!(view.top_position(&r(&[2, 99])), 3);
        let p = view.permissions(4, &r(&[2]));
        assert!(
            p.contains(serenity::Permissions::SEND_MESSAGES | serenity::Permissions::MANAGE_ROLES)
        );
        assert!(!p.manage_nicknames());
        assert!(view.permissions(9, &[]).manage_nicknames());
        assert!(view.permissions(4, &r(&[3])).administrator());
    }
}
