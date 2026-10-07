using System;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using CommunityBot.Services;
using CommunityBot.Services.Music;
using DisCatSharp.ApplicationCommands;
using DisCatSharp.ApplicationCommands.Attributes;
using DisCatSharp.ApplicationCommands.Context;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.Enums.Core;

namespace CommunityBot.commands
{
    /// <summary>
    /// Musica do YouTube, do SoundCloud e do Spotify. Video do YouTube e BAIXADO
    /// com o yt-dlp, tocado e apagado logo depois - nunca transmitido direto.
    /// SoundCloud toca por streaming. Link do Spotify vira a mesma musica no
    /// SoundCloud ou, se nao bater, no YouTube. Nada de audio fica guardado - a
    /// fila e so metadado, em memoria.
    ///
    /// Todo comando daqui exige servidor: canal de voz so existe la. As regras de
    /// quem pode o que moram no MusicActions, compartilhadas com os botoes.
    /// </summary>
    internal class Music : ApplicationCommandsModule
    {
        private const string LoopOff = "off";
        private const string LoopTrack = "track";
        private const string LoopQueue = "queue";

        private const string SearchYouTube = "youtube";
        private const string SearchSoundCloud = "soundcloud";

        [SlashCommand("play", "Play from YouTube, SoundCloud or Spotify — paste a link or type what to search for", allowedContexts: new[] { InteractionContextType.Guild })]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 15, CooldownBucketType.User)]
        public async Task PlayCommand(
            InteractionContext ctx,
            [Option("song", "A YouTube, SoundCloud or Spotify link (tracks, playlists, albums), or a search")] string busca,
            [Choice("YouTube", SearchYouTube)]
            [Choice("SoundCloud", SearchSoundCloud)]
            [Option("search_on", "Where to search when you type a name instead of a link (default: YouTube)")]
            string ondeBuscar = SearchYouTube)
        {
            var guild = ctx.Guild!;
            var member = ctx.Member!;

            // As checagens baratas vem ANTES do defer, para a recusa sair
            // efemera. Depois do defer a resposta ja nasceu publica.
            if (member.VoiceState?.Channel is not { } voice)
            {
                await RespondAsync(ctx, MusicReply.Private(Embeds.Error("Join a voice channel first",
                    "Hop into a voice channel and run `/play` again.")));
                return;
            }

            if (voice.Type == ChannelType.Stage)
            {
                await RespondAsync(ctx, MusicReply.Private(Embeds.Error("Stage channels aren't supported",
                    "Use a regular voice channel for music.")));
                return;
            }

            // O RequireBotPermissions olharia o canal de TEXTO onde o comando foi
            // digitado; o que importa aqui e o canal de voz.
            const Permissions needed = Permissions.AccessChannels | Permissions.UseVoice | Permissions.Speak;
            if (guild.CurrentMember is not { } self || (voice.PermissionsFor(self) & needed) != needed)
            {
                await RespondAsync(ctx, MusicReply.Private(Embeds.Error("I can't use that channel",
                    $"I need **View Channel**, **Connect** and **Speak** in {voice.Mention}.")));
                return;
            }

            if (MusicService.Get(guild.Id) is { } playing && playing.VoiceChannel?.Id != voice.Id)
            {
                await RespondAsync(ctx, MusicReply.Private(Embeds.Error("I'm busy in another channel",
                    $"I'm already playing in {playing.VoiceChannel?.Mention ?? "another channel"}. Join me there to add songs.")));
                return;
            }

            // Buscar e entrar no canal levam segundos - bem mais que os 3 do
            // Discord para a primeira resposta.
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource);

            if (await MusicService.CheckAvailabilityAsync() is not null)
            {
                await FailAfterDeferAsync(ctx, "Music is unavailable",
                    "Music isn't set up on this bot right now. The bot's admin can see why in the logs.");
                return;
            }

            var settings = await MusicService.ReadSettingsAsync();

            MusicSources.Request request;
            try
            {
                var searchSource = ondeBuscar == SearchSoundCloud ? TrackSource.SoundCloud : TrackSource.YouTube;
                request = await MusicSources.ResolveAsync(busca, searchSource, settings, CancellationToken.None);
            }
            catch (MusicException ex)
            {
                await FailAfterDeferAsync(ctx, "Can't play that", ex.Message);
                return;
            }
            catch (Exception ex) when (ex is TimeoutException or FileNotFoundException or InvalidOperationException)
            {
                Console.WriteLine($"[musica] falha ao resolver '{busca}' em {guild.Id}: {ex.Message}");
                await FailAfterDeferAsync(ctx, "Can't play that", "Looking that up took too long or failed. Try again.");
                return;
            }

            GuildPlayer player;
            try
            {
                player = await MusicService.GetOrJoinAsync(ctx.Client, guild, voice, ctx.Channel);
            }
            catch (MusicException ex)
            {
                await FailAfterDeferAsync(ctx, "Can't join", ex.Message);
                return;
            }

            var entries = request.Tracks.Select(t => new QueuedTrack(t, member.Id, member.DisplayName)).ToList();
            var added = player.TryEnqueue(entries, settings.MaxQueue, out var position);
            if (added == 0)
            {
                await FailAfterDeferAsync(ctx, "The queue is full",
                    $"There are already {settings.MaxQueue} songs waiting. Try again once a few have played.");
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                AddedEmbed(request, added, position, settings, member.DisplayName)));
        }

        [SlashCommand("skip", "Skip the current song (or vote to skip it)", allowedContexts: new[] { InteractionContextType.Guild })]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task SkipCommand(InteractionContext ctx) =>
            RespondAsync(ctx, MusicActions.Skip(ctx.Member!, ctx.Guild!.Id));

        [SlashCommand("stop", "Stop the music, clear the queue and leave the voice channel", allowedContexts: new[] { InteractionContextType.Guild })]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task StopCommand(InteractionContext ctx) =>
            RespondAsync(ctx, MusicActions.Stop(ctx.Member!, ctx.Guild!.Id));

        [SlashCommand("pause", "Pause the current song", allowedContexts: new[] { InteractionContextType.Guild })]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task PauseCommand(InteractionContext ctx) =>
            await RespondAsync(ctx, await MusicActions.SetPausedAsync(ctx.Member!, ctx.Guild!.Id, true));

        [SlashCommand("resume", "Resume the paused song", allowedContexts: new[] { InteractionContextType.Guild })]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task ResumeCommand(InteractionContext ctx) =>
            await RespondAsync(ctx, await MusicActions.SetPausedAsync(ctx.Member!, ctx.Guild!.Id, false));

        [SlashCommand("volume", "Change the music volume", allowedContexts: new[] { InteractionContextType.Guild })]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task VolumeCommand(
            InteractionContext ctx,
            [Option("percent", "From 1 to 150 (default 100)")][MinimumValue(1)][MaximumValue(150)] long percent) =>
            RespondAsync(ctx, MusicActions.SetVolume(ctx.Member!, ctx.Guild!.Id, (int)Math.Clamp(percent, 1, 150)));

        [SlashCommand("loop", "Repeat the current song or the whole queue", allowedContexts: new[] { InteractionContextType.Guild })]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task LoopCommand(
            InteractionContext ctx,
            [Choice("Off", LoopOff)]
            [Choice("Current song", LoopTrack)]
            [Choice("Whole queue", LoopQueue)]
            [Option("mode", "What to repeat")] string modo)
        {
            var mode = modo switch
            {
                LoopTrack => LoopMode.Track,
                LoopQueue => LoopMode.Queue,
                _ => LoopMode.Off
            };

            return RespondAsync(ctx, MusicActions.SetLoop(ctx.Member!, ctx.Guild!.Id, mode));
        }

        [SlashCommand("shuffle", "Shuffle the songs waiting in the queue", allowedContexts: new[] { InteractionContextType.Guild })]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task ShuffleCommand(InteractionContext ctx) =>
            RespondAsync(ctx, MusicActions.Shuffle(ctx.Member!, ctx.Guild!.Id));

        [SlashCommand("remove", "Remove a song from the queue", allowedContexts: new[] { InteractionContextType.Guild })]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task RemoveCommand(
            InteractionContext ctx,
            [Option("position", "Its number in /queue")][MinimumValue(1)] long posicao) =>
            RespondAsync(ctx, MusicActions.Remove(ctx.Member!, ctx.Guild!.Id, (int)Math.Clamp(posicao, 1, int.MaxValue)));

        [SlashCommand("queue", "Show the songs waiting to play", allowedContexts: new[] { InteractionContextType.Guild })]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task QueueCommand(
            InteractionContext ctx,
            [Option("page", "Which page of the queue")][MinimumValue(1)] long pagina = 1) =>
            RespondAsync(ctx, MusicActions.Queue(ctx.Guild!.Id, (int)Math.Clamp(pagina, 1, 1000)));

        [SlashCommand("nowplaying", "Show the song that's playing and how far along it is", allowedContexts: new[] { InteractionContextType.Guild })]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task NowPlayingCommand(InteractionContext ctx) =>
            RespondAsync(ctx, MusicActions.NowPlaying(ctx.Guild!.Id));

        /// <summary>O "vai tocar" / "entrou na fila" do /play, para uma faixa ou uma lista.</summary>
        private static DiscordEmbed AddedEmbed(MusicSources.Request request, int added, int position,
            CommunityBot.config.MusicSettings settings, string requester)
        {
            var embed = new DiscordEmbedBuilder()
                .WithColor(MusicFormat.Color)
                .WithFooter($"Requested by {requester}");

            var first = request.Tracks[0];

            if (request.CollectionName is null)
            {
                // O que acontece ate a musica comecar depende da fonte - e e o
                // que explica os segundos de espera.
                var startNote = first.Source switch
                {
                    TrackSource.YouTube => "Downloading it now — it'll start in a few seconds.",
                    TrackSource.Spotify => "Finding it on SoundCloud or YouTube — it'll start in a few seconds.",
                    _ => "Starting the stream now."
                };

                var line = $"{MusicFormat.Link(first)} `{MusicFormat.Duration(first.Duration)}`";
                if (first.Source == TrackSource.Spotify && !string.IsNullOrWhiteSpace(first.Uploader))
                    line += $" · {Embeds.SafeTrim(first.Uploader, 100)}";

                if (position == 0)
                    embed.WithTitle("🎶 Starting up").WithDescription($"{line}\n{startNote}");
                else
                    embed.WithTitle("➕ Added to the queue").WithDescription($"{line}\nPosition **#{position}**.");

                if (Uri.TryCreate(first.ThumbnailUrl, UriKind.Absolute, out var thumb) && thumb.Scheme == Uri.UriSchemeHttps)
                    embed.WithThumbnail(thumb.AbsoluteUri);

                return embed.Build();
            }

            var text = $"From **{Embeds.SafeTrim(request.CollectionName, 150)}** on {MusicFormat.SourceName(request.Source)}.\n" +
                       (position == 0 ? "The first one is starting now." : $"They start at position **#{position}**.");

            // O que ficou de fora e dito, e por que: limite por link ou fila cheia.
            var leftOut = request.Tracks.Count - added;
            if (request.Truncated > 0)
                text += $"\n-# Only the first {settings.MaxPlaylist} tracks of a link are added.";
            if (leftOut > 0)
                text += $"\n-# {leftOut} more didn't fit — the queue holds {settings.MaxQueue} songs.";

            if (Uri.TryCreate(first.ThumbnailUrl, UriKind.Absolute, out var cover) && cover.Scheme == Uri.UriSchemeHttps)
                embed.WithThumbnail(cover.AbsoluteUri);

            return embed.WithTitle($"➕ Added {added} song{(added == 1 ? "" : "s")}").WithDescription(text).Build();
        }

        private static Task RespondAsync(InteractionContext ctx, MusicReply reply)
        {
            // Sem mencao de ninguem: as respostas citam quem agiu (<@id>), e isso
            // e para ler, nao para notificar.
            var builder = new DiscordInteractionResponseBuilder()
                .AddEmbed(reply.Embed)
                .WithAllowedMentions(Array.Empty<IMention>());

            if (reply.Ephemeral)
                builder.AsEphemeral();

            return ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource, builder);
        }

        /// <summary>
        /// Recusa depois do defer publico: apaga o "pensando..." e manda o erro
        /// so para quem pediu, em vez de deixar um cartao vermelho no canal.
        /// </summary>
        private static async Task FailAfterDeferAsync(InteractionContext ctx, string title, string message)
        {
            try
            {
                await ctx.DeleteResponseAsync();
                await ctx.FollowUpAsync(new DiscordFollowupMessageBuilder()
                    .AddEmbed(Embeds.Error(title, message))
                    .AsEphemeral());
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[musica] aviso: recusa efemera falhou ({ex.Message}); editando a publica");
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(Embeds.Error(title, message)));
            }
        }
    }
}
