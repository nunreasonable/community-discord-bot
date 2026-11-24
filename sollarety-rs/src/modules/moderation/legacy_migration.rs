//! Traz para a configuracao por servidor as chaves que viviam no
//! config.jsonc (Services/LegacyConfigMigration.cs). Roda uma vez na vida da
//! instalacao.
//!
//! O que torna isto menos trivial do que copiar campos: as chaves antigas
//! eram ids SOLTOS, sem servidor. O par do vigia dizia a que servidor
//! pertencia; o canal de log de moderacao e os tres de ticket nao diziam
//! nada - eram globais porque o bot fingia viver num servidor so. Entao cada
//! id e resolvido no cache para descobrir de qual servidor ele e, e o que nao
//! resolver vira uma linha de log em vez de sumir calado.
//!
//! Roda no CacheReady (cache quente) e e marcada no proprio arquivo do store,
//! nao numa flag de processo: migrar uma vez por reinicializacao desfaria uma
//! limpeza que o dono tivesse feito pelo /config.

use poise::serenity_prelude as serenity;
use tracing::{error, info, warn};

use crate::{
    Data, Error, config,
    store::guild_settings::{GuildSettings, GuildSettingsEdit},
};

pub async fn run(ctx: &serenity::Context, data: &Data) {
    if let Err(cause) = migrate(ctx, data).await {
        error!("[config] falha na migracao do config.jsonc: {cause}");
    }
}

/// Os valores antigos ja resolvidos para (servidor, valor). Separado da
/// escrita para a decisao poder ser testada sem Discord.
#[derive(Default)]
struct Legacy {
    watcher_guild: Option<u64>,
    watcher_channel: Option<u64>,
    mod_log: Resolved,
    ticket_category: Resolved,
    ticket_staff_role: Resolved,
    ticket_log: Resolved,
}

/// (servidor a que o id pertence, se deu para saber; o id do config).
type Resolved = (Option<u64>, Option<u64>);

/// Uma chave solta: onde ela esta, como grava-la e o nome no log.
type Move<'a> = (&'a Resolved, fn(&mut GuildSettings, u64), &'static str);

async fn migrate(ctx: &serenity::Context, data: &Data) -> Result<(), Error> {
    let already = data
        .guild_settings
        .update(|edit| edit.file.migrated_from_config)
        .await?;
    if already {
        return Ok(());
    }

    let config = config::current();
    let positive = |v: Option<u64>| v.filter(|&id| id > 0);

    let legacy = Legacy {
        // O par do vigia ja carrega o servidor.
        watcher_guild: positive(config.auto_softban_guild_id),
        watcher_channel: positive(config.auto_softban_channel_id),
        mod_log: (
            resolve_channel_guild(ctx, config.moderation_log_channel_id).await,
            config.moderation_log_channel_id,
        ),
        ticket_category: (
            resolve_channel_guild(ctx, config.ticket_category_id).await,
            config.ticket_category_id,
        ),
        ticket_staff_role: (
            resolve_role_guild(ctx, config.ticket_staff_role_id),
            config.ticket_staff_role_id,
        ),
        ticket_log: (
            resolve_channel_guild(ctx, config.ticket_log_channel_id).await,
            config.ticket_log_channel_id,
        ),
    };

    let (applied, pending) = data
        .guild_settings
        .update(|edit| {
            let result = apply(edit, &legacy);
            // Marcada mesmo quando nada foi movido: nao havia nada para mover,
            // e insistir a cada reconexao so gastaria I/O.
            edit.file.migrated_from_config = true;
            edit.mark_changed();
            result
        })
        .await?;

    if applied > 0 {
        info!(
            "[config] migracao: {applied} configuracao(oes) do config.jsonc movida(s) para a configuracao por servidor."
        );
    } else {
        info!("[config] migracao: nada a mover do config.jsonc.");
    }

    for item in pending {
        warn!(
            "[config] AVISO: nao consegui migrar {item}. Reconfigure com /config no servidor certo."
        );
    }
    Ok(())
}

