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
        [SlashCommand("ban", "Bane um usuário do servidor", (long)Permissions.BanMembers)]
        [ApplicationCommandRequireUserPermissions(Permissions.BanMembers)]
        [ApplicationCommandRequireBotPermissions(Permissions.BanMembers)]
        public async Task BanCommand(
            InteractionContext ctx,
            [Option("usuario", "Quem será banido")] DiscordUser user,
            [Option("motivo", "Motivo do banimento")] string? reason = null,
            [Option("apagar_dias", "Apagar mensagens dos últimos N dias (0 a 7)")] long deleteDays = 0)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            if (deleteDays is < 0 or > 7)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Valor inválido", "`apagar_dias` precisa ficar entre 0 e 7.")));
                return;
            }

            // O alvo pode nao ser membro do servidor - banir alguem que ja saiu e
            // legitimo, e por isso a hierarquia so e checada quando ele esta la.
            var member = await TryGetMemberAsync(ctx, user.Id);
            if (member is not null)
            {
                var blocked = Hierarchy.Check(ctx.Guild!, ctx.Member!, member, await ctx.Guild!.GetMemberAsync(ctx.Client.CurrentUser.Id));
                if (blocked is not null)
                {
                    await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(blocked));
                    return;
                }
            }

            try
            {
                await ctx.Guild!.BanMemberAsync(user.Id, (int)deleteDays, Reason(ctx, reason));
            }
            catch (Exception ex)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Falha ao banir", Embeds.Trim(ex.Message, 500))));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Banido", $"{user.Mention} foi banido.")));

            var config = new JSONReader();
            await config.ReadJSON();
            await ModerationLog.RecordAsync(ctx.Client, config, "Banimento", user, ctx.User, reason,
                deleteDays > 0 ? $"Mensagens dos últimos {deleteDays} dia(s) apagadas." : null);
        }

        [SlashCommand("kick", "Expulsa um usuário do servidor", (long)Permissions.KickMembers)]
        [ApplicationCommandRequireUserPermissions(Permissions.KickMembers)]
        [ApplicationCommandRequireBotPermissions(Permissions.KickMembers)]
        public async Task KickCommand(
            InteractionContext ctx,
            [Option("usuario", "Quem será expulso")] DiscordUser user,
            [Option("motivo", "Motivo da expulsão")] string? reason = null)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var member = await TryGetMemberAsync(ctx, user.Id);
            if (member is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Fora do servidor", $"{user.Mention} não está neste servidor.")));
                return;
            }

            var blocked = Hierarchy.Check(ctx.Guild!, ctx.Member!, member, await ctx.Guild!.GetMemberAsync(ctx.Client.CurrentUser.Id));
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
                    Embeds.Error("Falha ao expulsar", Embeds.Trim(ex.Message, 500))));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Expulso", $"{user.Mention} foi expulso.")));

            var config = new JSONReader();
            await config.ReadJSON();
            await ModerationLog.RecordAsync(ctx.Client, config, "Expulsão", user, ctx.User, reason);
        }

        [SlashCommand("timeout", "Silencia um usuário por um tempo", (long)Permissions.ModerateMembers)]
        [ApplicationCommandRequireUserPermissions(Permissions.ModerateMembers)]
        [ApplicationCommandRequireBotPermissions(Permissions.ModerateMembers)]
        public async Task TimeoutCommand(
            InteractionContext ctx,
            [Option("usuario", "Quem será silenciado")] DiscordUser user,
            [Option("duracao", "Ex.: 10m, 2h30m, 1d (máximo 28 dias)")] string duration,
            [Option("motivo", "Motivo do silenciamento")] string? reason = null)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            if (!DurationParser.TryParse(duration, out var span, out var error))
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Duração inválida", error!)));
                return;
            }

            var member = await TryGetMemberAsync(ctx, user.Id);
            if (member is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Fora do servidor", $"{user.Mention} não está neste servidor.")));
                return;
            }

            var blocked = Hierarchy.Check(ctx.Guild!, ctx.Member!, member, await ctx.Guild!.GetMemberAsync(ctx.Client.CurrentUser.Id));
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
                    Embeds.Error("Falha ao silenciar", Embeds.Trim(ex.Message, 500))));
                return;
            }

            var describe = DurationParser.Describe(span);
            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Silenciado", $"{user.Mention} ficou silenciado por {describe}.")));

            var config = new JSONReader();
            await config.ReadJSON();
            await ModerationLog.RecordAsync(ctx.Client, config, "Silenciamento", user, ctx.User, reason,
                $"Duração: {describe}");
        }

        [SlashCommand("untimeout", "Remove o silenciamento de um usuário", (long)Permissions.ModerateMembers)]
        [ApplicationCommandRequireUserPermissions(Permissions.ModerateMembers)]
        [ApplicationCommandRequireBotPermissions(Permissions.ModerateMembers)]
        public async Task UntimeoutCommand(
            InteractionContext ctx,
            [Option("usuario", "Quem será liberado")] DiscordUser user,
            [Option("motivo", "Motivo")] string? reason = null)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var member = await TryGetMemberAsync(ctx, user.Id);
            if (member is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Fora do servidor", $"{user.Mention} não está neste servidor.")));
                return;
            }

            try
            {
                await member.RemoveTimeoutAsync(Reason(ctx, reason));
            }
            catch (Exception ex)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Falha ao liberar", Embeds.Trim(ex.Message, 500))));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Liberado", $"{user.Mention} não está mais silenciado.")));

            var config = new JSONReader();
            await config.ReadJSON();
            await ModerationLog.RecordAsync(ctx.Client, config, "Silenciamento removido", user, ctx.User, reason);
        }

        [SlashCommand("purge", "Apaga mensagens recentes do canal", (long)Permissions.ManageMessages)]
        [ApplicationCommandRequireUserPermissions(Permissions.ManageMessages)]
        [ApplicationCommandRequireBotPermissions(Permissions.ManageMessages)]
        public async Task PurgeCommand(
            InteractionContext ctx,
            [Option("quantidade", "Quantas mensagens olhar (1 a 100)")] long amount,
            [Option("usuario", "Apagar só as mensagens deste usuário")] DiscordUser? user = null)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            if (amount is < 1 or > 100)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Valor inválido", "A quantidade precisa ficar entre 1 e 100.")));
                return;
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
                        Embeds.Info("Nada a apagar",
                            "Nenhuma mensagem dos últimos 14 dias bateu com o filtro. O Discord não deixa apagar em lote mensagem mais antiga que isso.")));
                    return;
                }

                await ctx.Channel.DeleteMessagesAsync(target, Reason(ctx, null));

                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Ok("Limpeza concluída",
                        $"{target.Count} mensagem(ns) apagada(s)" + (user is null ? "." : $" de {user.Mention}."))));

                var config = new JSONReader();
                await config.ReadJSON();
                await ModerationLog.RecordAsync(ctx.Client, config, "Limpeza de mensagens",
                    user ?? ctx.User, ctx.User, null,
                    $"{target.Count} mensagem(ns) em {ctx.Channel.Mention}");
            }
            catch (Exception ex)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Falha ao apagar", Embeds.Trim(ex.Message, 500))));
            }
        }

        [SlashCommand("slowmode", "Define o modo lento do canal", (long)Permissions.ManageChannels)]
        [ApplicationCommandRequireUserPermissions(Permissions.ManageChannels)]
        [ApplicationCommandRequireBotPermissions(Permissions.ManageChannels)]
        public async Task SlowmodeCommand(
            InteractionContext ctx,
            [Option("segundos", "Intervalo entre mensagens (0 desliga, máximo 21600)")] long seconds)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            if (seconds is < 0 or > 21600)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Valor inválido", "O intervalo precisa ficar entre 0 e 21600 segundos (6 horas).")));
                return;
            }

            try
            {
                await ctx.Channel.ModifyAsync(c => c.PerUserRateLimit = (int)seconds);
            }
            catch (Exception ex)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Falha ao aplicar", Embeds.Trim(ex.Message, 500))));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(seconds == 0
                ? Embeds.Ok("Modo lento desligado", $"{ctx.Channel.Mention} voltou ao normal.")
                : Embeds.Ok("Modo lento ligado", $"Uma mensagem a cada {seconds} segundo(s) em {ctx.Channel.Mention}.")));
        }

        [SlashCommand("lock", "Impede o @everyone de enviar mensagens no canal", (long)Permissions.ManageChannels)]
        [ApplicationCommandRequireUserPermissions(Permissions.ManageChannels)]
        [ApplicationCommandRequireBotPermissions(Permissions.ManageRoles)]
        public Task LockCommand(InteractionContext ctx,
            [Option("motivo", "Motivo do fechamento")] string? reason = null) =>
            SetLockAsync(ctx, locked: true, reason);

        [SlashCommand("unlock", "Devolve ao @everyone o envio de mensagens no canal", (long)Permissions.ManageChannels)]
        [ApplicationCommandRequireUserPermissions(Permissions.ManageChannels)]
        [ApplicationCommandRequireBotPermissions(Permissions.ManageRoles)]
        public Task UnlockCommand(InteractionContext ctx,
            [Option("motivo", "Motivo da reabertura")] string? reason = null) =>
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
                        Embeds.Error("Cargo não encontrado", "Não consegui ler o cargo @everyone deste servidor.")));
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

                await ctx.Channel.AddOverwriteAsync(everyone, allow, deny, Reason(ctx, reason));
            }
            catch (Exception ex)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Falha ao alterar o canal", Embeds.Trim(ex.Message, 500))));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(locked
                ? Embeds.Ok("Canal fechado", $"O @everyone não pode mais falar em {ctx.Channel.Mention}.")
                : Embeds.Ok("Canal reaberto", $"O @everyone voltou a falar em {ctx.Channel.Mention}.")));

            var config = new JSONReader();
            await config.ReadJSON();
            await ModerationLog.RecordAsync(ctx.Client, config,
                locked ? "Canal fechado" : "Canal reaberto",
                ctx.User, ctx.User, reason, ctx.Channel.Mention);
        }

        /// <summary>
        /// Alvo pode ter saido do servidor. Devolve null em vez de lancar, para o
        /// comando decidir se isso e problema.
        /// </summary>
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

        /// <summary>
        /// Texto que vai para o Audit Log do Discord. Carregar quem executou faz
        /// o registro nativo continuar util mesmo se o canal de log for apagado.
        /// </summary>
        private static string Reason(InteractionContext ctx, string? reason) =>
            Embeds.Trim($"{ctx.User.UsernameWithDiscriminator}: {reason ?? "sem motivo informado"}", 400);
    }
}
