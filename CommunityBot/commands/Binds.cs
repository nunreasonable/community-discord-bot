using System;
using System.Linq;
using System.Threading.Tasks;
using CommunityBot.Services;
using CommunityBot.Services.Roblox;
using DisCatSharp.ApplicationCommands;
using DisCatSharp.ApplicationCommands.Attributes;
using DisCatSharp.ApplicationCommands.Context;
using DisCatSharp.Entities;
using DisCatSharp.Enums;

namespace CommunityBot.commands
{
    /// <summary>
    /// Binds de grupo: "quem esta no grupo X com rank entre A e B ganha o cargo Y".
    ///
    /// Exige Gerenciar Servidor E Gerenciar Cargos. Um bind e uma regra que da
    /// cargo sozinha a quem entra, entao pede as duas coisas que ele mexe:
    /// configuracao do servidor e distribuicao de cargo. E o cargo escolhido
    /// ainda passa pela checagem de hierarquia de quem cria o bind - ver
    /// VerificationService.CheckAssignableRole.
    /// </summary>
    [SlashCommandGroup("bind", "Discord roles based on Roblox group and rank",
        (long)(Permissions.ManageGuild | Permissions.ManageRoles), allowedContexts: new[] { InteractionContextType.Guild })]
    [ApplicationCommandRequireGuild]
    [ApplicationCommandRequireUserPermissions(Permissions.ManageGuild | Permissions.ManageRoles)]
    internal class Binds : ApplicationCommandsModule
    {
        /// <summary>
        /// Cada bind custa, por pessoa verificada, zero chamadas extras ao Roblox
        /// (uma leitura de grupos serve a todos), mas a lista precisa caber num
        /// embed do /bind list. Cinquenta cabem com folga.
        /// </summary>
        private const int MaxBinds = 50;

        [SlashCommand("add", "Gives a role to members of a Roblox group within a rank range")]
        public async Task AddCommand(
            InteractionContext ctx,
            [Option("group", "Roblox group ID (the number in the group's URL)")] long grupo,
            [Option("role", "Discord role the bind gives")] DiscordRole cargo,
            [Option("min_rank", "Lowest rank that gets the role, 0 to 255 (default 1: any member)")] long rankMinimo = 1,
            [Option("max_rank", "Highest rank that gets the role, 0 to 255 (default 255)")] long rankMaximo = 255)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            if (grupo <= 0)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(Embeds.Error("Invalid group",
                    "The group ID is the number in its URL, as in `roblox.com/communities/1234567/...`.")));
                return;
            }

            if (rankMinimo is < 0 or > 255 || rankMaximo is < 0 or > 255 || rankMinimo > rankMaximo)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(Embeds.Error("Invalid range",
                    "Ranks go from 0 to 255, and the minimum can't be higher than the maximum. Rank 0 means being **outside** the group.")));
                return;
            }

            if (VerificationService.CheckAssignableRole(ctx.Guild!, ctx.Member!, cargo) is { } refusal)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(refusal));
                return;
            }

            var group = await RobloxApi.GetGroupAsync(grupo);
            if (!group.Ok)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(group.NotFound
                    ? Embeds.Error("Group not found", $"Roblox says group `{grupo}` doesn't exist.")
                    : Embeds.Error("Couldn't confirm the group", $"{group.Error}. Try again in a few seconds.")));
                return;
            }

            var (bind, error) = await GuildSettingsStore.Instance.UpdateAsync<(GroupBind? Bind, string? Error)>(edit =>
            {
                var settings = edit.Get(ctx.Guild!.Id);

                if (settings.groupBinds.Count >= MaxBinds)
                    return (null, $"This server already has {MaxBinds} binds, the maximum. Remove one with `/bind remove`.");

                if (settings.groupBinds.Any(b => b.groupId == grupo && b.roleId == cargo.Id &&
                                                 b.minRank == rankMinimo && b.maxRank == rankMaximo))
                    return (null, "An identical bind already exists.");

                string id;
                do
                {
                    id = Guid.NewGuid().ToString("N")[..6];
                }
                while (settings.groupBinds.Any(b => string.Equals(b.id, id, StringComparison.OrdinalIgnoreCase)));

                var created = new GroupBind
                {
                    id = id,
                    groupId = grupo,
                    groupName = Embeds.Trim(group.Value!.Name, 100),
                    minRank = (int)rankMinimo,
                    maxRank = (int)rankMaximo,
                    roleId = cargo.Id,
                    createdById = ctx.User.Id,
                    createdAtUtc = DateTimeOffset.UtcNow
                };

                settings.groupBinds.Add(created);
                settings.updatedAtUtc = DateTimeOffset.UtcNow;
                settings.updatedById = ctx.User.Id;
                edit.MarkChanged();
                return (created.Copy(), null);
            });

            if (bind is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(Embeds.Error("Bind not created", error!)));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(Embeds.Ok("Bind created",
                $"{Describe(bind)}\n\n" +
                "Applies to anyone who verifies or joins from now on. Members who are **already** verified get it on their next " +
                "`/update` — they can run it themselves, or someone with Manage Roles can run it for them.")));
        }

        [SlashCommand("remove", "Removes a bind by the id shown in /bind list")]
        public async Task RemoveCommand(
            InteractionContext ctx,
            [Option("id", "Bind id")] string id)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var removed = await GuildSettingsStore.Instance.UpdateAsync<GroupBind?>(edit =>
            {
                var settings = edit.Get(ctx.Guild!.Id);
                var bind = settings.groupBinds.FirstOrDefault(b =>
                    string.Equals(b.id, id.Trim(), StringComparison.OrdinalIgnoreCase));
                if (bind is null)
                    return null;

                settings.groupBinds.Remove(bind);
                settings.updatedAtUtc = DateTimeOffset.UtcNow;
                settings.updatedById = ctx.User.Id;
                edit.MarkChanged();
                return bind.Copy();
            });

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(removed is null
                ? Embeds.Error("Bind not found", $"There's no bind with id `{Embeds.SafeTrim(id, 20)}` in this server. See `/bind list`.")
                : Embeds.Ok("Bind removed",
                    $"{Describe(removed)}\n\nThe bot stops giving this role. Members who have it **keep it**: with no bind " +
                    "pointing to the role, the bot stops touching it entirely, including removing it.")));
        }

        [SlashCommand("list", "Lists this server's group binds")]
        public async Task ListCommand(InteractionContext ctx)
        {
            var binds = GuildSettingsStore.For(ctx.Guild!.Id)?.groupBinds ?? new();

            var embed = binds.Count == 0
                ? Embeds.Info("No binds", "This server has no group binds. Create one with `/bind add`.")
                : new DiscordEmbedBuilder()
                    .WithTitle($"Group binds ({binds.Count})")
                    .WithColor(DiscordColor.Blurple)
                    .WithDescription(Embeds.Trim(string.Join("\n", binds.Select(b => $"`{b.id}` {Describe(b)}")), 4000))
                    .Build();

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(embed).AsEphemeral());
        }

        private static string Describe(GroupBind bind)
        {
            var ranks = (bind.minRank, bind.maxRank) switch
            {
                (1, 255) => "any rank",
                (0, 255) => "anyone, even outside the group",
                var (min, max) when min == max => $"rank {min}",
                var (min, max) => $"ranks {min}–{max}"
            };

            return $"**{Embeds.Safe(bind.groupName)}** (`{bind.groupId}`), {ranks} → <@&{bind.roleId}>";
        }
    }
}
