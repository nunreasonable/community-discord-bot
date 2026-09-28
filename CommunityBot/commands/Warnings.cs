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

        [SlashCommand("warn", "Records a warning for a user", (long)Permissions.ModerateMembers)]
        [ApplicationCommandRequireUserPermissions(Permissions.ModerateMembers)]
        public async Task WarnCommand(
            InteractionContext ctx,
            [Option("user", "Who to warn")] DiscordUser user,
            [Option("reason", "Reason for the warning")] string reason)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            if (user.IsBot)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Invalid target", "There's no point in warning a bot.")));
                return;
            }

            // Recusa quando o alvo nao esta no servidor, igual a /kick e /timeout.
            // Sem isto, um moderador com apenas ModerateMembers advertia alguem que
            // saiu temporariamente - inclusive um admin acima dele -, e a
            // advertencia (indexada por userId) reaparecia quando a pessoa voltava,
            // pulando a checagem de hierarquia que so da para fazer com o membro
            // presente.
            var member = await Hierarchy.TryGetMemberAsync(ctx, user.Id);
            if (member is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Not in this server", $"{user.Mention} is not in this server.")));
                return;
            }

            var blocked = Hierarchy.Check(ctx.Guild!, ctx.Member!, member,
                ctx.Guild!.CurrentMember);
            if (blocked is not null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(blocked));
                return;
            }

            var warning = new Warning
            {
                guildId = ctx.Guild!.Id,
                userId = user.Id,
                moderatorId = ctx.User.Id,
                moderatorTag = ctx.User.UsernameWithDiscriminator,
                reason = Embeds.Trim(reason, 500),
                createdAtUtc = DateTimeOffset.UtcNow
            };

            var total = await WarningStore.Instance.UpdateAsync(edit =>
            {
                // Id gerado aqui dentro, com o arquivo em maos: garante unicidade
                // sem corrida (ver WarningStore.NewUniqueId).
                warning.id = WarningStore.NewUniqueId(edit.File);
                edit.File.warnings.Add(warning);
                edit.MarkChanged();
                return edit.File.warnings.Count(w => w.guildId == warning.guildId && w.userId == warning.userId);
            });

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Warning recorded",
                    $"{user.Mention} now has **{total}** warning(s).\nThis warning's ID: `{warning.id}`")));

            await ModerationLog.RecordAsync(ctx.Client, ctx.Guild!.Id, "Warning", user, ctx.User, reason,
                $"ID `{warning.id}` — {total} in total");
        }

        [SlashCommand("warnings", "Lists a user's warnings", (long)Permissions.ModerateMembers)]
        [ApplicationCommandRequireUserPermissions(Permissions.ModerateMembers)]
        public async Task WarningsCommand(
            InteractionContext ctx,
            [Option("user", "Whose history to view")] DiscordUser user,
            [Option("page", "Page of the list, starting at 1")] long page = 1)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var all = await WarningStore.Instance.ListAsync(ctx.Guild!.Id, user.Id);

            if (all.Count == 0)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Info("No warnings", $"{user.Mention} has no recorded warnings.")));
                return;
            }

            var pageCount = (all.Count + PageSize - 1) / PageSize;
            var current = (int)Math.Clamp(page, 1, pageCount);

            var lines = all
                .Skip((current - 1) * PageSize)
                .Take(PageSize)
                .Select(w =>
                    $"`{w.id}` — <t:{w.createdAtUtc.ToUnixTimeSeconds()}:d> by <@{w.moderatorId}>\n" +
                    $"> {Embeds.Trim(w.reason, 200)}");

            var embed = new DiscordEmbedBuilder()
                .WithTitle($"Warnings for {user.UsernameWithDiscriminator}")
                .WithDescription(string.Join("\n\n", lines))
                .WithColor(DiscordColor.Orange)
                .WithThumbnail(user.AvatarUrl)
                .WithFooter($"Page {current}/{pageCount} — {all.Count} warning(s) in total");

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(embed));
        }

        [SlashCommand("delwarn", "Removes a warning by its ID", (long)Permissions.ModerateMembers)]
        [ApplicationCommandRequireUserPermissions(Permissions.ModerateMembers)]
        public async Task DelWarnCommand(
            InteractionContext ctx,
            [Option("id", "The ID shown in /warnings")] string id)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var wanted = id.Trim();
            var guildId = ctx.Guild!.Id;

            /*
             * Hierarquia ANTES de apagar.
             *
             * O /warn passa por Hierarchy.Check; o /delwarn nao passava por nada,
             * e qualquer um com ModerateMembers podia apagar qualquer advertencia
             * do servidor - inclusive uma que um administrador acima dele
             * escreveu, e inclusive as escritas contra ele mesmo. Apagar o
             * registro de uma punicao e um ato de moderacao como tirar a punicao,
             * que e o mesmo argumento ja aceito no /untimeout.
             *
             * O que se compara aqui e o MODERADOR QUE ESCREVEU, nao o advertido:
             * apagar uma advertencia nao e um ato contra quem a levou - e um ato
             * contra o registro de quem a aplicou. Mexer no que um superior
             * escreveu exige estar acima dele. A propria advertencia continua
             * livre para o autor dela apagar.
             */
            var existing = (await WarningStore.Instance.ReadAsync()).warnings
                .FirstOrDefault(w => w.guildId == guildId &&
                                     string.Equals(w.id, wanted, StringComparison.OrdinalIgnoreCase));

            if (existing is not null && existing.moderatorId != ctx.User.Id)
            {
                // `issuer is null` RECUSA, e nao libera: null aqui quer dizer que
                // quem aplicou saiu do servidor, e sem o cargo dele nao da para
                // dizer quem estava acima de quem. Liberando, bastava um admin
                // deixar o servidor para as advertencias que ele escreveu
                // virarem apagaveis por qualquer moderador - lavagem de ficha
                // exatamente na hora em que ninguem esta olhando.
                var issuer = await Hierarchy.TryGetMemberAsync(ctx, existing.moderatorId);
                if (issuer is null)
                {
                    await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                        Embeds.Error("Role hierarchy unavailable",
                            "Whoever issued this warning is no longer in the server, so I can't check " +
                            "whether you outrank them. Only someone with an administrator role can sort this out manually.")));
                    return;
                }

                if (!Hierarchy.Outranks(ctx.Guild!, ctx.Member!, issuer))
                {
                    await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                        Embeds.Error("Role hierarchy",
                            $"This warning was issued by {issuer.Mention}, whose role is equal to or higher " +
                            "than yours. Only someone above them — or they themselves — can delete it.")));
                    return;
                }
            }

            var removed = await WarningStore.Instance.UpdateAsync(edit =>
            {
                // Preso ao servidor de propósito: um id de outro servidor não pode
                // ser apagado a partir daqui.
                var target = edit.File.warnings.FirstOrDefault(w =>
                    w.guildId == guildId && string.Equals(w.id, wanted, StringComparison.OrdinalIgnoreCase));

                if (target is null)
                    return null;

                edit.File.warnings.Remove(target);
                edit.MarkChanged();
                return target;
            });

            if (removed is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("ID not found",
                        $"No warning with ID `{Embeds.Trim(wanted, 40)}` in this server. Check `/warnings`.")));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Warning removed", $"Warning `{removed.id}` for <@{removed.userId}> was deleted.")));

            // Tudo o que vem DEPOIS do "sucesso" acima e best-effort. O
            // GetUserAsync em especial: a conta pode ter sido apagada, e uma
            // excecao aqui chegaria ao SlashCommandErrored, que edita a mesma
            // resposta original - o moderador veria "Command failed" numa
            // advertencia que de fato foi removida.
            try
            {
                var target = await ctx.Client.GetUserAsync(removed.userId);
                await ModerationLog.RecordAsync(ctx.Client, ctx.Guild!.Id, "Warning removed", target, ctx.User,
                    removed.reason, $"ID `{removed.id}`, originally issued by <@{removed.moderatorId}>");
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[warnings] falha ao registrar a remocao de {removed.id}: {ex.Message}");
            }
        }
    }
}
