//! Binds de grupo: "quem esta no grupo X com rank entre A e B ganha o cargo
//! Y" (Binds.cs).
//!
//! Exige Gerenciar Servidor E Gerenciar Cargos. Um bind e uma regra que da
//! cargo sozinha a quem entra, entao pede as duas coisas que ele mexe:
//! configuracao do servidor e distribuicao de cargo. E o cargo escolhido
//! ainda passa pela checagem de hierarquia de quem cria o bind - ver
//! hierarchy::check_assignable_role.

use poise::{CreateReply, serenity_prelude as serenity};

use super::api;
use crate::{
    Context, Error, embeds, hierarchy, scope,
    store::{GroupBind, short_id, time},
};

/// Cada bind custa, por pessoa verificada, zero chamadas extras ao Roblox
/// (uma leitura de grupos serve a todos), mas a lista precisa caber num
/// embed do /bind list. Cinquenta cabem com folga.
const MAX_BINDS: usize = 50;

/// Discord roles based on Roblox group and rank
#[poise::command(
    slash_command,
    subcommands("add", "remove", "list"),
    subcommand_required,
    guild_only,
    default_member_permissions = "MANAGE_GUILD | MANAGE_ROLES",
    required_permissions = "MANAGE_GUILD | MANAGE_ROLES"
)]
pub async fn bind(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// O `EditResponseAsync` do C#: troca a resposta deferida pelo embed.
async fn edit(ctx: Context<'_>, embed: serenity::CreateEmbed) -> Result<(), Error> {
    let poise::Context::Application(app) = ctx else {
        return Err("bind fora de slash".into());
    };
    app.interaction
        .edit_response(ctx, serenity::EditInteractionResponse::new().embed(embed))
        .await?;
    Ok(())
}

/// Gives a role to members of a Roblox group within a rank range
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD | MANAGE_ROLES"
)]
pub async fn add(
    ctx: Context<'_>,
    #[description = "Roblox group ID (the number in the group's URL)"] group: i64,
    #[description = "Discord role the bind gives"] role: serenity::Role,
    #[description = "Lowest rank that gets the role, 0 to 255 (default 1: any member)"]
    min_rank: Option<i64>,
    #[description = "Highest rank that gets the role, 0 to 255 (default 255)"] max_rank: Option<
        i64,
    >,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().ok_or("sem servidor")?;
    let min_rank = min_rank.unwrap_or(1);
    let max_rank = max_rank.unwrap_or(255);

    if group <= 0 {
        return edit(ctx, embeds::error(
            "Invalid group",
            "The group ID is the number in its URL, as in `roblox.com/communities/1234567/...`.",
        ))
        .await;
    }

    if !valid_range(min_rank, max_rank) {
        return edit(ctx, embeds::error(
            "Invalid range",
            "Ranks go from 0 to 255, and the minimum can't be higher than the maximum. Rank 0 means being **outside** the group.",
        ))
        .await;
    }

    let actor = ctx
        .author_member()
        .await
        .ok_or("membro de quem chamou indisponivel")?
        .into_owned();
    let refusal = {
        let bot_id = ctx.cache().current_user().id;
        match ctx.cache().guild(guild_id) {
            Some(guild) => {
                hierarchy::check_assignable_role(&guild, &actor, &role, guild.members.get(&bot_id))
            }
            // Sem o servidor no cache nao ha como saber onde o bot esta.
            None => Some(embeds::error(
                "Role hierarchy unavailable",
                "I couldn't read my own role right now. Try again in a few seconds.",
            )),
        }
    };
    if let Some(refusal) = refusal {
        return edit(ctx, refusal).await;
    }

    let found = match api::get_group(&ctx.data().http, group).await {
        api::Lookup::Found(found) => found,
        api::Lookup::Missing => {
            return edit(
                ctx,
                embeds::error(
                    "Group not found",
                    format!("Roblox says group `{group}` doesn't exist."),
                ),
            )
            .await;
        }
        api::Lookup::Fail(error) => {
            return edit(
                ctx,
                embeds::error(
                    "Couldn't confirm the group",
                    format!("{error}. Try again in a few seconds."),
                ),
            )
            .await;
        }
    };

    let actor_id = ctx.author().id.get();
    let role_id = role.id.get();
    let created = ctx
        .data()
        .guild_settings
        .update(|edit| {
            let settings = edit.get(guild_id.get());

            if settings.group_binds.len() >= MAX_BINDS {
                return Err(format!(
                    "This server already has {MAX_BINDS} binds, the maximum. Remove one with `/bind remove`."
                ));
            }

            if settings.group_binds.iter().any(|b| {
                b.group_id == group
                    && b.role_id == role_id
                    && i64::from(b.min_rank) == min_rank
                    && i64::from(b.max_rank) == max_rank
            }) {
                return Err("An identical bind already exists.".into());
            }

            let id = loop {
                let id = short_id(6);
                if !settings.group_binds.iter().any(|b| same_id(&b.id, &id)) {
                    break id;
                }
            };

            let now = time::now();
            let bind = GroupBind {
                id,
                group_id: group,
                group_name: embeds::trim(&found.name, 100),
                // valid_range ja garantiu 0..=255.
                min_rank: min_rank as i32,
                max_rank: max_rank as i32,
                role_id,
                created_by_id: actor_id,
                created_at_utc: now,
            };

            settings.group_binds.push(bind.clone());
            settings.updated_at_utc = Some(now);
            settings.updated_by_id = Some(actor_id);
            edit.mark_changed();
            Ok(bind)
        })
        .await?;

    match created {
        Err(error) => edit(ctx, embeds::error("Bind not created", error)).await,
        Ok(bind) => {
            edit(ctx, embeds::ok(
                "Bind created",
                format!(
                    "{}\n\nApplies to anyone who verifies or joins from now on. Members who are **already** verified get it on their next \
                     `/update` — they can run it themselves, or someone with Manage Roles can run it for them.",
                    describe(&bind)
                ),
            ))
            .await
        }
    }
}

