using System;
using System.Collections.Generic;
using System.Linq;
using System.Text;
using System.Threading.Tasks;
using CommunityBot.Services;
using CommunityBot.Services.Economy;
using CommunityBot.Services.Levels;
using DisCatSharp;
using DisCatSharp.ApplicationCommands;
using DisCatSharp.ApplicationCommands.Attributes;
using DisCatSharp.ApplicationCommands.Context;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.Enums.Core;
using DisCatSharp.Exceptions;

namespace CommunityBot.commands
{
    /// <summary>
    /// /profile e /leaderboard: so mostram o que os niveis e a economia ja
    /// guardam. Cada parte so aparece onde o servidor ligou o sistema dela.
    /// </summary>
    internal class Profile : ApplicationCommandsModule
    {
        private const string BoardLevels = "levels";
        private const string BoardSol = "sol";
        private const string ScopeServer = "server";
        private const string ScopeGlobal = "global";
        private const int PerPage = 10;

        private static bool LevelingOn(ulong guildId) => GuildSettingsStore.For(guildId)?.levelingEnabled == true;

        [SlashCommand("profile", "Show someone's Sollarety profile: levels, SOL$ and more")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task ProfileCommand(
            InteractionContext ctx,
            [Option("user", "Whose profile (default: you)")] DiscordUser? usuario = null)
        {
            var target = usuario ?? ctx.User;
            if (target.IsBot)
            {
                await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                    new DiscordInteractionResponseBuilder().AddEmbed(Embeds.Error("Bots don't have profiles",
                        "Only people earn XP and SOL$.")).AsEphemeral());
                return;
            }

            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource);

            var guild = ctx.Guild;
            var settings = guild is null ? null : GuildSettingsStore.For(guild.Id);
            var now = DateTimeOffset.UtcNow;

            var embed = new DiscordEmbedBuilder()
                .WithTitle(Embeds.Trim($"{Levels.NameOf(target)}'s profile", 256))
                .WithThumbnail(target.GetAvatarUrl(MediaFormat.Auto, 256))
                .WithColor(DiscordColor.Gold)
                .WithFooter("Sollarety profile");

            // Nivel neste servidor.
            if (guild is not null && settings?.levelingEnabled == true)
            {
                var board = LevelStore.Instance.GuildBoard(guild.Id);
                var xp = LevelStore.Instance.XpOf(guild.Id, target.Id);
                var position = board.FindIndex(r => r.User == target.Id) + 1;

                embed.AddField(new DiscordEmbedField($"Level in {Embeds.Trim(guild.Name, 200)}",
                    Levels.ProgressLine(xp) + (position > 0 ? $"\nRank **#{position}** of {Embeds.Number(board.Count)}" : "\nNot ranked yet."),
                    false));
            }

            // Nivel global: a soma dos servidores com nivel ligado.
            var global = LevelStore.Instance.GlobalBoard(LevelingOn);
            var globalIndex = global.FindIndex(r => r.User == target.Id);
            embed.AddField(new DiscordEmbedField("Global level", globalIndex >= 0
                ? $"**Level {LevelMath.LevelOf(global[globalIndex].Xp)}** · {Embeds.Number(global[globalIndex].Xp)} XP\n" +
                  $"Rank **#{globalIndex + 1}** of {Embeds.Number(global.Count)}"
                : "No XP yet.", true));

            embed.AddField(new DiscordEmbedField("Leveling in",
                $"{LevelStore.Instance.ActiveGuildCount(target.Id, LevelingOn)} server(s)", true));

            // Carteira: em DM, ou onde a economia esta ligada.
            if (guild is null || settings?.economyEnabled == true)
            {
                var wallet = EconomyStore.Instance.For(target.Id);
                var board = EconomyStore.Instance.Board();
                var position = board.FindIndex(r => r.User == target.Id) + 1;
                var next = EconomyService.NextDaily(wallet, now);

                embed.AddField(new DiscordEmbedField("Wallet",
                    $"**{EconomyService.Format(wallet?.balance ?? 0)}**" +
                    (position > 0 ? $" · Rank **#{position}**" : string.Empty) +
                    $"\n🔥 Daily streak: {EconomyService.LiveStreak(wallet, now)} day(s)" +
                    $"\nNext daily: {(next is { } at ? $"<t:{at.ToUnixTimeSeconds()}:R>" : "ready now — `/daily`")}",
                    false));
            }

            embed.AddField(new DiscordEmbedField("Account created",
                $"<t:{target.CreationTimestamp.ToUnixTimeSeconds()}:D>", true));

            if (guild is not null)
            {
                try
                {
                    var member = await guild.GetMemberAsync(target.Id);
                    if (member.JoinedAt != default)
                        embed.AddField(new DiscordEmbedField("Joined this server",
                            $"<t:{member.JoinedAt.ToUnixTimeSeconds()}:D>", true));
                }
                catch (NotFoundException)
                {
                    // Fora do servidor: o perfil global continua valendo.
                }
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(embed.Build()));
        }

