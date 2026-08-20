using System;
using System.Linq;
using System.Threading.Tasks;
using CommunityBot.config;
using CommunityBot.Services;
using DisCatSharp.ApplicationCommands;
using DisCatSharp.ApplicationCommands.Attributes;
using DisCatSharp.ApplicationCommands.Context;
using DisCatSharp.Entities;
using DisCatSharp.Enums;

namespace CommunityBot.commands
{
    /// <summary>
    /// Advertências com histórico.
    ///
    /// É o que dá memória à moderação: sem isto cada punição é um evento solto e
    /// ninguém sabe se a pessoa já foi avisada três vezes antes.
    /// </summary>
    [ApplicationCommandRequireGuild]
    internal class Warnings : ApplicationCommandsModule
    {
        /// <summary>Quantas advertências cabem numa página do /warnings.</summary>
        private const int PageSize = 10;

        [SlashCommand("warn", "Registra uma advertência para um usuário")]
        [ApplicationCommandRequireUserPermissions(Permissions.ModerateMembers)]
        public async Task WarnCommand(
            InteractionContext ctx,
            [Option("usuario", "Quem será advertido")] DiscordUser user,
            [Option("motivo", "Motivo da advertência")] string reason)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            if (user.IsBot)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Alvo inválido", "Não faz sentido advertir um bot.")));
                return;
            }

            var member = await TryGetMemberAsync(ctx, user.Id);
            if (member is not null)
            {
                var blocked = Hierarchy.Check(ctx.Guild!, ctx.Member!, member,
                    await ctx.Guild!.GetMemberAsync(ctx.Client.CurrentUser.Id));
                if (blocked is not null)
                {
                    await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(blocked));
                    return;
                }
            }

            var warning = new Warning
            {
                id = WarningStore.NewId(),
                guildId = ctx.Guild!.Id,
                userId = user.Id,
                moderatorId = ctx.User.Id,
                moderatorTag = ctx.User.UsernameWithDiscriminator,
                reason = Embeds.Trim(reason, 500),
                createdAtUtc = DateTimeOffset.UtcNow
            };

            var total = await WarningStore.Instance.UpdateAsync(file =>
            {
                file.warnings.Add(warning);
                return file.warnings.Count(w => w.guildId == warning.guildId && w.userId == warning.userId);
            });

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Advertência registrada",
                    $"{user.Mention} agora tem **{total}** advertência(s).\nId desta: `{warning.id}`")));

            var config = new JSONReader();
            await config.ReadJSON();
            await ModerationLog.RecordAsync(ctx.Client, config, "Advertência", user, ctx.User, reason,
                $"Id `{warning.id}` — total de {total}");
        }

        [SlashCommand("warnings", "Lista as advertências de um usuário")]
        [ApplicationCommandRequireUserPermissions(Permissions.ModerateMembers)]
        public async Task WarningsCommand(
            InteractionContext ctx,
            [Option("usuario", "De quem ver o histórico")] DiscordUser user,
            [Option("pagina", "Página da lista, começando em 1")] long page = 1)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var all = await WarningStore.Instance.ListAsync(ctx.Guild!.Id, user.Id);

            if (all.Count == 0)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Info("Sem advertências", $"{user.Mention} não tem nenhuma advertência registrada.")));
                return;
            }

            var pageCount = (all.Count + PageSize - 1) / PageSize;
            var current = (int)Math.Clamp(page, 1, pageCount);

            var lines = all
                .Skip((current - 1) * PageSize)
                .Take(PageSize)
                .Select(w =>
                    $"`{w.id}` — <t:{w.createdAtUtc.ToUnixTimeSeconds()}:d> por <@{w.moderatorId}>\n" +
                    $"> {Embeds.Trim(w.reason, 200)}");

            var embed = new DiscordEmbedBuilder()
                .WithTitle($"Advertências de {user.UsernameWithDiscriminator}")
                .WithDescription(string.Join("\n\n", lines))
                .WithColor(DiscordColor.Orange)
                .WithThumbnail(user.AvatarUrl)
                .WithFooter($"Página {current}/{pageCount} — {all.Count} advertência(s) no total");

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(embed));
        }

        [SlashCommand("delwarn", "Remove uma advertência pelo id")]
        [ApplicationCommandRequireUserPermissions(Permissions.ModerateMembers)]
        public async Task DelWarnCommand(
            InteractionContext ctx,
            [Option("id", "O id mostrado no /warnings")] string id)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var wanted = (id ?? string.Empty).Trim();
            var guildId = ctx.Guild!.Id;

            var removed = await WarningStore.Instance.UpdateAsync(file =>
            {
                // Preso ao servidor de propósito: um id de outro servidor não pode
                // ser apagado a partir daqui.
                var target = file.warnings.FirstOrDefault(w =>
                    w.guildId == guildId && string.Equals(w.id, wanted, StringComparison.OrdinalIgnoreCase));

                if (target is null)
                    return null;

                file.warnings.Remove(target);
                return target;
            });

            if (removed is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Id não encontrado",
                        $"Nenhuma advertência com id `{Embeds.Trim(wanted, 40)}` neste servidor. Confira em `/warnings`.")));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Advertência removida", $"A advertência `{removed.id}` de <@{removed.userId}> foi apagada.")));

            var config = new JSONReader();
            await config.ReadJSON();
            var target = await ctx.Client.GetUserAsync(removed.userId);
            await ModerationLog.RecordAsync(ctx.Client, config, "Advertência removida", target, ctx.User,
                removed.reason, $"Id `{removed.id}`, registrada originalmente por <@{removed.moderatorId}>");
        }

        private static async Task<DiscordMember?> TryGetMemberAsync(InteractionContext ctx, ulong userId)
        {
            try
            {
                return await ctx.Guild!.GetMemberAsync(userId);
            }
            catch
            {
                return null;
            }
        }
    }
}
