//! Os comandos de musica (commands/Music.cs). Video do YouTube e BAIXADO com
//! o yt-dlp, tocado e apagado logo depois - nunca transmitido direto.
//! SoundCloud toca por streaming. Link do Spotify vira a mesma musica no
//! SoundCloud ou, se nao bater, no YouTube. Nada de audio fica guardado - a
//! fila e so metadado, em memoria.
//!
//! Todo comando daqui exige servidor: canal de voz so existe la. As regras de
//! quem pode o que moram no `actions`, compartilhadas com os botoes.

use poise::{CreateReply, serenity_prelude as serenity};
use tracing::{error, warn};

use super::{
    actions::{self, MusicReply},
    format, service, sources,
    track::{LoopMode, MusicError, QueuedTrack, TrackSource},
};
use crate::{Context, Error, config::MusicSettings, embeds};

#[derive(Debug, poise::ChoiceParameter)]
pub enum SearchOn {
    #[name = "YouTube"]
    YouTube,
    #[name = "SoundCloud"]
    SoundCloud,
}

#[derive(Debug, poise::ChoiceParameter)]
pub enum LoopChoice {
    #[name = "Off"]
    Off,
    #[name = "Current song"]
    Track,
    #[name = "Whole queue"]
    Queue,
}

/// Quem usou o comando, como membro (com cargos, para as permissoes).
async fn member(ctx: Context<'_>) -> Result<serenity::Member, Error> {
    Ok(ctx
        .author_member()
        .await
        .ok_or("comando de musica fora de servidor")?
        .into_owned())
}

fn guild(ctx: Context<'_>) -> Result<serenity::GuildId, Error> {
    Ok(ctx.guild_id().ok_or("comando de musica fora de servidor")?)
}

async fn respond(ctx: Context<'_>, reply: MusicReply) -> Result<(), Error> {
    // Sem mencao de ninguem: as respostas citam quem agiu (<@id>), e isso e
    // para ler, nao para notificar.
    ctx.send(
        CreateReply::default()
            .embed(reply.embed)
            .ephemeral(reply.ephemeral)
            .allowed_mentions(serenity::CreateAllowedMentions::new()),
    )
    .await?;
    Ok(())
}

