using System;
using System.Linq;
using System.Threading.Tasks;
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
    /// Sistema de tickets.
    ///
    /// O /ticket e aberto a qualquer um de proposito - e a porta de entrada, e
    /// trancar a porta com permissao derrotaria o proposito. O resto exige a
    /// permissao correspondente do Discord, como no restante do bot.
    ///
    /// Toda a logica mora em Services/Tickets.cs, porque o painel de botoes
    /// precisa executar as MESMAS acoes sem passar por comando nenhum.
    /// </summary>
    [ApplicationCommandRequireGuild]
    internal class Tickets : ApplicationCommandsModule
    {
        [SlashCommand("ticket", "Abre um ticket com a equipe")]
        [ApplicationCommandRequireBotPermissions(Permissions.ManageChannels | Permissions.ManageRoles)]
        [SlashCommandCooldown(2, 60, CooldownBucketType.User)]
        public async Task TicketCommand(
            InteractionContext ctx,
            [Option("assunto", "Em poucas palavras, do que se trata")] string assunto,
            // As escolhas vem do TicketTypeChoiceProvider, e nao de [Choice]
            // escrito a mao: era a segunda copia da lista de tipos.
            [ChoiceProvider(typeof(TicketTypeChoiceProvider))]
            [Option("tipo", "Que tipo de ticket é este")] string tipo = "duvida",
            [Option("detalhes", "Conte o que aconteceu")] string? detalhes = null)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var settings = TicketService.TryGetSettings(ctx.Guild!.Id);
            if (settings is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(TicketService.NotConfigured()));
                return;
            }

            var (channel, error) = await TicketService.OpenAsync(
                ctx.Client, ctx.Guild!, ctx.Member!, settings, tipo, assunto, detalhes);

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                error ?? Embeds.Ok("Ticket aberto", $"Continue em {channel!.Mention}.")));
        }

        [SlashCommand("ticket-painel", "Publica o painel de abertura de tickets neste canal",
            (long)Permissions.ManageGuild)]
        [ApplicationCommandRequireUserPermissions(Permissions.ManageGuild)]
        [ApplicationCommandRequireBotPermissions(Permissions.SendMessages)]
        public async Task PanelCommand(
            InteractionContext ctx,
            [Option("titulo", "Título do painel")] string? titulo = null,
            [Option("texto", "Texto explicativo do painel")] string? texto = null)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var settings = TicketService.TryGetSettings(ctx.Guild!.Id);
            if (settings is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(TicketService.NotConfigured()));
                return;
            }

            var embed = new DiscordEmbedBuilder()
                .WithTitle(Embeds.SafeTrim(titulo ?? "Precisa falar com a equipe?", 200))
                .WithDescription(Embeds.SafeTrim(texto ??
                    "Escolha abaixo o tipo do seu atendimento. Um canal privado será criado só para você e a equipe.", 2000))
                .WithColor(DiscordColor.Blurple);

            // Um botao por tipo. Os custom ids nao carregam nada alem do tipo, e e
            // isso que faz este painel continuar funcionando daqui a meses,
            // inclusive depois de o bot reiniciar.
            var message = new DiscordMessageBuilder().AddEmbed(embed);

            // Em linhas de cinco. Uma action row do Discord comporta cinco botoes
            // e o AddComponents lanca no sexto - com os quatro tipos de hoje isso
            // nunca acontece, mas o teto estava documentado so em prosa, e um
            // quinto tipo acrescentado em TicketTypes derrubaria este comando em
            // vez de simplesmente quebrar a linha.
            foreach (var row in TicketTypes.All.Chunk(5))
            {
                message.AddComponents(row
                    .Select(t => new DiscordButtonComponent(ButtonStyle.Primary,
                        $"{TicketService.Prefix}open:{t.Key}", t.Label,
                        emoji: new DiscordComponentEmoji(t.Emoji)))
                    .Cast<DiscordComponent>()
                    .ToArray());
            }

            try
            {
                await ctx.Channel.SendMessageAsync(message);
            }
            catch (Exception ex)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Falha ao publicar o painel", Embeds.Trim(ex.Message, 500))));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Painel publicado", $"O painel está de pé em {ctx.Channel.Mention}.")));
        }

        [SlashCommand("ticket-fechar", "Fecha o ticket deste canal")]
        [ApplicationCommandRequireBotPermissions(Permissions.ManageChannels)]
        public async Task CloseCommand(
            InteractionContext ctx,
            [Option("motivo", "Como isso foi resolvido")] string? motivo = null)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var (settings, ticket, refusal) = await ResolveAsync(ctx);
            if (refusal is not null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(refusal));
                return;
            }

            if (ticket!.openerId != ctx.User.Id && !TicketService.IsStaff(ctx.Member!, settings!))
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Sem permissão", "Só quem abriu o ticket ou a equipe pode fechá-lo.")));
                return;
            }

            var result = await TicketService.CloseAsync(
                ctx.Client, ctx.Guild!, ctx.Channel, ticket, ctx.User, motivo, settings!);

            if (result.Outcome == CloseOutcome.KeptChannel)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Fechado, mas não arquivado", result.Detail!)));
                return;
            }

            // O canal deixou de existir; a edicao da resposta vai falhar e tudo
            // bem.
            try
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Ok("Ticket fechado", "A conversa foi arquivada no canal de log.")));
            }
            catch
            {
                // Esperado.
            }
        }

        [SlashCommand("ticket-add", "Adiciona alguém ao ticket deste canal")]
        [ApplicationCommandRequireBotPermissions(Permissions.ManageRoles)]
        public Task AddCommand(
            InteractionContext ctx,
            [Option("usuario", "Quem entra no ticket")] DiscordUser user) =>
            SetParticipantAsync(ctx, user, add: true);

        [SlashCommand("ticket-remove", "Remove alguém do ticket deste canal")]
        [ApplicationCommandRequireBotPermissions(Permissions.ManageRoles)]
        public Task RemoveCommand(
            InteractionContext ctx,
            [Option("usuario", "Quem sai do ticket")] DiscordUser user) =>
            SetParticipantAsync(ctx, user, add: false);

        /// <summary>
        /// Entra e sai pelo mesmo caminho: a unica diferenca e o lado para onde o
        /// overwrite vai.
        /// </summary>
        private static async Task SetParticipantAsync(InteractionContext ctx, DiscordUser user, bool add)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var (settings, ticket, refusal) = await ResolveAsync(ctx);
            if (refusal is not null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(refusal));
                return;
            }

            if (!TicketService.IsStaff(ctx.Member!, settings!))
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Só para a equipe", "Chamar ou tirar gente do ticket é coisa de quem atende.")));
                return;
            }

            if (user.Id == ticket!.openerId && !add)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Alvo inválido",
                        "Não dá para tirar do ticket quem o abriu. Feche o ticket em vez disso.")));
                return;
            }

            var member = await Hierarchy.TryGetMemberAsync(ctx, user.Id);
            if (member is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Fora do servidor", $"{user.Mention} não está neste servidor.")));
                return;
            }

            try
            {
                // Preserva o resto do overwrite e mexe so nos bits que interessam,
                // como o /lock faz. AddOverwriteAsync SUBSTITUI a linha inteira da
                // pessoa: escrevendo allow/deny do zero, um /ticket-add no autor
                // de um ticket ja trancado apagava o `deny SendMessages` e
                // destrancava a conversa sem ninguem pedir - fora qualquer ajuste
                // que um admin tivesse feito a mao no canal.
                var existing = ctx.Channel.PermissionOverwrites.FirstOrDefault(o => o.Id == member.Id);
                var allow = existing?.Allowed ?? Permissions.None;
                var deny = existing?.Denied ?? Permissions.None;

                if (add)
                {
                    // A lista vem do servico, e nao copiada a mao: a copia que
                    // estava aqui ja tinha divergido, sem AddReactions.
                    allow |= Services.TicketService.ParticipantPermissions;
                    deny &= ~Services.TicketService.ParticipantPermissions;
                }
                else
                {
                    // Nega o acesso em vez de apagar o overwrite: quem tem o cargo
                    // da equipe enxerga o canal pelo cargo, e simplesmente remover
                    // a linha dessa pessoa nao a tiraria de lugar nenhum.
                    allow &= ~Permissions.AccessChannels;
                    deny |= Permissions.AccessChannels;
                }

                await ctx.Channel.AddOverwriteAsync(member, allow, deny,
                    AuditReason.For(ctx.User.UsernameWithDiscriminator,
                        add ? $"adicionado ao ticket {ticket.id}" : $"removido do ticket {ticket.id}"));
            }
            catch (Exception ex)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Falha ao alterar o acesso", Embeds.Trim(ex.Message, 500))));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(add
                ? Embeds.Ok("Adicionado", $"{user.Mention} agora enxerga este ticket.")
                : Embeds.Ok("Removido", $"{user.Mention} não enxerga mais este ticket.")));
        }

        /// <summary>
        /// Config + ticket do canal atual, ou o embed de recusa. Os tres comandos
        /// que agem dentro de um ticket comecam exatamente igual.
        /// </summary>
        private static async Task<(TicketSettings? Settings, Ticket? Ticket, DiscordEmbed? Refusal)> ResolveAsync(
            InteractionContext ctx)
        {
            var settings = TicketService.TryGetSettings(ctx.Guild!.Id);
            if (settings is null)
                return (null, null, TicketService.NotConfigured());

            var ticket = await TicketStore.Instance.ByChannelAsync(ctx.Channel.Id);
            if (ticket is null)
            {
                return (settings, null, Embeds.Error("Aqui não é um ticket",
                    "Use este comando dentro do canal de um ticket."));
            }

            if (!ticket.IsOpen)
                return (settings, null, Embeds.Error("Ticket fechado", "Este ticket já foi fechado."));

            return (settings, ticket, null);
        }
    }
}
