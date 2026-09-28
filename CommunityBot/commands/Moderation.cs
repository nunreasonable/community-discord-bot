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
using DisCatSharp.Enums.Core;

namespace CommunityBot.commands
{
    /// <summary>
    /// Comandos de moderação.
    ///
    /// O portão é a permissão do PROPRIO Discord, com
    /// ApplicationCommandRequireUserPermissions: quem não pode banir nem vê o
    /// /ban na lista. Isso aproveita os cargos que o servidor já tem, em vez de
    /// exigir um cargo configurado à mão.
    ///
    /// RequireBotPermissions existe pelo outro lado: sem ele, faltar permissão ao
    /// bot vira uma exceção crua da API no meio da execução.
    /// </summary>
    [ApplicationCommandRequireGuild]
    internal class Moderation : ApplicationCommandsModule
    {
        [SlashCommand("ban", "Bans a user from the server", (long)Permissions.BanMembers)]
        [ApplicationCommandRequireUserPermissions(Permissions.BanMembers)]
        [ApplicationCommandRequireBotPermissions(Permissions.BanMembers)]
        public async Task BanCommand(
            InteractionContext ctx,
            [Option("user", "Who to ban")] DiscordUser user,
            [Option("reason", "Reason for the ban")] string? reason = null,
            [Option("delete_days", "Delete their messages from the last N days (0 to 7)")] long deleteDays = 0)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            if (deleteDays is < 0 or > 7)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Invalid value", "`delete_days` must be between 0 and 7.")));
                return;
            }

            // A API do Discord conta este parametro em SEGUNDOS, nao em dias - o
            // proprio DisCatSharp o chama de deleteMessageSeconds, com maximo de
            // 604800 (7 dias). Passar `deleteDays` cru apagava 7 SEGUNDOS de
            // mensagens quando o moderador pedia 7 dias, e o embed de confirmacao
            // ainda afirmava que tinha apagado a semana inteira.
            var deleteMessageSeconds = (int)(deleteDays * 86400);

