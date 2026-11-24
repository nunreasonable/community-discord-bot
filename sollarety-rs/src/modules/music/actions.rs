//! As acoes compartilhadas pelos comandos e pelos botoes do "Now playing"
//! (MusicActions.cs). Um lugar so para as regras de quem pode o que: se o
//! botao e o comando tivessem cada um a sua copia, a primeira mudanca deixaria
//! um deles como atalho para contornar o outro.

use std::{sync::Arc, time::Duration};

use poise::serenity_prelude as serenity;

use super::{
    format,
    player::{GuildPlayer, Removal},
    service,
    track::{LoopMode, QueuedTrack},
};
use crate::embeds;

/// Resposta de uma acao de musica. Recusa e aviso pessoal saem efemeros; o
/// resto, no canal.
pub struct MusicReply {
    pub embed: serenity::CreateEmbed,
    pub ephemeral: bool,
}

impl MusicReply {
    pub fn public(embed: serenity::CreateEmbed) -> Self {
        Self {
            embed,
            ephemeral: false,
        }
    }

    pub fn private(embed: serenity::CreateEmbed) -> Self {
        Self {
            embed,
            ephemeral: true,
        }
    }
}

const CONTROL_RULE: &str = "Only whoever requested the current song, someone with **Move Members**, or someone alone with me can do that.";

/// O player do servidor, se a pessoa estiver no canal dele; senao, a recusa
/// pronta.
fn resolve(
    cache: &serenity::Cache,
    member: &serenity::Member,
    guild: serenity::GuildId,
) -> Result<Arc<GuildPlayer>, Box<MusicReply>> {
    let Some(player) = service::get(guild) else {
        return Err(Box::new(MusicReply::private(embeds::error(
            "Nothing is playing",
            "I'm not in a voice channel right now.",
        ))));
    };

    if !service::is_in_player_channel(cache, member.user.id, &player) {
        return Err(Box::new(MusicReply::private(embeds::error(
            "Join my voice channel",
            format!(
                "You need to be in <#{}> to control the music.",
                player.voice_channel()
            ),
        ))));
    }

    Ok(player)
}

pub fn skip(
    cache: &serenity::Cache,
    member: &serenity::Member,
    guild: serenity::GuildId,
) -> MusicReply {
    let player = match resolve(cache, member, guild) {
        Ok(player) => player,
        Err(refusal) => return *refusal,
    };

    let Some(current) = player.current() else {
        return MusicReply::private(embeds::error(
            "Nothing to skip",
            "No song is playing right now.",
        ));
    };
    let title = format::link(&current.display());

    if service::can_control(cache, member, &player) {
        player.skip();
        return MusicReply::public(embeds::info(
            "⏭️ Skipped",
            format!("<@{}> skipped {title}.", member.user.id),
        ));
    }

    let listeners: Vec<u64> = service::player_listeners(cache, &player)
        .into_iter()
        .map(serenity::UserId::get)
        .collect();
    let vote = player.vote_skip(member.user.id.get(), &listeners);

    if vote.skipped {
        return MusicReply::public(embeds::info(
            "⏭️ Vote passed",
            format!(
                "{}/{} listeners voted, so {title} was skipped.",
                vote.votes, vote.needed
            ),
        ));
    }

    if vote.duplicate {
        return MusicReply::private(embeds::info(
            "Already voted",
            format!(
                "You already voted to skip. **{}/{}** votes so far.",
                vote.votes, vote.needed
            ),
        ));
    }

    MusicReply::public(embeds::info(
        "🗳️ Vote to skip",
        format!(
            "<@{}> wants to skip {title}.\n**{}/{}** votes — use `/skip` or the Skip button to vote.",
            member.user.id, vote.votes, vote.needed
        ),
    ))
}

pub fn stop(
    cache: &serenity::Cache,
    member: &serenity::Member,
    guild: serenity::GuildId,
) -> MusicReply {
    let player = match resolve(cache, member, guild) {
        Ok(player) => player,
        Err(refusal) => return *refusal,
    };

    if !service::can_control(cache, member, &player) {
        return MusicReply::private(embeds::error(
            "Can't stop the music",
            format!("{CONTROL_RULE} Use `/skip` to vote to skip instead."),
        ));
    }

    player.clear_queue();
    player.stop(&format!("/stop por {}", member.user.id));
    MusicReply::public(embeds::info(
        "⏹️ Stopped",
        format!(
            "<@{}> stopped the music. I cleared the queue and left the voice channel.",
            member.user.id
        ),
    ))
}

