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
                    Embeds.Error("Falha ao banir", Embeds.Trim(ex.Message, 500))));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Banido", $"{user.Mention} foi banido.")));

            await ModerationLog.RecordAsync(ctx.Client, ctx.Guild!.Id, "Banimento", user, ctx.User, reason,
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

            var member = await Hierarchy.TryGetMemberAsync(ctx, user.Id);
            if (member is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Fora do servidor", $"{user.Mention} não está neste servidor.")));
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
                    Embeds.Error("Falha ao expulsar", Embeds.Trim(ex.Message, 500))));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Expulso", $"{user.Mention} foi expulso.")));

            await ModerationLog.RecordAsync(ctx.Client, ctx.Guild!.Id, "Expulsão", user, ctx.User, reason);
        }

        // Cooldown de 3 segundos por usuário — é o número da documentação que
        // este comando segue. É o único comando de moderação com cooldown aqui:
        // os vizinhos não têm, e sem esta nota o (1, 3) pareceria arbitrário.
        [SlashCommand("softban", "Bane e desbane na hora, apagando as mensagens do usuário", (long)Permissions.BanMembers)]
        [ApplicationCommandRequireUserPermissions(Permissions.BanMembers)]
        [ApplicationCommandRequireBotPermissions(Permissions.BanMembers)]
        [SlashCommandCooldown(1, 3, CooldownBucketType.User)]
        public async Task SoftbanCommand(
            InteractionContext ctx,
            [Option("usuario", "Quem levará o softban")] DiscordUser user,
            [Option("motivo", "Motivo do softban")] string? reason = null)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            // Exige o membro presente, como o /kick e ao contrário do /ban.
            // Softban é uma expulsão com limpeza; em quem já saiu não há nada
            // para expulsar, e o unban logo em seguida devolve o servidor ao
            // estado exato de antes. Quem quiser só apagar o histórico de alguém
            // que já foi embora tem o /ban com apagar_dias, que é um banimento
            // assumido e não uma expulsão que não expulsou ninguém.
            var member = await Hierarchy.TryGetMemberAsync(ctx, user.Id);
            if (member is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Fora do servidor", $"{user.Mention} não está neste servidor.")));
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
                    Embeds.Error("Falha ao aplicar o softban", Embeds.Trim(result.Error, 500))));
                return;
            }

            if (result.Outcome == Softban.SoftbanOutcome.UnbanFailed)
            {
                // O banimento ficou de pé. Dizer só "falhou" aqui seria mentir
                // sobre o estado do servidor: alguém precisa ir desfazer à mão.
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Softban pela metade",
                        $"{user.Mention} foi banido e as mensagens foram apagadas, mas o desbanimento **falhou** — " +
                        "ele continua banido. Desfaça o banimento à mão.\n\n" +
                        // Sem crase em volta: a mensagem da API vem de fora e uma
                        // crase dentro dela quebraria o bloco de codigo, engolindo
                        // justamente o texto que explica o que deu errado.
                        $"Detalhe: {Embeds.Trim(result.Error, 400)}")));

                // Registrado mesmo assim, e de propósito: um estado errado é
                // justamente o que precisa ficar anotado em algum lugar.
                await ModerationLog.RecordAsync(ctx.Client, ctx.Guild!.Id, "Softban", user, ctx.User, reason,
                    "⚠️ O desbanimento falhou — o usuário continua BANIDO e precisa ser desbanido à mão.");
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Softban aplicado",
                    $"{user.Mention} foi banido e desbanido na hora. As mensagens dos últimos 7 dias " +
                    "foram apagadas e ele pode voltar por convite.")));

            await ModerationLog.RecordAsync(ctx.Client, ctx.Guild!.Id, "Softban", user, ctx.User, reason,
                "Mensagens dos últimos 7 dias apagadas. O usuário pode voltar por convite.");
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

            var member = await Hierarchy.TryGetMemberAsync(ctx, user.Id);
            if (member is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Fora do servidor", $"{user.Mention} não está neste servidor.")));
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
                    Embeds.Error("Falha ao silenciar", Embeds.Trim(ex.Message, 500))));
                return;
            }

            var describe = DurationParser.Describe(span);
            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Silenciado", $"{user.Mention} ficou silenciado por {describe}.")));

            await ModerationLog.RecordAsync(ctx.Client, ctx.Guild!.Id, "Silenciamento", user, ctx.User, reason,
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

            var member = await Hierarchy.TryGetMemberAsync(ctx, user.Id);
            if (member is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Fora do servidor", $"{user.Mention} não está neste servidor.")));
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
                    Embeds.Error("Falha ao liberar", Embeds.Trim(ex.Message, 500))));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Liberado", $"{user.Mention} não está mais silenciado.")));

            await ModerationLog.RecordAsync(ctx.Client, ctx.Guild!.Id, "Silenciamento removido", user, ctx.User, reason);
        }

        // ReadMessageHistory alem de ManageMessages: o GetMessagesAsync abaixo
        // precisa dela, e sem declara-la aqui a falta de permissao virava excecao
        // crua da API no meio da execucao - exatamente o que o RequireBotPermissions
        // existe para evitar.
        [SlashCommand("purge", "Apaga mensagens recentes do canal", (long)Permissions.ManageMessages)]
        [ApplicationCommandRequireUserPermissions(Permissions.ManageMessages)]
        [ApplicationCommandRequireBotPermissions(Permissions.ManageMessages | Permissions.ReadMessageHistory)]
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
                        Embeds.Info("Nada a apagar",
                            "Nenhuma mensagem dos últimos 14 dias bateu com o filtro. O Discord não deixa apagar em lote mensagem mais antiga que isso.")));
                    return;
                }

                // Uma mensagem so nao pode ir pelo bulk delete: o endpoint exige
                // de 2 a 100 ids e responde 400 com um unico, o que virava
                // "Falha ao apagar" para o caso mais banal - /purge quantidade:1,
                // ou um filtro de usuario que casou com so uma mensagem.
                if (target.Count == 1)
                    await target[0].DeleteAsync(Reason(ctx, null));
                else
                    await ctx.Channel.DeleteMessagesAsync(target, Reason(ctx, null));

                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Ok("Limpeza concluída",
                        $"{target.Count} mensagem(ns) apagada(s)" + (user is null ? "." : $" de {user.Mention}."))));

                // Sem filtro de usuario nao existe "alvo": passar ctx.User aqui
                // fazia o embed dizer "Usuario: @mod / Moderador: @mod", como se
                // o moderador tivesse feito uma limpeza contra si mesmo.
                await ModerationLog.RecordAsync(ctx.Client, ctx.Guild!.Id, "Limpeza de mensagens",
                    user, ctx.User, null,
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

        // Exige ManageRoles de quem usa, e nao so ManageChannels: o que o comando
        // faz e escrever um permission overwrite, e o Discord pede Manage Roles
        // (Gerenciar Permissoes) para isso. Pedindo so ManageChannels, o bot
        // emprestava a PROPRIA permissao de cargos para alguem executar uma acao
        // que aquela pessoa nao pode fazer a mao - e o defaultMemberPermissions
        // ainda anunciava a permissao errada a quem configura a integracao.
        [SlashCommand("lock", "Impede o @everyone de enviar mensagens no canal",
            (long)(Permissions.ManageChannels | Permissions.ManageRoles))]
        [ApplicationCommandRequireUserPermissions(Permissions.ManageChannels | Permissions.ManageRoles)]
        [ApplicationCommandRequireBotPermissions(Permissions.ManageRoles)]
        public Task LockCommand(InteractionContext ctx,
            [Option("motivo", "Motivo do fechamento")] string? reason = null) =>
            SetLockAsync(ctx, locked: true, reason);

        [SlashCommand("unlock", "Devolve ao @everyone o envio de mensagens no canal",
            (long)(Permissions.ManageChannels | Permissions.ManageRoles))]
        [ApplicationCommandRequireUserPermissions(Permissions.ManageChannels | Permissions.ManageRoles)]
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
                    Embeds.Error("Falha ao alterar o canal", Embeds.Trim(ex.Message, 500))));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(locked
                ? Embeds.Ok("Canal fechado", $"O @everyone não pode mais falar em {ctx.Channel.Mention}.")
                : Embeds.Ok("Canal reaberto",
                    $"A proibição de falar em {ctx.Channel.Mention} foi retirada. Se o canal tinha " +
                    "uma permissão de escrita concedida explicitamente ao @everyone antes do /lock, " +
                    "confira as permissões: ela precisa ser reposta à mão.")));

            // target = null: um lock/unlock nao tem usuario-alvo. Passar ctx.User
            // aqui (como antes) fazia o embed sair com "Usuário: @mod / Moderador:
            // @mod", como se o moderador tivesse agido contra si mesmo - o mesmo
            // bug que o comentario do /purge diz ter corrigido. O canal vai no
            // campo de detalhes.
            await ModerationLog.RecordAsync(ctx.Client, ctx.Guild!.Id,
                locked ? "Canal fechado" : "Canal reaberto",
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