        [SlashCommand("leaderboard", "Top members by level or SOL$, in this server or worldwide")]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 15, CooldownBucketType.User)]
        public async Task LeaderboardCommand(
            InteractionContext ctx,
            [Choice("Levels", BoardLevels)]
            [Choice("SOL$", BoardSol)]
            [Option("board", "What to rank by (default: levels)")] string quadro = BoardLevels,
            [Choice("This server", ScopeServer)]
            [Choice("Global", ScopeGlobal)]
            [Option("scope", "This server or every server (default: this server)")] string escopo = ScopeServer,
            [Option("page", "Which page")][MinimumValue(1)] long pagina = 1)
        {
            var guild = ctx.Guild!;
            var settings = GuildSettingsStore.For(guild.Id);
            var levels = quadro != BoardSol;
            var global = escopo == ScopeGlobal;

            // O servidor decide: com o sistema desligado aqui, nem o ranking
            // global dele aparece aqui.
            if (levels ? settings?.levelingEnabled != true : settings?.economyEnabled != true)
            {
                await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                    new DiscordInteractionResponseBuilder()
                        .AddEmbed(levels ? Levels.LevelingOff() : Economy.EconomyOff()).AsEphemeral());
                return;
            }

            // Os nomes do ranking global vem por REST para quem nao esta no cache.
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource);

            List<(ulong User, long Value)> rows = (levels, global) switch
            {
                (true, false) => LevelStore.Instance.GuildBoard(guild.Id),
                (true, true) => LevelStore.Instance.GlobalBoard(LevelingOn),
                (false, true) => EconomyStore.Instance.Board(),
                // SOL$ no servidor: a carteira e global, entao "do servidor" e o
                // ranking global filtrado pelos membros que o bot tem em cache.
                (false, false) => EconomyStore.Instance.Board().Where(r => guild.Members.ContainsKey(r.User)).ToList()
            };

            var pages = Math.Max(1, (int)Math.Ceiling(rows.Count / (double)PerPage));
            var page = (int)Math.Clamp(pagina, 1, pages);

            var text = new StringBuilder();
            if (rows.Count == 0)
                text.AppendLine(levels ? "*Nobody has XP yet — start chatting!*" : "*Nobody has SOL$ yet — try `/daily`.*");

            var rank = (page - 1) * PerPage;
            foreach (var (userId, value) in rows.Skip((page - 1) * PerPage).Take(PerPage))
            {
                rank++;
                var medal = rank switch { 1 => "🥇", 2 => "🥈", 3 => "🥉", _ => $"`#{rank}`" };
                var name = Embeds.Plain(await NameAsync(ctx.Client, guild, userId, global), 40);
                var detail = levels
                    ? $"Level {LevelMath.LevelOf(value)} · {Embeds.Number(value)} XP"
                    : EconomyService.Format(value);
                text.AppendLine($"{medal} **{name}** — {detail}");
            }

            var mine = rows.FindIndex(r => r.User == ctx.User.Id);
            text.AppendLine().Append(mine >= 0
                ? $"Your position: **#{mine + 1}**"
                : levels ? "You're not ranked yet." : "You don't have any SOL$ yet.");

            var title = (levels, global) switch
            {
                (true, false) => $"🏆 Levels — {guild.Name}",
                (true, true) => "🌍 Levels — global",
                (false, false) => $"💰 SOL$ — {guild.Name}",
                _ => "🌍 SOL$ — global"
            };

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(new DiscordEmbedBuilder()
                .WithTitle(Embeds.Trim(title, 256))
                .WithDescription(Embeds.Trim(text.ToString(), 4000))
                .WithFooter($"Page {page}/{pages} · {Embeds.Number(rows.Count)} ranked")
                .WithColor(levels ? DiscordColor.Gold : Economy.Color)
                .Build()));
        }

        /// <summary>
        /// Apelido no servidor quando a pessoa esta nele; senao o nome da conta.
        /// No ranking global usa sempre o nome da conta - o apelido e coisa de um
        /// servidor, e a pessoa pode nem estar neste.
        /// </summary>
        private static async Task<string> NameAsync(DiscordClient client, DiscordGuild guild, ulong userId, bool global)
        {
            if (!global && guild.Members.TryGetValue(userId, out var member))
                return member.DisplayName;

            try
            {
                var user = await client.GetUserAsync(userId);
                return string.IsNullOrWhiteSpace(user.GlobalName) ? user.Username : user.GlobalName;
            }
            catch (Exception ex) when (ex is NotFoundException or BadRequestException)
            {
                return "Unknown user";
            }
        }
    }
}