/// `paused`: Some(true) pausa, Some(false) retoma, None alterna (o botao).
pub fn set_paused(
    cache: &serenity::Cache,
    member: &serenity::Member,
    guild: serenity::GuildId,
    paused: Option<bool>,
) -> MusicReply {
    let player = match resolve(cache, member, guild) {
        Ok(player) => player,
        Err(refusal) => return *refusal,
    };

    if player.current().is_none() {
        return MusicReply::private(embeds::error(
            "Nothing is playing",
            "There's no song to pause or resume.",
        ));
    }

    if !service::can_control(cache, member, &player) {
        return MusicReply::private(embeds::error("Can't do that", CONTROL_RULE));
    }

    let is_paused = player.is_paused();
    let target = paused.unwrap_or(!is_paused);
    if target == is_paused {
        return MusicReply::private(if target {
            embeds::info("Already paused", "Use `/resume` to continue.")
        } else {
            embeds::info("Not paused", "The music is already playing.")
        });
    }

    if target {
        player.pause();
    } else {
        player.resume();
    }

    MusicReply::public(embeds::info(
        if target {
            "⏸️ Paused"
        } else {
            "▶️ Resumed"
        },
        format!(
            "<@{}> {} the music.",
            member.user.id,
            if target { "paused" } else { "resumed" }
        ),
    ))
}

pub fn set_volume(
    cache: &serenity::Cache,
    member: &serenity::Member,
    guild: serenity::GuildId,
    percent: u32,
) -> MusicReply {
    let player = match resolve(cache, member, guild) {
        Ok(player) => player,
        Err(refusal) => return *refusal,
    };

    if !service::can_control(cache, member, &player) {
        return MusicReply::private(embeds::error("Can't change the volume", CONTROL_RULE));
    }

    player.set_volume(percent);
    MusicReply::public(embeds::info(
        "🔊 Volume",
        format!("<@{}> set the volume to **{percent}%**.", member.user.id),
    ))
}

pub fn set_loop(
    cache: &serenity::Cache,
    member: &serenity::Member,
    guild: serenity::GuildId,
    mode: LoopMode,
) -> MusicReply {
    let player = match resolve(cache, member, guild) {
        Ok(player) => player,
        Err(refusal) => return *refusal,
    };

    if !service::can_control(cache, member, &player) {
        return MusicReply::private(embeds::error("Can't change the loop", CONTROL_RULE));
    }

    player.set_loop(mode);
    let text = match mode {
        LoopMode::Track => "🔂 Looping the current song.",
        LoopMode::Queue => "🔁 Looping the whole queue.",
        LoopMode::Off => "➡️ Loop is off.",
    };

    MusicReply::public(embeds::info(
        "Loop",
        format!("{text} (set by <@{}>)", member.user.id),
    ))
}

pub fn shuffle(
    cache: &serenity::Cache,
    member: &serenity::Member,
    guild: serenity::GuildId,
) -> MusicReply {
    let player = match resolve(cache, member, guild) {
        Ok(player) => player,
        Err(refusal) => return *refusal,
    };

    if !service::can_control(cache, member, &player) {
        return MusicReply::private(embeds::error("Can't shuffle", CONTROL_RULE));
    }

    let count = player.shuffle();
    if count < 2 {
        MusicReply::private(embeds::info(
            "Nothing to shuffle",
            "The queue needs at least two songs.",
        ))
    } else {
        MusicReply::public(embeds::info(
            "🔀 Shuffled",
            format!("<@{}> shuffled {count} songs.", member.user.id),
        ))
    }
}

pub fn remove(
    cache: &serenity::Cache,
    member: &serenity::Member,
    guild: serenity::GuildId,
    position: usize,
) -> MusicReply {
    let player = match resolve(cache, member, guild) {
        Ok(player) => player,
        Err(refusal) => return *refusal,
    };

    // A propria faixa sai sem pedir licenca; a dos outros, so com Move
    // Members. "Pediu a faixa atual" nao da direito a mexer no resto.
    let can_moderate =
        service::permissions_in(cache, guild, player.voice_channel(), member).move_members();
    let user = member.user.id.get();

    match player.remove_at(position, |e| e.requester_id == user || can_moderate) {
        Removal::Denied => MusicReply::private(embeds::error(
            "Not your song",
            "You can only remove songs you added, unless you have **Move Members**.",
        )),
        Removal::Missing => MusicReply::private(embeds::error(
            "No such position",
            format!(
                "The queue has {} song(s). Check `/queue` for the numbers.",
                player.queue_count()
            ),
        )),
        Removal::Removed(removed) => MusicReply::public(embeds::info(
            "🗑️ Removed",
            format!(
                "<@{user}> removed {} from the queue.",
                format::link(&removed.display())
            ),
        )),
    }
}

const PER_PAGE: usize = 10;

