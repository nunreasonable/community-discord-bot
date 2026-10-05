using System;
using System.Linq;
using DisCatSharp.Entities;
using DisCatSharp.Enums;

namespace CommunityBot.Services.Music
{
    /// <summary>Como a musica aparece no Discord: duracao, barra, embeds e botoes.</summary>
    internal static class MusicFormat
    {
        public const string Prefix = "music:";
        public const string PauseId = Prefix + "pause";
        public const string SkipId = Prefix + "skip";
        public const string StopId = Prefix + "stop";
        public const string QueueId = Prefix + "queue";

        public static readonly DiscordColor Color = new(0xE0457B);

        /// <summary>"3:45", "1:02:03"; "?:??" para faixa de set ainda nao consultada.</summary>
        public static string Duration(TimeSpan t) =>
            t == TimeSpan.Zero ? "?:??"
            : t.TotalHours >= 1 ? $"{(int)t.TotalHours}:{t.Minutes:00}:{t.Seconds:00}" : $"{t.Minutes}:{t.Seconds:00}";

        public static string SourceName(TrackSource source) => source switch
        {
            TrackSource.SoundCloud => "SoundCloud",
            TrackSource.Spotify => "Spotify",
            _ => "YouTube"
        };

        /// <summary>
        /// De onde a faixa veio e de onde ela toca: "YouTube", "SoundCloud" ou,
        /// para o Spotify, "Spotify → SoundCloud" com link para a faixa que de
        /// fato toca - quem ouvir algo diferente do esperado ve na hora por que.
        /// </summary>
        public static string SourceLine(QueuedTrack entry) =>
            entry.Track.Source == TrackSource.Spotify && entry.Playable is { } playable
                ? $"Spotify → [{SourceName(playable.Source)}]({playable.WebpageUrl})"
                : SourceName(entry.Track.Source);

        public static string ProgressBar(TimeSpan position, TimeSpan total, int width = 16)
        {
            var ratio = total.TotalSeconds <= 0 ? 0 : Math.Clamp(position.TotalSeconds / total.TotalSeconds, 0, 1);
            var knob = (int)Math.Round(ratio * (width - 1));
            return string.Concat(Enumerable.Range(0, width).Select(i => i == knob ? "🔘" : "▬"));
        }

        /// <summary>
        /// Titulo com link para o video. O link mascarado e seguro aqui: a URL e
        /// a canonica devolvida pelo yt-dlp, e o titulo passa pelo Safe, que
        /// impede o video de montar um rotulo proprio.
        /// </summary>
        public static string Link(TrackInfo track) =>
            $"[{Embeds.SafeTrim(track.Title, 200).Replace("]", "\\]")}]({track.WebpageUrl})";

        public static DiscordEmbed NowPlaying(QueuedTrack entry, LoopMode loop, int upcoming)
        {
            var track = entry.Display;
            var playing = entry.Playable ?? track;
            var embed = new DiscordEmbedBuilder()
                .WithAuthor("Now playing")
                .WithDescription($"**{Link(track)}**")
                .AddField(new DiscordEmbedField("Duration", Duration(playing.Duration), true))
                .AddField(new DiscordEmbedField("Requested by", $"<@{entry.RequesterId}>", true))
                .AddField(new DiscordEmbedField("Source", SourceLine(entry), true))
                .WithColor(Color);

            if (!string.IsNullOrWhiteSpace(track.Uploader))
                embed.AddField(new DiscordEmbedField(track.Source == TrackSource.Spotify ? "Artist" : "By",
                    Embeds.SafeTrim(track.Uploader, 100), true));

            var thumbnail = track.ThumbnailUrl ?? playing.ThumbnailUrl;
            if (Uri.TryCreate(thumbnail, UriKind.Absolute, out var thumb) && thumb.Scheme == Uri.UriSchemeHttps)
                embed.WithThumbnail(thumb.AbsoluteUri);

            var footer = upcoming == 0 ? "Nothing else in the queue" : $"{upcoming} more in the queue";
            if (loop != LoopMode.Off)
                footer += loop == LoopMode.Track ? " · 🔂 looping this track" : " · 🔁 looping the queue";

            return embed.WithFooter(footer).Build();
        }

        public static DiscordComponent[] Controls(bool enabled = true) => new DiscordComponent[]
        {
            new DiscordButtonComponent(ButtonStyle.Secondary, PauseId, "Pause/Resume", !enabled, new DiscordComponentEmoji("⏯️")),
            new DiscordButtonComponent(ButtonStyle.Primary, SkipId, "Skip", !enabled, new DiscordComponentEmoji("⏭️")),
            new DiscordButtonComponent(ButtonStyle.Danger, StopId, "Stop", !enabled, new DiscordComponentEmoji("⏹️")),
            new DiscordButtonComponent(ButtonStyle.Secondary, QueueId, "Queue", !enabled, new DiscordComponentEmoji("📜"))
        };
    }
}