/// Play from YouTube, SoundCloud or Spotify — paste a link or type what to search for
#[poise::command(slash_command, guild_only, check = "crate::cooldown::user::<3, 15>")]
pub async fn play(
    ctx: Context<'_>,
    #[description = "A YouTube, SoundCloud or Spotify link (tracks, playlists, albums), or a search"]
    song: String,
    #[description = "Where to search when you type a name instead of a link (default: YouTube)"]
    search_on: Option<SearchOn>,
) -> Result<(), Error> {
    let poise::Context::Application(app) = ctx else {
        return Ok(());
    };
    let guild_id = guild(ctx)?;
    let member = member(ctx).await?;
    let bot_id = ctx.cache().current_user().id;

    // Tudo do cache sai daqui antes do primeiro await.
    let (voice, bot_permissions) = {
        let found = ctx.cache().guild(guild_id).map(|guild| {
            let voice = guild
                .voice_states
                .get(&member.user.id)
                .and_then(|v| v.channel_id)
                .and_then(|c| guild.channels.get(&c).cloned());
            let permissions = voice.as_ref().and_then(|channel| {
                guild
                    .members
                    .get(&bot_id)
                    .map(|me| guild.user_permissions_in(channel, me))
            });
            (voice, permissions)
        });
        found.unwrap_or((None, None))
    };

    // As checagens baratas vem ANTES do defer, para a recusa sair efemera.
    // Depois do defer a resposta ja nasceu publica.
    let Some(voice) = voice else {
        return respond(
            ctx,
            MusicReply::private(embeds::error(
                "Join a voice channel first",
                "Hop into a voice channel and run `/play` again.",
            )),
        )
        .await;
    };

    if voice.kind == serenity::ChannelType::Stage {
        return respond(
            ctx,
            MusicReply::private(embeds::error(
                "Stage channels aren't supported",
                "Use a regular voice channel for music.",
            )),
        )
        .await;
    }

    // O required_bot_permissions olharia o canal de TEXTO onde o comando foi
    // digitado; o que importa aqui e o canal de voz.
    let needed = serenity::Permissions::VIEW_CHANNEL
        | serenity::Permissions::CONNECT
        | serenity::Permissions::SPEAK;
    if !bot_permissions.is_some_and(|p| p.contains(needed)) {
        return respond(
            ctx,
            MusicReply::private(embeds::error(
                "I can't use that channel",
                format!(
                    "I need **View Channel**, **Connect** and **Speak** in <#{}>.",
                    voice.id
                ),
            )),
        )
        .await;
    }

    if let Some(playing) = service::get(guild_id)
        && playing.voice_channel() != voice.id
    {
        return respond(
            ctx,
            MusicReply::private(embeds::error(
                "I'm busy in another channel",
                format!(
                    "I'm already playing in <#{}>. Join me there to add songs.",
                    playing.voice_channel()
                ),
            )),
        )
        .await;
    }

    // Buscar e entrar no canal levam segundos - bem mais que os 3 do Discord
    // para a primeira resposta.
    ctx.defer().await?;
    let interaction = app.interaction;

    if service::check_availability(false).await.is_some() {
        return fail_after_defer(
            ctx,
            interaction,
            "Music is unavailable",
            "Music isn't set up on this bot right now. The bot's admin can see why in the logs.",
        )
        .await;
    }

    let settings = service::settings();
    let source = match search_on {
        Some(SearchOn::SoundCloud) => TrackSource::SoundCloud,
        _ => TrackSource::YouTube,
    };

    let request = match sources::resolve(&ctx.data().http, &song, source, &settings).await {
        Ok(request) => request,
        Err(MusicError::User(message)) => {
            return fail_after_defer(ctx, interaction, "Can't play that", &message).await;
        }
        Err(MusicError::Other(cause)) => {
            error!("[musica] falha ao resolver '{song}' em {guild_id}: {cause}");
            return fail_after_defer(
                ctx,
                interaction,
                "Can't play that",
                "Looking that up took too long or failed. Try again.",
            )
            .await;
        }
    };

    let player = match service::get_or_join(
        &ctx.data().songbird,
        &ctx.serenity_context().http,
        guild_id,
        voice.id,
        ctx.channel_id(),
    )
    .await
    {
        Ok(player) => player,
        Err(MusicError::User(message)) => {
            return fail_after_defer(ctx, interaction, "Can't join", &message).await;
        }
        // Nao tem frase pronta: o on_error responde "Command failed".
        Err(MusicError::Other(cause)) => return Err(cause.into()),
    };

    let name = member.display_name().to_owned();
    let entries: Vec<_> = request
        .tracks
        .iter()
        .map(|t| QueuedTrack::new(t.clone(), member.user.id.get()))
        .collect();
    let (added, position) = player.try_enqueue(&entries, settings.max_queue());
    if added == 0 {
        return fail_after_defer(
            ctx,
            interaction,
            "The queue is full",
            &format!(
                "There are already {} songs waiting. Try again once a few have played.",
                settings.max_queue()
            ),
        )
        .await;
    }

    interaction
        .edit_response(
            ctx.http(),
            serenity::EditInteractionResponse::new()
                .embed(added_embed(&request, added, position, &settings, &name)),
        )
        .await?;
    Ok(())
}

/// O "vai tocar" / "entrou na fila" do /play, para uma faixa ou uma lista.
fn added_embed(
    request: &sources::Request,
    added: usize,
    position: usize,
    settings: &MusicSettings,
    requester: &str,
) -> serenity::CreateEmbed {
    let embed = serenity::CreateEmbed::new().colour(format::COLOR).footer(
        serenity::CreateEmbedFooter::new(format!("Requested by {requester}")),
    );
    let first = &request.tracks[0];
    let thumbnail = format::https_url(first.thumbnail_url.as_deref());

    let Some(collection) = &request.collection_name else {
        // O que acontece ate a musica comecar depende da fonte - e e o que
        // explica os segundos de espera.
        let start_note = match first.source {
            TrackSource::YouTube => "Downloading it now — it'll start in a few seconds.",
            TrackSource::Spotify => {
                "Finding it on SoundCloud or YouTube — it'll start in a few seconds."
            }
            TrackSource::SoundCloud => "Starting the stream now.",
        };

        let mut line = format!(
            "{} `{}`",
            format::link(first),
            format::duration(first.duration)
        );
        if first.source == TrackSource::Spotify
            && let Some(artists) = first.uploader.as_deref().filter(|u| !u.trim().is_empty())
        {
            line.push_str(&format!(" · {}", embeds::safe_trim(artists, 100)));
        }

        let embed = if position == 0 {
            embed
                .title("🎶 Starting up")
                .description(format!("{line}\n{start_note}"))
        } else {
            embed
                .title("➕ Added to the queue")
                .description(format!("{line}\nPosition **#{position}**."))
        };
        return match thumbnail {
            Some(thumb) => embed.thumbnail(thumb),
            None => embed,
        };
    };

    let mut text = format!(
        "From **{}** on {}.\n{}",
        embeds::safe_trim(collection, 150),
        format::source_name(request.source),
        if position == 0 {
            "The first one is starting now.".to_owned()
        } else {
            format!("They start at position **#{position}**.")
        }
    );

    // O que ficou de fora e dito, e por que: limite por link ou fila cheia.
    let left_out = request.tracks.len() - added;
    if request.truncated > 0 {
        text.push_str(&format!(
            "\n-# Only the first {} tracks of a link are added.",
            settings.max_playlist()
        ));
    }
    if left_out > 0 {
        text.push_str(&format!(
            "\n-# {left_out} more didn't fit — the queue holds {} songs.",
            settings.max_queue()
        ));
    }

    let embed = embed
        .title(format!(
            "➕ Added {added} song{}",
            if added == 1 { "" } else { "s" }
        ))
        .description(text);
    match thumbnail {
        Some(thumb) => embed.thumbnail(thumb),
        None => embed,
    }
}

