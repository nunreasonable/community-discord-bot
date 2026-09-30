using System;
using System.Linq;
using System.Text;
using System.Threading.Tasks;
using DisCatSharp.Entities;

namespace CommunityBot.Services.Music
{
    /// <summary>Resposta de uma acao de musica. Recusa e aviso pessoal saem efemeros; o resto, no canal.</summary>
    internal readonly record struct MusicReply(DiscordEmbed Embed, bool Ephemeral)
    {
        public static MusicReply Public(DiscordEmbed embed) => new(embed, false);
        public static MusicReply Private(DiscordEmbed embed) => new(embed, true);
    }

    /// <summary>
    /// As acoes compartilhadas pelos comandos e pelos botoes do "Now playing".
    /// Um lugar so para as regras de quem pode o que: se o botao e o comando
    /// tivessem cada um a sua copia, a primeira mudanca deixaria um deles como
    /// atalho para contornar o outro.
    /// </summary>
    internal static class MusicActions
    {
        private const string ControlRule =
            "Only whoever requested the current song, someone with **Move Members**, or someone alone with me can do that.";

        /// <summary>O player do servidor, se a pessoa estiver no canal dele; senao, a recusa pronta.</summary>
        public static GuildPlayer? Resolve(DiscordMember member, ulong guildId, out MusicReply refusal)
        {
            refusal = default;
            var player = MusicService.Get(guildId);

            if (player is null)
            {
                refusal = MusicReply.Private(Embeds.Error("Nothing is playing", "I'm not in a voice channel right now."));
                return null;
            }

            if (!MusicService.IsInPlayerChannel(member, player))
            {
                refusal = MusicReply.Private(Embeds.Error("Join my voice channel",
                    $"You need to be in {player.VoiceChannel?.Mention ?? "my voice channel"} to control the music."));
                return null;
            }

            return player;
        }

        public static MusicReply Skip(DiscordMember member, ulong guildId)
        {
            if (Resolve(member, guildId, out var refusal) is not { } player)
                return refusal;

            if (player.Current is not { } current)
                return MusicReply.Private(Embeds.Error("Nothing to skip", "No song is playing right now."));

            if (MusicService.CanControl(member, player))
            {
                player.Skip();
                return MusicReply.Public(Embeds.Info("⏭️ Skipped",
                    $"{member.Mention} skipped {MusicFormat.Link(current.Track)}."));
            }

            var listeners = MusicService.Listeners(player).Select(m => m.Id).ToList();
            var vote = player.VoteSkip(member.Id, listeners);

            if (vote.Skipped)
                return MusicReply.Public(Embeds.Info("⏭️ Vote passed",
                    $"{vote.Votes}/{vote.Needed} listeners voted, so {MusicFormat.Link(current.Track)} was skipped."));

            if (vote.Duplicate)
                return MusicReply.Private(Embeds.Info("Already voted",
                    $"You already voted to skip. **{vote.Votes}/{vote.Needed}** votes so far."));

            return MusicReply.Public(Embeds.Info("🗳️ Vote to skip",
                $"{member.Mention} wants to skip {MusicFormat.Link(current.Track)}.\n" +
                $"**{vote.Votes}/{vote.Needed}** votes — use `/skip` or the Skip button to vote."));
        }

        public static MusicReply Stop(DiscordMember member, ulong guildId)
        {
            if (Resolve(member, guildId, out var refusal) is not { } player)
                return refusal;

            if (!MusicService.CanControl(member, player))
                return MusicReply.Private(Embeds.Error("Can't stop the music",
                    ControlRule + " Use `/skip` to vote to skip instead."));

            player.ClearQueue();
            player.Stop($"/stop por {member.Id}");
            return MusicReply.Public(Embeds.Info("⏹️ Stopped",
                $"{member.Mention} stopped the music. I cleared the queue and left the voice channel."));
        }

        public static async Task<MusicReply> SetPausedAsync(DiscordMember member, ulong guildId, bool? paused)
        {
            if (Resolve(member, guildId, out var refusal) is not { } player)
                return refusal;

            if (player.Current is null)
                return MusicReply.Private(Embeds.Error("Nothing is playing", "There's no song to pause or resume."));

            if (!MusicService.CanControl(member, player))
                return MusicReply.Private(Embeds.Error("Can't do that", ControlRule));

            var target = paused ?? !player.IsPaused;
            if (target == player.IsPaused)
                return MusicReply.Private(Embeds.Info(target ? "Already paused" : "Not paused",
                    target ? "Use `/resume` to continue." : "The music is already playing."));

            if (target)
                player.Pause();
            else
                await player.ResumeAsync();

            return MusicReply.Public(Embeds.Info(target ? "⏸️ Paused" : "▶️ Resumed",
                $"{member.Mention} {(target ? "paused" : "resumed")} the music."));
        }

        public static MusicReply SetVolume(DiscordMember member, ulong guildId, int percent)
        {
            if (Resolve(member, guildId, out var refusal) is not { } player)
                return refusal;

            if (!MusicService.CanControl(member, player))
                return MusicReply.Private(Embeds.Error("Can't change the volume", ControlRule));

            player.SetVolume(percent);
            return MusicReply.Public(Embeds.Info("🔊 Volume", $"{member.Mention} set the volume to **{percent}%**."));
        }