            // O alvo pode nao ser membro do servidor - banir alguem que ja saiu e
            // legitimo, e por isso a hierarquia so e checada quando ele esta la.
            var member = await Hierarchy.TryGetMemberAsync(ctx, user.Id);
            if (member is not null)
            {
                var blocked = Hierarchy.Check(ctx.Guild!, ctx.Member!, member, ctx.Guild!.CurrentMember);
                if (blocked is not null)
                {
                    await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(blocked));
                    return;
                }
            }

            try
            {
                await ctx.Guild!.BanMemberAsync(user.Id, deleteMessageSeconds, Reason(ctx, reason));
            }
            catch (Exception ex)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Failed to ban", Embeds.Trim(ex.Message, 500))));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Banned", $"{user.Mention} was banned.")));

            await ModerationLog.RecordAsync(ctx.Client, ctx.Guild!.Id, "Ban", user, ctx.User, reason,
                deleteDays > 0 ? $"Deleted messages from the last {deleteDays} day(s)." : null);
        }

        [SlashCommand("kick", "Kicks a user from the server", (long)Permissions.KickMembers)]
        [ApplicationCommandRequireUserPermissions(Permissions.KickMembers)]
        [ApplicationCommandRequireBotPermissions(Permissions.KickMembers)]
        public async Task KickCommand(
            InteractionContext ctx,
            [Option("user", "Who to kick")] DiscordUser user,
            [Option("reason", "Reason for the kick")] string? reason = null)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var member = await Hierarchy.TryGetMemberAsync(ctx, user.Id);
            if (member is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Not in this server", $"{user.Mention} is not in this server.")));
                return;
            }

            var blocked = Hierarchy.Check(ctx.Guild!, ctx.Member!, member, ctx.Guild!.CurrentMember);
            if (blocked is not null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(blocked));
                return;
            }

            try
            {
                await member.RemoveAsync(Reason(ctx, reason));
            }
            catch (Exception ex)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Failed to kick", Embeds.Trim(ex.Message, 500))));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Kicked", $"{user.Mention} was kicked.")));

            await ModerationLog.RecordAsync(ctx.Client, ctx.Guild!.Id, "Kick", user, ctx.User, reason);
        }

        // Cooldown de 3 segundos por usuário — é o número da documentação que
        // este comando segue. É o único comando de moderação com cooldown aqui:
        // os vizinhos não têm, e sem esta nota o (1, 3) pareceria arbitrário.
        [SlashCommand("softban", "Bans and immediately unbans a user, deleting their messages", (long)Permissions.BanMembers)]
        [ApplicationCommandRequireUserPermissions(Permissions.BanMembers)]
        [ApplicationCommandRequireBotPermissions(Permissions.BanMembers)]
        [SlashCommandCooldown(1, 3, CooldownBucketType.User)]
        public async Task SoftbanCommand(
            InteractionContext ctx,
            [Option("user", "Who to softban")] DiscordUser user,
            [Option("reason", "Reason for the softban")] string? reason = null)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            // Exige o membro presente, como o /kick e ao contrário do /ban.
            // Softban é uma expulsão com limpeza; em quem já saiu não há nada
            // para expulsar, e o unban logo em seguida devolve o servidor ao
            // estado exato de antes. Quem quiser só apagar o histórico de alguém
            // que já foi embora tem o /ban com delete_days, que é um banimento
            // assumido e não uma expulsão que não expulsou ninguém.
            var member = await Hierarchy.TryGetMemberAsync(ctx, user.Id);
            if (member is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Not in this server", $"{user.Mention} is not in this server.")));
                return;
            }

            var blocked = Hierarchy.Check(ctx.Guild!, ctx.Member!, member, ctx.Guild!.CurrentMember);
            if (blocked is not null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(blocked));
                return;
            }

            var result = await Softban.ApplyAsync(ctx.Guild!, user.Id, Reason(ctx, reason));

            if (result.Outcome == Softban.SoftbanOutcome.BanFailed)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Failed to apply the softban", Embeds.Trim(result.Error, 500))));
                return;
            }

            if (result.Outcome == Softban.SoftbanOutcome.UnbanFailed)
            {
                // O banimento ficou de pé. Dizer só "falhou" aqui seria mentir
                // sobre o estado do servidor: alguém precisa ir desfazer à mão.
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Softban only half done",
                        $"{user.Mention} was banned and their messages were deleted, but the unban **failed** — " +
                        "they are still banned. Remove the ban manually.\n\n" +
                        // Sem crase em volta: a mensagem da API vem de fora e uma
                        // crase dentro dela quebraria o bloco de codigo, engolindo
                        // justamente o texto que explica o que deu errado.
                        $"Details: {Embeds.Trim(result.Error, 400)}")));

                // Registrado mesmo assim, e de propósito: um estado errado é
                // justamente o que precisa ficar anotado em algum lugar.
                await ModerationLog.RecordAsync(ctx.Client, ctx.Guild!.Id, "Softban", user, ctx.User, reason,
                    "⚠️ The unban failed — the user is still BANNED and must be unbanned manually.");
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Softban applied",
                    $"{user.Mention} was banned and immediately unbanned. Their messages from the last 7 days " +
                    "were deleted, and they can rejoin with an invite.")));

            await ModerationLog.RecordAsync(ctx.Client, ctx.Guild!.Id, "Softban", user, ctx.User, reason,
                "Deleted messages from the last 7 days. The user can rejoin with an invite.");
        }

        [SlashCommand("timeout", "Times out a user for a while", (long)Permissions.ModerateMembers)]
        [ApplicationCommandRequireUserPermissions(Permissions.ModerateMembers)]
        [ApplicationCommandRequireBotPermissions(Permissions.ModerateMembers)]
        public async Task TimeoutCommand(
            InteractionContext ctx,
            [Option("user", "Who to time out")] DiscordUser user,
            [Option("duration", "E.g. 10m, 2h30m, 1d (max 28 days)")] string duration,
            [Option("reason", "Reason for the timeout")] string? reason = null)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            if (!DurationParser.TryParse(duration, out var span, out var error))
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Invalid duration", error!)));
                return;
            }

            var member = await Hierarchy.TryGetMemberAsync(ctx, user.Id);
            if (member is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Not in this server", $"{user.Mention} is not in this server.")));
                return;
            }

            var blocked = Hierarchy.Check(ctx.Guild!, ctx.Member!, member, ctx.Guild!.CurrentMember);
            if (blocked is not null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(blocked));
                return;
            }

            try
            {
                await member.TimeoutAsync(DateTimeOffset.UtcNow + span, Reason(ctx, reason));
            }
            catch (Exception ex)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Failed to time out", Embeds.Trim(ex.Message, 500))));
                return;
            }

            var describe = DurationParser.Describe(span);
            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Timed out", $"{user.Mention} is timed out for {describe}.")));

            await ModerationLog.RecordAsync(ctx.Client, ctx.Guild!.Id, "Timeout", user, ctx.User, reason,
                $"Duration: {describe}");
        }

        [SlashCommand("untimeout", "Removes a user's timeout", (long)Permissions.ModerateMembers)]
        [ApplicationCommandRequireUserPermissions(Permissions.ModerateMembers)]
        [ApplicationCommandRequireBotPermissions(Permissions.ModerateMembers)]
        public async Task UntimeoutCommand(
            InteractionContext ctx,
            [Option("user", "Whose timeout to remove")] DiscordUser user,
            [Option("reason", "Reason")] string? reason = null)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var member = await Hierarchy.TryGetMemberAsync(ctx, user.Id);
            if (member is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Not in this server", $"{user.Mention} is not in this server.")));
                return;
            }

            // Tirar um silenciamento e ato de moderacao como qualquer outro: sem
            // esta checagem, um mod com ModerateMembers desfazia o silenciamento
            // que um admin acima dele tinha aplicado. O README ja afirmava que
            // "toda punicao passa por uma checagem de hierarquia antes"; aqui e
            // /purge eram as duas excecoes que desmentiam a frase.
            var blocked = Hierarchy.Check(ctx.Guild!, ctx.Member!, member, ctx.Guild!.CurrentMember);
            if (blocked is not null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(blocked));
                return;
            }

            try
            {
                await member.RemoveTimeoutAsync(Reason(ctx, reason));
            }
            catch (Exception ex)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Failed to remove the timeout", Embeds.Trim(ex.Message, 500))));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Timeout removed", $"{user.Mention} is no longer timed out.")));

            await ModerationLog.RecordAsync(ctx.Client, ctx.Guild!.Id, "Timeout removed", user, ctx.User, reason);
        }

        // ReadMessageHistory alem de ManageMessages: o GetMessagesAsync abaixo
        // precisa dela, e sem declara-la aqui a falta de permissao virava excecao
        // crua da API no meio da execucao - exatamente o que o RequireBotPermissions
        // existe para evitar.
        [SlashCommand("purge", "Deletes recent messages in the channel", (long)Permissions.ManageMessages)]
        [ApplicationCommandRequireUserPermissions(Permissions.ManageMessages)]
        [ApplicationCommandRequireBotPermissions(Permissions.ManageMessages | Permissions.ReadMessageHistory)]
        public async Task PurgeCommand(
            InteractionContext ctx,
            [Option("amount", "How many messages to check (1 to 100)")] long amount,
            [Option("user", "Only delete messages from this user")] DiscordUser? user = null)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            if (amount is < 1 or > 100)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Invalid value", "The amount must be between 1 and 100.")));
                return;
            }

            // Com filtro de usuario, o /purge e uma acao CONTRA alguem, e passa
            // pela mesma hierarquia do resto. Sem filtro e limpeza de canal, que
            // nao tem alvo - por isso a checagem so acontece quando ha um.
            if (user is not null)
            {
                var member = await Hierarchy.TryGetMemberAsync(ctx, user.Id);
                if (member is not null)
                {
                    var blocked = Hierarchy.Check(ctx.Guild!, ctx.Member!, member, ctx.Guild!.CurrentMember);
                    if (blocked is not null)
                    {
                        await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(blocked));
                        return;
                    }
                }
            }

            try
            {
                var messages = await ctx.Channel.GetMessagesAsync((int)amount);

                // A API recusa apagar em lote mensagem com mais de 14 dias, e uma
                // unica dessas no meio derruba a chamada inteira - por isso ela e
                // filtrada aqui em vez de descobrirmos no erro.
                var cutoff = DateTimeOffset.UtcNow.AddDays(-14);
                var target = messages
                    .Where(m => m.CreationTimestamp > cutoff)
                    .Where(m => user is null || m.Author.Id == user.Id)
                    .ToList();

                if (target.Count == 0)
                {
                    await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                        Embeds.Info("Nothing to delete",
                            "No messages from the last 14 days matched the filter. Discord doesn't allow bulk-deleting messages older than that.")));
                    return;
                }

                // Uma mensagem so nao pode ir pelo bulk delete: o endpoint exige
                // de 2 a 100 ids e responde 400 com um unico, o que virava
                // "Failed to delete" para o caso mais banal - /purge amount:1,
                // ou um filtro de usuario que casou com so uma mensagem.
                if (target.Count == 1)
                    await target[0].DeleteAsync(Reason(ctx, null));
                else
                    await ctx.Channel.DeleteMessagesAsync(target, Reason(ctx, null));

                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Ok("Purge complete",
                        $"{target.Count} message(s) deleted" + (user is null ? "." : $" from {user.Mention}."))));

                // Sem filtro de usuario nao existe "alvo": passar ctx.User aqui
                // fazia o embed dizer "User: @mod / Moderator: @mod", como se
                // o moderador tivesse feito uma limpeza contra si mesmo.
                await ModerationLog.RecordAsync(ctx.Client, ctx.Guild!.Id, "Message purge",
                    user, ctx.User, null,
                    $"{target.Count} message(s) in {ctx.Channel.Mention}");
            }
            catch (Exception ex)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Failed to delete", Embeds.Trim(ex.Message, 500))));
            }
        }

        [SlashCommand("slowmode", "Sets the channel's slowmode", (long)Permissions.ManageChannels)]
        [ApplicationCommandRequireUserPermissions(Permissions.ManageChannels)]
        [ApplicationCommandRequireBotPermissions(Permissions.ManageChannels)]
        public async Task SlowmodeCommand(
            InteractionContext ctx,
            [Option("seconds", "Delay between messages (0 turns it off, max 21600)")] long seconds)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            if (seconds is < 0 or > 21600)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Invalid value", "The delay must be between 0 and 21600 seconds (6 hours).")));
                return;
            }

            try
            {
                await ctx.Channel.ModifyAsync(c => c.PerUserRateLimit = (int)seconds);
            }
            catch (Exception ex)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Failed to apply slowmode", Embeds.Trim(ex.Message, 500))));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(seconds == 0
                ? Embeds.Ok("Slowmode off", $"{ctx.Channel.Mention} is back to normal.")
                : Embeds.Ok("Slowmode on", $"One message every {seconds} second(s) in {ctx.Channel.Mention}.")));
        }

        // Exige ManageRoles de quem usa, e nao so ManageChannels: o que o comando
        // faz e escrever um permission overwrite, e o Discord pede Manage Roles
        // (Gerenciar Permissoes) para isso. Pedindo so ManageChannels, o bot
        // emprestava a PROPRIA permissao de cargos para alguem executar uma acao
        // que aquela pessoa nao pode fazer a mao - e o defaultMemberPermissions
        // ainda anunciava a permissao errada a quem configura a integracao.
        [SlashCommand("lock", "Stops @everyone from sending messages in the channel",
            (long)(Permissions.ManageChannels | Permissions.ManageRoles))]
        [ApplicationCommandRequireUserPermissions(Permissions.ManageChannels | Permissions.ManageRoles)]
        [ApplicationCommandRequireBotPermissions(Permissions.ManageRoles)]
        public Task LockCommand(InteractionContext ctx,
            [Option("reason", "Reason for locking")] string? reason = null) =>
            SetLockAsync(ctx, locked: true, reason);

        [SlashCommand("unlock", "Lets @everyone send messages in the channel again",
            (long)(Permissions.ManageChannels | Permissions.ManageRoles))]
        [ApplicationCommandRequireUserPermissions(Permissions.ManageChannels | Permissions.ManageRoles)]
        [ApplicationCommandRequireBotPermissions(Permissions.ManageRoles)]
        public Task UnlockCommand(InteractionContext ctx,
            [Option("reason", "Reason for unlocking")] string? reason = null) =>
            SetLockAsync(ctx, locked: false, reason);

        /// <summary>
        /// Fecha e reabre o canal pelo mesmo caminho: a unica diferenca e o lado
        /// para onde o SendMessages vai.
        /// </summary>
        private static async Task SetLockAsync(InteractionContext ctx, bool locked, string? reason)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            try
            {
                var everyone = ctx.Guild!.EveryoneRole;
                if (everyone is null)
                {
                    await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                        Embeds.Error("Role not found", "I couldn't read this server's @everyone role.")));
                    return;
                }

                var existing = ctx.Channel.PermissionOverwrites
                    .FirstOrDefault(o => o.Id == everyone.Id);

                // Preserva o resto do overwrite: mexer so no bit de SendMessages
                // evita apagar permissoes que alguem configurou a mao no canal.
                var allow = existing?.Allowed ?? Permissions.None;
                var deny = existing?.Denied ?? Permissions.None;

                if (locked)
                {
                    allow &= ~Permissions.SendMessages;
                    deny |= Permissions.SendMessages;
                }
                else
                {
                    deny &= ~Permissions.SendMessages;
                }

                /*
                 * LIMITACAO CONHECIDA, deliberada.
                 *
                 * O /lock faz duas coisas - tira o allow explicito e poe o deny -
                 * e o /unlock desfaz so a segunda. Num canal onde o @everyone
                 * tinha SendMessages explicitamente CONCEDIDO, sobrepondo uma
                 * negacao no nivel do servidor, o ciclo lock/unlock deixa o
                 * overwrite em "neutro": o @everyone continua mudo.
                 *
                 * Nao e corrigido aqui de proposito. Restaurar o allow exige
                 * lembrar o estado anterior entre duas invocacoes separadas, o
                 * que pede persistencia; e adivinhar - sempre repor o allow no
                 * unlock - ABRIRIA canais que estavam fechados de proposito
                 * antes do /lock. Entre errar concedendo e errar negando, negar
                 * e o lado certo.
                 *
                 * Nao da para simplesmente manter allow e deny juntos: o Discord
                 * aplica o deny e depois o allow no overwrite do @everyone,
                 * entao o allow venceria e o /lock nao trancaria nada.
                 */

                await ctx.Channel.AddOverwriteAsync(everyone, allow, deny, Reason(ctx, reason));
            }
            catch (Exception ex)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Failed to update the channel", Embeds.Trim(ex.Message, 500))));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(locked
                ? Embeds.Ok("Channel locked", $"@everyone can no longer send messages in {ctx.Channel.Mention}.")
                : Embeds.Ok("Channel unlocked",
                    $"The block on sending messages in {ctx.Channel.Mention} was lifted. If @everyone had " +
                    "Send Messages explicitly granted in this channel before /lock, " +
                    "check the permissions: it has to be restored manually.")));

            // target = null: um lock/unlock nao tem usuario-alvo. Passar ctx.User
            // aqui (como antes) fazia o embed sair com "User: @mod / Moderator:
            // @mod", como se o moderador tivesse agido contra si mesmo - o mesmo
            // bug que o comentario do /purge diz ter corrigido. O canal vai no
            // campo de detalhes.
            await ModerationLog.RecordAsync(ctx.Client, ctx.Guild!.Id,
                locked ? "Channel locked" : "Channel unlocked",
                null, ctx.User, reason, ctx.Channel.Mention,
                // Idem /say: trancar um canal nao tem usuario-alvo, e o rotulo
                // padrao e do /purge.
                noTargetLabel: "—");
        }

        /// <summary>
        /// Texto que vai para o Audit Log do Discord. A limpeza e o corte moram
        /// em <see cref="AuditReason"/>, que os tickets tambem usam.
        /// </summary>
        private static string Reason(InteractionContext ctx, string? reason) =>
            AuditReason.For(ctx.User.UsernameWithDiscriminator, reason);
    }
}
