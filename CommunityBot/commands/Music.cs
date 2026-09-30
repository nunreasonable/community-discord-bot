using System;
using System.IO;
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
    /// Musica do YouTube: o bot baixa o audio com o yt-dlp, toca no canal de voz
    /// e apaga o arquivo logo depois. Nada de audio fica guardado - a fila e so
    /// metadado, em memoria.
    ///
    /// Todo comando daqui exige servidor: canal de voz so existe la. As regras de
    /// quem pode o que moram no MusicActions, compartilhadas com os botoes.
    /// </summary>
    internal class Music : ApplicationCommandsModule
    {
        private const string LoopOff = "off";
        private const string LoopTrack = "track";
        private const string LoopQueue = "queue";

        [SlashCommand("play", "Play a song from YouTube — paste a link or type what to search for")]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 15, CooldownBucketType.User)]
        public async Task PlayCommand(
            InteractionContext ctx,
            [Option("song", "A YouTube link, or what to search for")] string busca)
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

            TrackInfo track;
            try
            {
                track = await YtDlp.ResolveAsync(busca, settings, CancellationToken.None);
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

            if (!player.TryEnqueue(new QueuedTrack(track, member.Id, member.DisplayName), settings.MaxQueue, out var position))
            {
                await FailAfterDeferAsync(ctx, "The queue is full",
                    $"There are already {settings.MaxQueue} songs waiting. Try again once a few have played.");
                return;
            }

            var embed = new DiscordEmbedBuilder()
                .WithColor(MusicFormat.Color)
                .WithFooter($"Requested by {member.DisplayName}");

            if (position == 0)
                embed.WithTitle("🎶 Starting up")
                    .WithDescription($"{MusicFormat.Link(track)} `{MusicFormat.Duration(track.Duration)}`\n" +
                                     "Downloading it now — it'll start in a few seconds.");
            else
                embed.WithTitle("➕ Added to the queue")
                    .WithDescription($"{MusicFormat.Link(track)} `{MusicFormat.Duration(track.Duration)}`\n" +
                                     $"Position **#{position}**.");

            if (Uri.TryCreate(track.ThumbnailUrl, UriKind.Absolute, out var thumb) && thumb.Scheme == Uri.UriSchemeHttps)
                embed.WithThumbnail(thumb.AbsoluteUri);

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(embed.Build()));
        }

        [SlashCommand("skip", "Skip the current song (or vote to skip it)")]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task SkipCommand(InteractionContext ctx) =>
            RespondAsync(ctx, MusicActions.Skip(ctx.Member!, ctx.Guild!.Id));

        [SlashCommand("stop", "Stop the music, clear the queue and leave the voice channel")]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task StopCommand(InteractionContext ctx) =>
            RespondAsync(ctx, MusicActions.Stop(ctx.Member!, ctx.Guild!.Id));

        [SlashCommand("pause", "Pause the current song")]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task PauseCommand(InteractionContext ctx) =>
            await RespondAsync(ctx, await MusicActions.SetPausedAsync(ctx.Member!, ctx.Guild!.Id, true));

        [SlashCommand("resume", "Resume the paused song")]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task ResumeCommand(InteractionContext ctx) =>
            await RespondAsync(ctx, await MusicActions.SetPausedAsync(ctx.Member!, ctx.Guild!.Id, false));

        [SlashCommand("volume", "Change the music volume")]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task VolumeCommand(
            InteractionContext ctx,
            [Option("percent", "From 1 to 150 (default 100)")][MinimumValue(1)][MaximumValue(150)] long percent) =>
            RespondAsync(ctx, MusicActions.SetVolume(ctx.Member!, ctx.Guild!.Id, (int)Math.Clamp(percent, 1, 150)));

        [SlashCommand("loop", "Repeat the current song or the whole queue")]
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

        [SlashCommand("shuffle", "Shuffle the songs waiting in the queue")]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task ShuffleCommand(InteractionContext ctx) =>
            RespondAsync(ctx, MusicActions.Shuffle(ctx.Member!, ctx.Guild!.Id));

        [SlashCommand("remove", "Remove a song from the queue")]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task RemoveCommand(
            InteractionContext ctx,
            [Option("position", "Its number in /queue")][MinimumValue(1)] long posicao) =>
            RespondAsync(ctx, MusicActions.Remove(ctx.Member!, ctx.Guild!.Id, (int)Math.Clamp(posicao, 1, int.MaxValue)));

        [SlashCommand("queue", "Show the songs waiting to play")]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task QueueCommand(
            InteractionContext ctx,
            [Option("page", "Which page of the queue")][MinimumValue(1)] long pagina = 1) =>
            RespondAsync(ctx, MusicActions.Queue(ctx.Guild!.Id, (int)Math.Clamp(pagina, 1, 1000)));

        [SlashCommand("nowplaying", "Show the song that's playing and how far along it is")]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task NowPlayingCommand(InteractionContext ctx) =>
            RespondAsync(ctx, MusicActions.NowPlaying(ctx.Guild!.Id));

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