/// Removes a bind by the id shown in /bind list
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD | MANAGE_ROLES"
)]
pub async fn remove(ctx: Context<'_>, #[description = "Bind id"] id: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().ok_or("sem servidor")?;
    let actor_id = ctx.author().id.get();
    let wanted = id.trim();

    let removed = ctx
        .data()
        .guild_settings
        .update(|edit| {
            let settings = edit.get(guild_id.get());
            let index = settings
                .group_binds
                .iter()
                .position(|b| same_id(&b.id, wanted))?;

            let bind = settings.group_binds.remove(index);
            settings.updated_at_utc = Some(time::now());
            settings.updated_by_id = Some(actor_id);
            edit.mark_changed();
            Some(bind)
        })
        .await?;

    let embed = match removed {
        None => embeds::error(
            "Bind not found",
            format!(
                "There's no bind with id `{}` in this server. See `/bind list`.",
                embeds::safe_trim(&id, 20)
            ),
        ),
        Some(bind) => embeds::ok(
            "Bind removed",
            format!(
                "{}\n\nThe bot stops giving this role. Members who have it **keep it**: with no bind \
                 pointing to the role, the bot stops touching it entirely, including removing it.",
                describe(&bind)
            ),
        ),
    };
    edit(ctx, embed).await
}

/// Lists this server's group binds
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD | MANAGE_ROLES"
)]
pub async fn list(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or("sem servidor")?;
    let binds = ctx
        .data()
        .guild_settings
        .for_guild(guild_id.get())
        .map(|s| s.group_binds)
        .unwrap_or_default();

    let embed = if binds.is_empty() {
        embeds::info(
            "No binds",
            "This server has no group binds. Create one with `/bind add`.",
        )
    } else {
        let lines: Vec<String> = binds
            .iter()
            .map(|b| format!("`{}` {}", b.id, describe(b)))
            .collect();
        serenity::CreateEmbed::new()
            .title(format!("Group binds ({})", binds.len()))
            .colour(embeds::colors::BLURPLE)
            .description(embeds::trim(&lines.join("\n"), 4000))
    };

    ctx.send(CreateReply::default().embed(embed).ephemeral(true))
        .await?;
    Ok(())
}

fn valid_range(min: i64, max: i64) -> bool {
    (0..=255).contains(&min) && (0..=255).contains(&max) && min <= max
}

/// O `OrdinalIgnoreCase` do C#.
fn same_id(a: &str, b: &str) -> bool {
    a.to_uppercase() == b.to_uppercase()
}

fn describe(bind: &GroupBind) -> String {
    let ranks = match (bind.min_rank, bind.max_rank) {
        (1, 255) => "any rank".to_owned(),
        (0, 255) => "anyone, even outside the group".to_owned(),
        (min, max) if min == max => format!("rank {min}"),
        (min, max) => format!("ranks {min}–{max}"),
    };

    format!(
        "**{}** (`{}`), {ranks} → <@&{}>",
        embeds::safe(&bind.group_name),
        bind.group_id,
        bind.role_id
    )
}

pub fn commands() -> Vec<scope::Command> {
    vec![scope::guild_only(bind())]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bind(min: i32, max: i32) -> GroupBind {
        GroupBind {
            group_id: 7,
            group_name: "[G]".into(),
            min_rank: min,
            max_rank: max,
            role_id: 9,
            ..GroupBind::default()
        }
    }

    #[test]
    fn descricao_dos_ranks() {
        assert_eq!(describe(&bind(1, 255)), "**\\[G]** (`7`), any rank → <@&9>");
        assert!(describe(&bind(0, 255)).contains("anyone, even outside the group"));
        assert!(describe(&bind(5, 5)).contains("rank 5 →"));
        assert!(describe(&bind(5, 9)).contains("ranks 5–9 →"));
    }

    #[test]
    fn faixa_e_id() {
        assert!(valid_range(0, 255));
        assert!(valid_range(3, 3));
        assert!(!valid_range(4, 3));
        assert!(!valid_range(-1, 3));
        assert!(!valid_range(1, 256));
        assert!(same_id("aB12cd", "AB12CD"));
        assert!(!same_id("ab12cd", "ab12ce"));
    }

    #[test]
    fn grupo_com_subcomandos() {
        let command = bind_command();
        let names: Vec<&str> = command.subcommands.iter().map(|c| &*c.name).collect();
        assert_eq!(names, ["add", "remove", "list"]);
        assert!(command.subcommand_required);
        assert!(command.subcommands.iter().all(|c| {
            c.required_permissions
                .contains(serenity::Permissions::MANAGE_ROLES)
        }));
    }

    fn bind_command() -> scope::Command {
        super::bind()
    }
}