/// Recusa depois do defer publico: apaga o "pensando..." e manda o erro so
/// para quem pediu, em vez de deixar um cartao vermelho no canal.
async fn fail_after_defer(
    ctx: Context<'_>,
    interaction: &serenity::CommandInteraction,
    title: &str,
    message: &str,
) -> Result<(), Error> {
    let http = ctx.http();
    let ephemeral = async {
        interaction.delete_response(http).await?;
        interaction
            .create_followup(
                http,
                serenity::CreateInteractionResponseFollowup::new()
                    .embed(embeds::error(title, message))
                    .ephemeral(true),
            )
            .await?;
        Ok::<_, serenity::Error>(())
    };

    if let Err(e) = ephemeral.await {
        warn!("[musica] aviso: recusa efemera falhou ({e}); editando a publica");
        interaction
            .edit_response(
                http,
                serenity::EditInteractionResponse::new().embed(embeds::error(title, message)),
            )
            .await?;
    }
    Ok(())
}

/// Skip the current song (or vote to skip it)
#[poise::command(slash_command, guild_only, check = "crate::cooldown::user::<3, 10>")]
pub async fn skip(ctx: Context<'_>) -> Result<(), Error> {
    let (guild, member) = (guild(ctx)?, member(ctx).await?);
    let reply = actions::skip(ctx.cache(), &member, guild);
    respond(ctx, reply).await
}

/// Stop the music, clear the queue and leave the voice channel
#[poise::command(slash_command, guild_only, check = "crate::cooldown::user::<3, 10>")]
pub async fn stop(ctx: Context<'_>) -> Result<(), Error> {
    let (guild, member) = (guild(ctx)?, member(ctx).await?);
    let reply = actions::stop(ctx.cache(), &member, guild);
    respond(ctx, reply).await
}

/// Pause the current song
#[poise::command(slash_command, guild_only, check = "crate::cooldown::user::<3, 10>")]
pub async fn pause(ctx: Context<'_>) -> Result<(), Error> {
    let (guild, member) = (guild(ctx)?, member(ctx).await?);
    let reply = actions::set_paused(ctx.cache(), &member, guild, Some(true));
    respond(ctx, reply).await
}

/// Resume the paused song
#[poise::command(slash_command, guild_only, check = "crate::cooldown::user::<3, 10>")]
pub async fn resume(ctx: Context<'_>) -> Result<(), Error> {
    let (guild, member) = (guild(ctx)?, member(ctx).await?);
    let reply = actions::set_paused(ctx.cache(), &member, guild, Some(false));
    respond(ctx, reply).await
}

/// Change the music volume
#[poise::command(slash_command, guild_only, check = "crate::cooldown::user::<3, 10>")]
pub async fn volume(
    ctx: Context<'_>,
    #[description = "From 1 to 150 (default 100)"]
    #[min = 1]
    #[max = 150]
    percent: i64,
) -> Result<(), Error> {
    let (guild, member) = (guild(ctx)?, member(ctx).await?);
    let reply = actions::set_volume(ctx.cache(), &member, guild, percent.clamp(1, 150) as u32);
    respond(ctx, reply).await
}

