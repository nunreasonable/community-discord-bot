using System;
using System.Linq;
using System.Threading.Tasks;
using CommunityBot.Services;
using CommunityBot.Services.Levels;
using DisCatSharp.ApplicationCommands;
using DisCatSharp.ApplicationCommands.Attributes;
using DisCatSharp.ApplicationCommands.Context;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.Enums.Core;

namespace CommunityBot.commands
{
    /// <summary>
    /// Niveis por XP. Desligados ate quem tem Manage Server ligar com /config
    /// leveling; com eles desligados, /rank e /xp recusam dizendo como ligar.
    /// </summary>
    internal class Levels : ApplicationCommandsModule
    {
        internal static string NameOf(DiscordUser user) =>
            user is DiscordMember member ? member.DisplayName
            : string.IsNullOrWhiteSpace(user.GlobalName) ? user.Username : user.GlobalName;

        internal static DiscordEmbed LevelingOff() =>
            Embeds.Error("Levels are off in this server",
                "Someone with **Manage Server** can turn them on with `/config leveling`.");

        internal static string ProgressLine(long xp)
        {
            var (level, into, needed) = LevelMath.Progress(xp);
            var filled = needed <= 0 ? 0 : (int)Math.Round(12.0 * into / needed);
            var bar = new string('█', filled) + new string('░', 12 - filled);
            return $"**Level {level}** · {Embeds.Number(into)}/{Embeds.Number(needed)} XP\n`{bar}`";
        }

        [SlashCommand("rank", "Show someone's level and XP in this server")]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task RankCommand(
            InteractionContext ctx,
            [Option("user", "Whose rank to show (default: you)")] DiscordUser? usuario = null)
        {
            var guild = ctx.Guild!;
            if (GuildSettingsStore.For(guild.Id)?.levelingEnabled != true)
            {
                await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                    new DiscordInteractionResponseBuilder().AddEmbed(LevelingOff()).AsEphemeral());
                return;
            }

            var target = usuario ?? ctx.User;
            if (target.IsBot)
            {
                await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                    new DiscordInteractionResponseBuilder().AddEmbed(Embeds.Error("Bots don't level up",
                        "Only people earn XP.")).AsEphemeral());
                return;
            }

            var board = LevelStore.Instance.GuildBoard(guild.Id);
            var xp = LevelStore.Instance.XpOf(guild.Id, target.Id);
            var position = board.FindIndex(r => r.User == target.Id) + 1;

            var rankLine = position > 0
                ? $"Rank **#{position}** of {board.Count} · {Embeds.Number(xp)} XP in total"
                : "Not ranked yet — chat a little to earn XP.";

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(new DiscordEmbedBuilder()
                    .WithAuthor(Embeds.Trim($"{NameOf(target)} — {guild.Name}", 256), iconUrl: target.AvatarUrl)
                    .WithDescription($"{ProgressLine(xp)}\n{rankLine}")
                    .WithColor(DiscordColor.Gold)));
        }

        [SlashCommand("level-dms", "Turn the DM you get when you level up on or off")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task LevelDmsCommand(
            InteractionContext ctx,
            [Option("enabled", "Get a DM when you level up")] bool ligado)
        {
            LevelStore.Instance.SetDm(ctx.User.Id, ligado);

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(ligado
                    ? Embeds.Ok("Level-up DMs on", "I'll DM you when you level up in a server that has levels on.")
                    : Embeds.Ok("Level-up DMs off", "No more level-up DMs. Turn them back on anytime with `/level-dms`."))
                    .AsEphemeral());
        }
    }

    /// <summary>
    /// Dar e tirar XP. Mesma permissao do /config: quem pode ligar os niveis
    /// pode corrigi-los. Cada uso vai para o log de moderacao - XP mexe em
    /// ranking, e ranking e o tipo de coisa sobre a qual alguem vai perguntar.
    /// </summary>
    [SlashCommandGroup("xp", "Give or take XP", (long)Permissions.ManageGuild)]
    [ApplicationCommandRequireGuild]
    [ApplicationCommandRequireUserPermissions(Permissions.ManageGuild)]
    internal class XpCommands : ApplicationCommandsModule
    {
        [SlashCommand("give", "Give XP to a member")]
        [SlashCommandCooldown(5, 30, CooldownBucketType.User)]
        public Task GiveCommand(
            InteractionContext ctx,
            [Option("user", "Who gets the XP")] DiscordUser usuario,
            [Option("amount", "How much XP (1 to 1,000,000)")][MinimumValue(1)][MaximumValue(1_000_000)] long quantidade) =>
            ChangeAsync(ctx, usuario, Math.Clamp(quantidade, 1, 1_000_000));

        [SlashCommand("take", "Take XP from a member")]
        [SlashCommandCooldown(5, 30, CooldownBucketType.User)]
        public Task TakeCommand(
            InteractionContext ctx,
            [Option("user", "Who loses the XP")] DiscordUser usuario,
            [Option("amount", "How much XP (1 to 1,000,000)")][MinimumValue(1)][MaximumValue(1_000_000)] long quantidade) =>
            ChangeAsync(ctx, usuario, -Math.Clamp(quantidade, 1, 1_000_000));

        private static async Task ChangeAsync(InteractionContext ctx, DiscordUser target, long delta)
        {
            var guild = ctx.Guild!;

            DiscordEmbed? refusal = null;
            if (GuildSettingsStore.For(guild.Id)?.levelingEnabled != true)
                refusal = Levels.LevelingOff();
            else if (target.IsBot)
                refusal = Embeds.Error("Bots don't level up", "Only people earn XP.");

            if (refusal is not null)
            {
                await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                    new DiscordInteractionResponseBuilder().AddEmbed(refusal).AsEphemeral());
                return;
            }

            var (oldXp, newXp) = LevelStore.Instance.AddXp(guild.Id, target.Id, delta);
            var oldLevel = LevelMath.LevelOf(oldXp);
            var newLevel = LevelMath.LevelOf(newXp);
            var changed = newXp - oldXp;

            var verb = delta > 0 ? "Gave" : "Took";
            var levelNote = newLevel == oldLevel ? $"still level {newLevel}" : $"level {oldLevel} → **{newLevel}**";

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(Embeds.Ok($"XP {(delta > 0 ? "given" : "taken")}",
                        $"{verb} **{Embeds.Number(Math.Abs(changed))} XP** {(delta > 0 ? "to" : "from")} {target.Mention} — " +
                        $"{levelNote}, {Embeds.Number(newXp)} XP in total."))
                    .AsEphemeral());

            // Subir de nivel por XP dado tambem avisa - quem recebe merece
            // saber, e quem desligou as DMs continua sem receber.
            if (newLevel > oldLevel)
                _ = Task.Run(() => LevelService.NotifyLevelUpAsync(guild, target, newLevel, byModerator: true));

            await ModerationLog.RecordAsync(ctx.Client, guild.Id, delta > 0 ? "XP given" : "XP removed",
                target, ctx.User, null,
                $"{(changed >= 0 ? "+" : "")}{Embeds.Number(changed)} XP — now level {newLevel} ({Embeds.Number(newXp)} XP)");
        }
    }
}