/// Aplica no arquivo e devolve (quantas chaves foram movidas, o que ficou
/// pendente).
fn apply(edit: &mut GuildSettingsEdit, legacy: &Legacy) -> (usize, Vec<String>) {
    let mut touched = 0;
    let mut pending = Vec::new();

    match (legacy.watcher_guild, legacy.watcher_channel) {
        (Some(guild), Some(channel)) => {
            edit.get(guild).auto_softban_channel_id = Some(channel);
            touched += 1;
        }
        (None, None) => {}
        _ => pending.push("autoSoftban (o par servidor+canal estava pela metade)".to_owned()),
    }

    let moves: [Move<'_>; 4] = [
        (
            &legacy.mod_log,
            |s, v| s.moderation_log_channel_id = Some(v),
            "moderationLogChannelId",
        ),
        (
            &legacy.ticket_category,
            |s, v| s.ticket_category_id = Some(v),
            "ticketCategoryId",
        ),
        (
            &legacy.ticket_staff_role,
            |s, v| s.ticket_staff_role_id = Some(v),
            "ticketStaffRoleId",
        ),
        (
            &legacy.ticket_log,
            |s, v| s.ticket_log_channel_id = Some(v),
            "ticketLogChannelId",
        ),
    ];

    for (&(guild, value), assign, name) in moves {
        let Some(value) = value.filter(|&v| v > 0) else {
            continue;
        };
        match guild {
            Some(guild) => {
                assign(edit.get(guild), value);
                touched += 1;
            }
            None => pending.push(format!("{name} ({value})")),
        }
    }

    (touched, pending)
}

/// De qual servidor e este canal. None quando nao da para saber.
async fn resolve_channel_guild(ctx: &serenity::Context, channel_id: Option<u64>) -> Option<u64> {
    let channel_id = serenity::ChannelId::new(channel_id.filter(|&id| id > 0)?);

    // Cache primeiro: nao vale gastar uma chamada REST por chave.
    for guild_id in ctx.cache.guilds() {
        let found = ctx.cache.guild(guild_id).is_some_and(|g| {
            g.channels.contains_key(&channel_id) || g.threads.iter().any(|t| t.id == channel_id)
        });
        if found {
            return Some(guild_id.get());
        }
    }

    match channel_id.to_channel(ctx).await {
        Ok(channel) => channel.guild().map(|c| c.guild_id.get()),
        Err(_) => None,
    }
}

fn resolve_role_guild(ctx: &serenity::Context, role_id: Option<u64>) -> Option<u64> {
    let role_id = serenity::RoleId::new(role_id.filter(|&id| id > 0)?);
    ctx.cache
        .guilds()
        .into_iter()
        .find(|&g| {
            ctx.cache
                .guild(g)
                .is_some_and(|g| g.roles.contains_key(&role_id))
        })
        .map(|g| g.get())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::GuildSettingsStore;

    #[tokio::test]
    async fn move_o_que_resolveu_e_anota_o_resto() {
        let dir = crate::store::test_support::scratch_dir("migracao");
        let store = GuildSettingsStore::open(dir.join("guild-settings.json"))
            .await
            .unwrap();

        let legacy = Legacy {
            watcher_guild: Some(1),
            watcher_channel: Some(10),
            mod_log: (Some(1), Some(11)),
            ticket_category: (None, Some(12)),
            ticket_staff_role: (Some(2), Some(13)),
            ticket_log: (None, None),
        };
        let (touched, pending) = store
            .update(|edit| {
                let result = apply(edit, &legacy);
                edit.mark_changed();
                result
            })
            .await
            .unwrap();

        assert_eq!(touched, 3);
        assert_eq!(pending, ["ticketCategoryId (12)"]);
        let one = store.for_guild(1).unwrap();
        assert_eq!(one.auto_softban_channel_id, Some(10));
        assert_eq!(one.moderation_log_channel_id, Some(11));
        assert_eq!(store.for_guild(2).unwrap().ticket_staff_role_id, Some(13));

        let half = Legacy {
            watcher_channel: Some(10),
            ..Legacy::default()
        };
        let (touched, pending) = store.update(|edit| apply(edit, &half)).await.unwrap();
        assert_eq!(touched, 0);
        assert_eq!(pending.len(), 1);
        assert!(pending[0].starts_with("autoSoftban"));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