/// Repeat the current song or the whole queue
#[poise::command(
    slash_command,
    rename = "loop",
    guild_only,
    check = "crate::cooldown::user::<3, 10>"
)]
pub async fn loop_(
    ctx: Context<'_>,
    #[description = "What to repeat"] mode: LoopChoice,
) -> Result<(), Error> {
    let (guild, member) = (guild(ctx)?, member(ctx).await?);
    let mode = match mode {
        LoopChoice::Off => LoopMode::Off,
        LoopChoice::Track => LoopMode::Track,
        LoopChoice::Queue => LoopMode::Queue,
    };
    let reply = actions::set_loop(ctx.cache(), &member, guild, mode);
    respond(ctx, reply).await
}

/// Shuffle the songs waiting in the queue
#[poise::command(slash_command, guild_only, check = "crate::cooldown::user::<3, 10>")]
pub async fn shuffle(ctx: Context<'_>) -> Result<(), Error> {
    let (guild, member) = (guild(ctx)?, member(ctx).await?);
    let reply = actions::shuffle(ctx.cache(), &member, guild);
    respond(ctx, reply).await
}

/// Remove a song from the queue
#[poise::command(slash_command, guild_only, check = "crate::cooldown::user::<3, 10>")]
pub async fn remove(
    ctx: Context<'_>,
    #[description = "Its number in /queue"]
    #[min = 1]
    position: i64,
) -> Result<(), Error> {
    let (guild, member) = (guild(ctx)?, member(ctx).await?);
    let position = usize::try_from(position.max(1)).unwrap_or(usize::MAX);
    let reply = actions::remove(ctx.cache(), &member, guild, position);
    respond(ctx, reply).await
}

/// Show the songs waiting to play
#[poise::command(slash_command, guild_only, check = "crate::cooldown::user::<3, 10>")]
pub async fn queue(
    ctx: Context<'_>,
    #[description = "Which page of the queue"]
    #[min = 1]
    page: Option<i64>,
) -> Result<(), Error> {
    let page = page.unwrap_or(1).clamp(1, 1000) as usize;
    respond(ctx, actions::queue(guild(ctx)?, page)).await
}

/// Show the song that's playing and how far along it is
#[poise::command(slash_command, guild_only, check = "crate::cooldown::user::<3, 10>")]
pub async fn nowplaying(ctx: Context<'_>) -> Result<(), Error> {
    let reply = actions::now_playing(guild(ctx)?).await;
    respond(ctx, reply).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::music::track::sample;

    fn json(embed: serenity::CreateEmbed) -> serde_json::Value {
        serde_json::to_value(embed).unwrap()
    }

    #[test]
    fn aviso_de_faixa_avulsa() {
        let request = sources::Request {
            tracks: vec![sample(TrackSource::YouTube, 225)],
            collection_name: None,
            source: TrackSource::YouTube,
            truncated: 0,
        };
        let settings = MusicSettings::default();

        let now = json(added_embed(&request, 1, 0, &settings, "Ana"));
        assert_eq!(now["title"], "🎶 Starting up");
        assert_eq!(
            now["description"],
            "[Song](https://example.com/x) `3:45`\nDownloading it now — it'll start in a few seconds."
        );
        assert_eq!(now["footer"]["text"], "Requested by Ana");

        let later = json(added_embed(&request, 1, 4, &settings, "Ana"));
        assert_eq!(later["title"], "➕ Added to the queue");
        assert!(
            later["description"]
                .as_str()
                .unwrap()
                .ends_with("Position **#4**.")
        );
    }

    #[test]
    fn aviso_de_lista_cortada() {
        let request = sources::Request {
            tracks: vec![sample(TrackSource::Spotify, 1); 5],
            collection_name: Some("Mix".into()),
            source: TrackSource::Spotify,
            truncated: 7,
        };
        let settings = MusicSettings::default();
        let embed = json(added_embed(&request, 3, 2, &settings, "Ana"));
        assert_eq!(embed["title"], "➕ Added 3 songs");
        assert_eq!(
            embed["description"],
            "From **Mix** on Spotify.\nThey start at position **#2**.\n-# Only the first 50 tracks of a link are added.\n-# 2 more didn't fit — the queue holds 50 songs."
        );
    }

    #[test]
    fn comandos_so_em_servidor_e_com_cooldown() {
        for command in super::super::commands() {
            assert_eq!(
                command.interaction_context.as_deref(),
                Some(&[serenity::InteractionContext::Guild][..]),
                "/{}",
                command.name
            );
            assert!(command.guild_only, "/{}", command.name);
            assert_eq!(command.checks.len(), 1, "/{}", command.name);
        }
    }
}