/// O texto do /queue: descricao, rodape e a pagina de fato mostrada.
fn render_queue(
    current: Option<&QueuedTrack>,
    queue: &[Arc<QueuedTrack>],
    page: usize,
    loop_mode: LoopMode,
) -> (String, String) {
    let pages = queue.len().div_ceil(PER_PAGE).max(1);
    let page = page.clamp(1, pages);

    let mut text = String::new();
    if let Some(current) = current {
        text.push_str(&format!(
            "**Now:** {} `{}`\n\n",
            format::link(&current.display()),
            format::duration(current.playing_duration())
        ));
    }

    if queue.is_empty() {
        text.push_str("*The queue is empty.* Add songs with `/play`.");
    }

    for (index, entry) in queue
        .iter()
        .enumerate()
        .skip((page - 1) * PER_PAGE)
        .take(PER_PAGE)
    {
        text.push_str(&format!(
            "`{}.` {} `{}` · <@{}>\n",
            index + 1,
            format::link(&entry.display()),
            format::duration(entry.playing_duration()),
            entry.requester_id
        ));
    }

    let total: Duration = queue.iter().map(|e| e.playing_duration()).sum();
    let mut footer = format!(
        "Page {page}/{pages} · {} song(s) · {} total",
        queue.len(),
        format::duration(total)
    );
    match loop_mode {
        LoopMode::Track => footer.push_str(" · 🔂 track loop"),
        LoopMode::Queue => footer.push_str(" · 🔁 queue loop"),
        LoopMode::Off => {}
    }
    (text, footer)
}

pub fn queue(guild: serenity::GuildId, page: usize) -> MusicReply {
    let Some(player) = service::get(guild) else {
        return MusicReply::private(embeds::info(
            "Queue",
            "Nothing is playing and the queue is empty.",
        ));
    };

    let current = player.current();
    let (text, footer) = render_queue(
        current.as_deref(),
        &player.queue_snapshot(),
        page,
        player.loop_mode(),
    );

    MusicReply::public(
        serenity::CreateEmbed::new()
            .title("📜 Queue")
            .description(embeds::trim(&text, 4000))
            .footer(serenity::CreateEmbedFooter::new(footer))
            .colour(format::COLOR),
    )
}

pub async fn now_playing(guild: serenity::GuildId) -> MusicReply {
    let Some((player, current)) = service::get(guild).and_then(|p| {
        let current = p.current()?;
        Some((p, current))
    }) else {
        return MusicReply::private(embeds::info(
            "Nothing is playing",
            "Use `/play` to start some music.",
        ));
    };

    let length = current.playing_duration();
    let position = player.position().await.min(length);
    let display = current.display();

    let mut embed = serenity::CreateEmbed::new()
        .author(serenity::CreateEmbedAuthor::new(if player.is_paused() {
            "Paused"
        } else {
            "Now playing"
        }))
        .description(format!(
            "**{}**\n\n{}\n`{} / {}`",
            format::link(&display),
            format::progress_bar(position, length),
            format::duration(position),
            format::duration(length)
        ))
        .field("Requested by", format!("<@{}>", current.requester_id), true)
        .field("Volume", format!("{}%", player.volume()), true)
        .field("Up next", player.queue_count().to_string(), true)
        .field("Source", format::source_line(&current), true)
        .colour(format::COLOR);

    let thumbnail = display
        .thumbnail_url
        .clone()
        .or_else(|| current.playable().and_then(|p| p.thumbnail_url));
    if let Some(thumb) = format::https_url(thumbnail.as_deref()) {
        embed = embed.thumbnail(thumb);
    }

    MusicReply::public(embed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::music::track::{TrackSource, sample};

    fn entries(n: u64) -> Vec<Arc<QueuedTrack>> {
        (0..n)
            .map(|i| QueuedTrack::new(sample(TrackSource::YouTube, 60), i))
            .collect()
    }

    #[test]
    fn fila_paginada() {
        let queue = entries(23);
        let (text, footer) = render_queue(None, &queue, 3, LoopMode::Off);
        assert_eq!(text.lines().count(), 3);
        assert!(text.starts_with("`21.` [Song](https://example.com/x) `1:00` · <@20>"));
        assert_eq!(footer, "Page 3/3 · 23 song(s) · 23:00 total");

        // Pagina alem do fim vira a ultima.
        let (_, footer) = render_queue(None, &queue, 99, LoopMode::Queue);
        assert_eq!(
            footer,
            "Page 3/3 · 23 song(s) · 23:00 total · 🔁 queue loop"
        );
    }

    #[test]
    fn fila_vazia_com_faixa_atual() {
        let current = &entries(1)[0];
        let (text, footer) = render_queue(Some(current), &[], 1, LoopMode::Track);
        assert_eq!(
            text,
            "**Now:** [Song](https://example.com/x) `1:00`\n\n*The queue is empty.* Add songs with `/play`."
        );
        assert_eq!(footer, "Page 1/1 · 0 song(s) · ?:?? total · 🔂 track loop");
    }
}
