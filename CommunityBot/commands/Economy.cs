using System;
using System.Threading.Tasks;
using CommunityBot.Services;
using CommunityBot.Services.Economy;
using DisCatSharp.ApplicationCommands;
using DisCatSharp.ApplicationCommands.Attributes;
using DisCatSharp.ApplicationCommands.Context;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.Enums.Core;

namespace CommunityBot.commands
{
    /// <summary>
    /// Economia de SOL$, no molde da Loritta. A carteira e GLOBAL - a mesma em
    /// todo servidor -, mas os comandos so funcionam onde quem tem Manage Server
    /// ligou /config economy.
    /// </summary>
    internal class Economy : ApplicationCommandsModule
    {
        internal static readonly DiscordColor Color = new(0xF2B705);

        internal static DiscordEmbed EconomyOff() =>
            Embeds.Error("The economy is off in this server",
                "Someone with **Manage Server** can turn it on with `/config economy`.");

        private static bool Enabled(InteractionContext ctx) =>
            ctx.Guild is { } guild && GuildSettingsStore.For(guild.Id)?.economyEnabled == true;

        private static Task RefuseAsync(InteractionContext ctx, DiscordEmbed embed) =>
            ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(embed).AsEphemeral());

        private static string When(DateTimeOffset at) => $"<t:{at.ToUnixTimeSeconds()}:R>";

        [SlashCommand("daily", "Claim your daily SOL$ — come back every day to build a streak")]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task DailyCommand(InteractionContext ctx)
        {
            if (!Enabled(ctx))
            {
                await RefuseAsync(ctx, EconomyOff());
                return;
            }

            var result = await EconomyService.ClaimDailyAsync(ctx.User.Id, DateTimeOffset.UtcNow);

            if (!result.Claimed)
            {
                await RefuseAsync(ctx, Embeds.Info("Already claimed today",
                    $"Your next daily unlocks {When(result.NextAt)}. 🔥 Streak: **{result.Streak}** day(s)."));
                return;
            }

            var bonus = result.Streak > 1
                ? $"\n🔥 **{result.Streak}-day streak** — keep it going for a bigger reward (up to {EconomyService.Format(EconomyService.DailyBase + EconomyService.DailyStreakBonus * EconomyService.DailyStreakCap)})."
                : "\nCome back tomorrow to start a streak.";

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(new DiscordEmbedBuilder()
                    .WithTitle("☀️ Daily claimed")
                    .WithDescription($"{ctx.User.Mention} got **{EconomyService.Format(result.Reward)}**!{bonus}\n\n" +
                                     $"Balance: **{EconomyService.Format(result.Balance)}** · next daily {When(result.NextAt)}")
                    .WithColor(Color))
                    .WithAllowedMentions(Array.Empty<IMention>()));
        }

        [SlashCommand("work", "Work a shift for some SOL$ (once an hour)")]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task WorkCommand(InteractionContext ctx)
        {
            if (!Enabled(ctx))
            {
                await RefuseAsync(ctx, EconomyOff());
                return;
            }

            var result = await EconomyService.WorkAsync(ctx.User.Id, DateTimeOffset.UtcNow);

            if (!result.Worked)
            {
                await RefuseAsync(ctx, Embeds.Info("You're still tired",
                    $"You can work again {When(result.NextAt)}."));
                return;
            }

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(new DiscordEmbedBuilder()
                    .WithTitle("💼 Shift done")
                    .WithDescription($"{result.Job} and earned **{EconomyService.Format(result.Reward)}**.\n\n" +
                                     $"Balance: **{EconomyService.Format(result.Balance)}** · next shift {When(result.NextAt)}")
                    .WithColor(Color)));
        }

        [SlashCommand("pay", "Send some of your SOL$ to someone")]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 15, CooldownBucketType.User)]
        public async Task PayCommand(
            InteractionContext ctx,
            [Option("user", "Who gets the SOL$")] DiscordUser usuario,
            [Option("amount", "How many SOL$ to send")][MinimumValue(1)] long quantidade)
        {
            if (!Enabled(ctx))
            {
                await RefuseAsync(ctx, EconomyOff());
                return;
            }

            // Recusas antes do pedido de confirmacao: nao faz sentido perguntar
            // "tem certeza?" de algo que nao vai poder acontecer.
            var balance = EconomyStore.Instance.For(ctx.User.Id)?.balance ?? 0;
            string? refusal = null;
            if (usuario.IsBot)
                refusal = "Bots don't have wallets.";
            else if (usuario.Id == ctx.User.Id)
                refusal = "You can't pay yourself.";
            else if (quantidade <= 0)
                refusal = "The amount has to be more than zero.";
            else if (balance < quantidade)
                refusal = $"You only have {EconomyService.Format(balance)}.";

            if (refusal is not null)
            {
                await RefuseAsync(ctx, Embeds.Error("Can't send that", refusal));
                return;
            }

            var now = DateTimeOffset.UtcNow;
            var nonce = EconomyService.RegisterPending(ctx.User.Id, usuario.Id, quantidade, now);

            // Publico: doacao e para os outros verem. Mas a menção nao notifica
            // ninguem ate o pagamento ser confirmado.
            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder()
                    .AddEmbed(new DiscordEmbedBuilder()
                        .WithTitle("💸 Confirm payment")
                        .WithDescription($"{ctx.User.Mention}, send **{EconomyService.Format(quantidade)}** to {usuario.Mention}?\n" +
                                         $"-# Only you can confirm. This expires {When(now + EconomyService.PayConfirmWindow)}.")
                        .WithColor(Color))
                    .AddComponents(
                        new DiscordButtonComponent(ButtonStyle.Success, $"{EconomyComponents.Prefix}pay:{nonce}", "Send",
                            emoji: new DiscordComponentEmoji("✅")),
                        new DiscordButtonComponent(ButtonStyle.Secondary, $"{EconomyComponents.Prefix}cancel:{nonce}", "Cancel"))
                    .WithAllowedMentions(Array.Empty<IMention>()));
        }

        [SlashCommand("balance", "See how many SOL$ someone has")]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task BalanceCommand(
            InteractionContext ctx,
            [Option("user", "Whose wallet (default: you)")] DiscordUser? usuario = null)
        {
            if (!Enabled(ctx))
            {
                await RefuseAsync(ctx, EconomyOff());
                return;
            }

            var target = usuario ?? ctx.User;
            var balance = EconomyStore.Instance.For(target.Id)?.balance ?? 0;
            var board = EconomyStore.Instance.Board();
            var position = board.FindIndex(r => r.User == target.Id) + 1;

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(new DiscordEmbedBuilder()
                    .WithAuthor(Embeds.Trim($"{Levels.NameOf(target)}'s wallet", 256), iconUrl: target.AvatarUrl)
                    .WithDescription($"**{EconomyService.Format(balance)}**" +
                                     (position > 0 ? $"\nGlobal rank **#{position}** of {Embeds.Number(board.Count)}" : string.Empty))
                    .WithColor(Color)));
        }
    }
}