        public static MusicReply SetLoop(DiscordMember member, ulong guildId, LoopMode mode)
        {
            if (Resolve(member, guildId, out var refusal) is not { } player)
                return refusal;

            if (!MusicService.CanControl(member, player))
                return MusicReply.Private(Embeds.Error("Can't change the loop", ControlRule));

            player.Loop = mode;
            var text = mode switch
            {
                LoopMode.Track => "🔂 Looping the current song.",
                LoopMode.Queue => "🔁 Looping the whole queue.",
                _ => "➡️ Loop is off."
            };

            return MusicReply.Public(Embeds.Info("Loop", $"{text} (set by {member.Mention})"));
        }

        public static MusicReply Shuffle(DiscordMember member, ulong guildId)
        {
            if (Resolve(member, guildId, out var refusal) is not { } player)
                return refusal;

            if (!MusicService.CanControl(member, player))
                return MusicReply.Private(Embeds.Error("Can't shuffle", ControlRule));

            var count = player.Shuffle();
            return count < 2
                ? MusicReply.Private(Embeds.Info("Nothing to shuffle", "The queue needs at least two songs."))
                : MusicReply.Public(Embeds.Info("🔀 Shuffled", $"{member.Mention} shuffled {count} songs."));
        }

        public static MusicReply Remove(DiscordMember member, ulong guildId, int position)
        {
            if (Resolve(member, guildId, out var refusal) is not { } player)
                return refusal;

            // A propria faixa sai sem pedir licenca; a dos outros, so com Move
            // Members. "Pediu a faixa atual" nao da direito a mexer no resto.
            var canModerate = player.VoiceChannel is { } channel &&
                              (member.PermissionsIn(channel) & DisCatSharp.Enums.Permissions.MoveMembers) != 0;

            var removed = player.RemoveAt(position, e => e.RequesterId == member.Id || canModerate, out var denied);

            if (denied)
                return MusicReply.Private(Embeds.Error("Not your song",
                    "You can only remove songs you added, unless you have **Move Members**."));

            if (removed is null)
                return MusicReply.Private(Embeds.Error("No such position",
                    $"The queue has {player.QueueCount} song(s). Check `/queue` for the numbers."));

            return MusicReply.Public(Embeds.Info("🗑️ Removed",
                $"{member.Mention} removed {MusicFormat.Link(removed.Track)} from the queue."));
        }

        public static MusicReply Queue(ulong guildId, int page)
        {
            var player = MusicService.Get(guildId);
            if (player is null)
                return MusicReply.Private(Embeds.Info("Queue", "Nothing is playing and the queue is empty."));

            const int perPage = 10;
            var queue = player.SnapshotQueue();
            var pages = Math.Max(1, (int)Math.Ceiling(queue.Count / (double)perPage));
            page = Math.Clamp(page, 1, pages);

            var text = new StringBuilder();
            if (player.Current is { } current)
                text.AppendLine($"**Now:** {MusicFormat.Link(current.Track)} `{MusicFormat.Duration(current.Track.Duration)}`")
                    .AppendLine();

            if (queue.Count == 0)
                text.Append("*The queue is empty.* Add songs with `/play`.");

            foreach (var (entry, index) in queue.Select((e, i) => (e, i)).Skip((page - 1) * perPage).Take(perPage))
                text.AppendLine($"`{index + 1}.` {MusicFormat.Link(entry.Track)} `{MusicFormat.Duration(entry.Track.Duration)}` · <@{entry.RequesterId}>");

            var total = TimeSpan.FromSeconds(queue.Sum(e => e.Track.Duration.TotalSeconds));
            var footer = $"Page {page}/{pages} · {queue.Count} song(s) · {MusicFormat.Duration(total)} total";
            if (player.Loop != LoopMode.Off)
                footer += player.Loop == LoopMode.Track ? " · 🔂 track loop" : " · 🔁 queue loop";

            return MusicReply.Public(new DiscordEmbedBuilder()
                .WithTitle("📜 Queue")
                .WithDescription(Embeds.Trim(text.ToString(), 4000))
                .WithFooter(footer)
                .WithColor(MusicFormat.Color)
                .Build());
        }

        public static MusicReply NowPlaying(ulong guildId)
        {
            var player = MusicService.Get(guildId);
            if (player?.Current is not { } current)
                return MusicReply.Private(Embeds.Info("Nothing is playing", "Use `/play` to start some music."));

            var position = player.Position;
            if (position > current.Track.Duration)
                position = current.Track.Duration;

            var embed = new DiscordEmbedBuilder()
                .WithAuthor(player.IsPaused ? "Paused" : "Now playing")
                .WithDescription($"**{MusicFormat.Link(current.Track)}**\n\n" +
                                 $"{MusicFormat.ProgressBar(position, current.Track.Duration)}\n" +
                                 $"`{MusicFormat.Duration(position)} / {MusicFormat.Duration(current.Track.Duration)}`")
                .AddField(new DiscordEmbedField("Requested by", $"<@{current.RequesterId}>", true))
                .AddField(new DiscordEmbedField("Volume", $"{player.Volume}%", true))
                .AddField(new DiscordEmbedField("Up next", player.QueueCount.ToString(), true))
                .WithColor(MusicFormat.Color);

            if (Uri.TryCreate(current.Track.ThumbnailUrl, UriKind.Absolute, out var thumb) && thumb.Scheme == Uri.UriSchemeHttps)
                embed.WithThumbnail(thumb.AbsoluteUri);

            return MusicReply.Public(embed.Build());
        }
    }
}
