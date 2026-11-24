//! Como a musica aparece no Discord: duracao, barra, embeds e botoes
//! (MusicFormat.cs).

use std::time::Duration;

use poise::serenity_prelude as serenity;

use super::track::{LoopMode, QueuedTrack, TrackInfo, TrackSource};
use crate::embeds;

pub const PREFIX: &str = "music:";
pub const PAUSE_ID: &str = "music:pause";
pub const SKIP_ID: &str = "music:skip";
pub const STOP_ID: &str = "music:stop";
pub const QUEUE_ID: &str = "music:queue";

pub const COLOR: serenity::Colour = serenity::Colour::new(0xE0457B);

/// "3:45", "1:02:03"; "?:??" para faixa de set ainda nao consultada.
pub fn duration(t: Duration) -> String {
    if t.is_zero() {
        return "?:??".into();
    }
    let total = t.as_secs();
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours >= 1 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

pub fn source_name(source: TrackSource) -> &'static str {
    match source {
        TrackSource::SoundCloud => "SoundCloud",
        TrackSource::Spotify => "Spotify",
        TrackSource::YouTube => "YouTube",
    }
}

/// De onde a faixa veio e de onde ela toca: "YouTube", "SoundCloud" ou, para
/// o Spotify, "Spotify → SoundCloud" com link para a faixa que de fato toca -
/// quem ouvir algo diferente do esperado ve na hora por que.
pub fn source_line(entry: &QueuedTrack) -> String {
    match (entry.track.source, entry.playable()) {
        (TrackSource::Spotify, Some(playable)) => format!(
            "Spotify → [{}]({})",
            source_name(playable.source),
            playable.webpage_url
        ),
        (source, _) => source_name(source).into(),
    }
}

pub fn progress_bar(position: Duration, total: Duration) -> String {
    const WIDTH: usize = 16;
    let ratio = if total.as_secs_f64() <= 0.0 {
        0.0
    } else {
        (position.as_secs_f64() / total.as_secs_f64()).clamp(0.0, 1.0)
    };
    // Math.Round do C# arredonda o meio para o par; o round do Rust nao.
    let knob = (ratio * (WIDTH - 1) as f64).round_ties_even() as usize;
    (0..WIDTH)
        .map(|i| if i == knob { "🔘" } else { "▬" })
        .collect()
}

/// Titulo com link para o video. O link mascarado e seguro aqui: a URL e a
/// canonica devolvida pelo yt-dlp (ou montada com um id validado do
/// Spotify), e o titulo passa pelo safe, que impede o video de montar um
/// rotulo proprio.
pub fn link(track: &TrackInfo) -> String {
    format!(
        "[{}]({})",
        embeds::safe_trim(&track.title, 200).replace(']', "\\]"),
        track.webpage_url
    )
}

/// Capa so por https: o Discord buscaria qualquer endereco que o yt-dlp ou o
/// Spotify devolvessem.
pub fn https_url(url: Option<&str>) -> Option<String> {
    let parsed = url::Url::parse(url?).ok()?;
    (parsed.scheme() == "https").then(|| parsed.to_string())
}

pub fn now_playing(
    entry: &QueuedTrack,
    loop_mode: LoopMode,
    upcoming: usize,
) -> serenity::CreateEmbed {
    let track = entry.display();
    let playing = entry.playable().unwrap_or_else(|| track.clone());
    let mut embed = serenity::CreateEmbed::new()
        .author(serenity::CreateEmbedAuthor::new("Now playing"))
        .description(format!("**{}**", link(&track)))
        .field("Duration", duration(playing.duration), true)
        .field("Requested by", format!("<@{}>", entry.requester_id), true)
        .field("Source", source_line(entry), true)
        .colour(COLOR);

    if let Some(uploader) = track.uploader.as_deref().filter(|u| !u.trim().is_empty()) {
        let name = if track.source == TrackSource::Spotify {
            "Artist"
        } else {
            "By"
        };
        embed = embed.field(name, embeds::safe_trim(uploader, 100), true);
    }

    let thumbnail = track
        .thumbnail_url
        .as_deref()
        .or(playing.thumbnail_url.as_deref());
    if let Some(thumb) = https_url(thumbnail) {
        embed = embed.thumbnail(thumb);
    }

    let mut footer = if upcoming == 0 {
        "Nothing else in the queue".to_owned()
    } else {
        format!("{upcoming} more in the queue")
    };
    match loop_mode {
        LoopMode::Track => footer.push_str(" · 🔂 looping this track"),
        LoopMode::Queue => footer.push_str(" · 🔁 looping the queue"),
        LoopMode::Off => {}
    }

    embed.footer(serenity::CreateEmbedFooter::new(footer))
}

