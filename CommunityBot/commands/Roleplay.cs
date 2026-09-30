using System.Threading.Tasks;
using CommunityBot.Services.Fun;
using DisCatSharp.ApplicationCommands;
using DisCatSharp.ApplicationCommands.Attributes;
using DisCatSharp.ApplicationCommands.Context;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.Enums.Core;

namespace CommunityBot.commands
{
    /// <summary>
    /// /roleplay, no molde da Loritta: abraco, tapa, high five e companhia, com
    /// GIF de anime do nekos.best e um botao "Return" para o alvo devolver.
    ///
    /// Sem RequireGuild: funciona em DM tambem - la o "user" so enxerga quem
    /// esta na conversa, e o botao nao depende de servidor nenhum.
    /// </summary>
    [SlashCommandGroup("roleplay", "Hug, pat, slap and more — with an anime GIF")]
    internal class Roleplay : ApplicationCommandsModule
    {
        [SlashCommand("hug", "Give someone a hug")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task Hug(InteractionContext ctx, [Option("user", "Who to hug")] DiscordUser user) =>
            RunAsync(ctx, "hug", user);

        [SlashCommand("kiss", "Give someone a kiss")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task Kiss(InteractionContext ctx, [Option("user", "Who to kiss")] DiscordUser user) =>
            RunAsync(ctx, "kiss", user);

        [SlashCommand("slap", "Slap someone")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task Slap(InteractionContext ctx, [Option("user", "Who to slap")] DiscordUser user) =>
            RunAsync(ctx, "slap", user);

        [SlashCommand("pat", "Give someone a headpat")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task Pat(InteractionContext ctx, [Option("user", "Who to pat")] DiscordUser user) =>
            RunAsync(ctx, "pat", user);

        [SlashCommand("highfive", "High-five someone")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task HighFive(InteractionContext ctx, [Option("user", "Who to high-five")] DiscordUser user) =>
            RunAsync(ctx, "highfive", user);

        [SlashCommand("dance", "Dance with someone")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task Dance(InteractionContext ctx, [Option("user", "Who to dance with")] DiscordUser user) =>
            RunAsync(ctx, "dance", user);

        [SlashCommand("attack", "Attack someone")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task Attack(InteractionContext ctx, [Option("user", "Who to attack")] DiscordUser user) =>
            RunAsync(ctx, "attack", user);

        [SlashCommand("cuddle", "Cuddle with someone")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task Cuddle(InteractionContext ctx, [Option("user", "Who to cuddle")] DiscordUser user) =>
            RunAsync(ctx, "cuddle", user);

        [SlashCommand("poke", "Poke someone")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task Poke(InteractionContext ctx, [Option("user", "Who to poke")] DiscordUser user) =>
            RunAsync(ctx, "poke", user);

        [SlashCommand("bonk", "Bonk someone")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task Bonk(InteractionContext ctx, [Option("user", "Who to bonk")] DiscordUser user) =>
            RunAsync(ctx, "bonk", user);

        private static async Task RunAsync(InteractionContext ctx, string key, DiscordUser user)
        {
            var action = RoleplayActions.Find(key)!;
            var bot = ctx.Client.CurrentUser;
            var (actor, target, note) = RoleplayActions.Resolve(action, ctx.User, user, bot);

            if (actor is null)
            {
                await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                    new DiscordInteractionResponseBuilder().AddEmbed(new DiscordEmbedBuilder()
                        .WithDescription($"😳 {ctx.User.Mention}, I'm flattered, but I'm just a bot. How about a hug instead?")
                        .WithColor(action.Color)));
                return;
            }

            // O GIF vem de fora e pode levar alguns segundos.
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource);

            var (embed, buttons) = await RoleplayActions.BuildAsync(action, actor, target, 1, note);
            var reply = new DiscordWebhookBuilder().AddEmbed(embed);
            if (buttons.Length > 0)
                reply.AddComponents(buttons);

            await ctx.EditResponseAsync(reply);
        }
    }
}