/// Os quatro botoes do aviso, numa linha so.
pub fn controls() -> Vec<serenity::CreateActionRow> {
    let button = |id: &str, style, label: &str, emoji: &str| {
        serenity::CreateButton::new(id)
            .style(style)
            .label(label)
            .emoji(serenity::ReactionType::Unicode(emoji.into()))
    };
    vec![serenity::CreateActionRow::Buttons(vec![
        button(
            PAUSE_ID,
            serenity::ButtonStyle::Secondary,
            "Pause/Resume",
            "⏯️",
        ),
        button(SKIP_ID, serenity::ButtonStyle::Primary, "Skip", "⏭️"),
        button(STOP_ID, serenity::ButtonStyle::Danger, "Stop", "⏹️"),
        button(QUEUE_ID, serenity::ButtonStyle::Secondary, "Queue", "📜"),
    ])]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::music::track::sample;

    #[test]
    fn duracao_como_o_csharp() {
        assert_eq!(duration(Duration::ZERO), "?:??");
        assert_eq!(duration(Duration::from_millis(300)), "0:00");
        assert_eq!(duration(Duration::from_secs(225)), "3:45");
        assert_eq!(duration(Duration::from_secs_f64(212.9)), "3:32");
        assert_eq!(duration(Duration::from_secs(3723)), "1:02:03");
        assert_eq!(duration(Duration::from_secs(3600)), "1:00:00");
        assert_eq!(duration(Duration::from_secs(36000)), "10:00:00");
    }

    #[test]
    fn barra_de_progresso() {
        let total = Duration::from_secs(100);
        let bar = progress_bar(Duration::ZERO, total);
        assert!(bar.starts_with('🔘'));
        assert_eq!(bar.chars().count(), 16);
        assert!(progress_bar(total, total).ends_with('🔘'));
        assert!(progress_bar(Duration::from_secs(500), total).ends_with('🔘'));
        assert!(progress_bar(Duration::from_secs(5), Duration::ZERO).starts_with('🔘'));
        // 0.5 * 15 = 7.5: arredonda para o par (8), como o Math.Round.
        assert_eq!(
            progress_bar(Duration::from_secs(50), total)
                .chars()
                .position(|c| c == '🔘'),
            Some(8)
        );
    }

    #[test]
    fn link_escapa_o_titulo() {
        let mut track = sample(TrackSource::YouTube, 1);
        track.title = "a] [b](https://evil)".into();
        assert_eq!(
            link(&track),
            "[a\\] \\[b\\](https://evil)](https://example.com/x)"
        );
    }

    #[test]
    fn capa_so_https() {
        assert_eq!(
            https_url(Some("https://i.ytimg.com/a.jpg")).as_deref(),
            Some("https://i.ytimg.com/a.jpg")
        );
        assert_eq!(https_url(Some("http://i.ytimg.com/a.jpg")), None);
        assert_eq!(https_url(Some("nada")), None);
        assert_eq!(https_url(None), None);
    }

    #[test]
    fn linha_de_origem_do_spotify() {
        let entry = QueuedTrack::new(sample(TrackSource::Spotify, 200), 1);
        assert_eq!(source_line(&entry), "Spotify");
        let mut found = sample(TrackSource::SoundCloud, 200);
        found.webpage_url = "https://soundcloud.com/a/b".into();
        entry.set_playable(found);
        assert_eq!(
            source_line(&entry),
            "Spotify → [SoundCloud](https://soundcloud.com/a/b)"
        );
    }
}
