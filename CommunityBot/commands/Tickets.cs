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
        [SlashCommand("ticket", "Open a ticket with the staff", allowedContexts: new[] { InteractionContextType.Guild })]
        [ApplicationCommandRequireBotPermissions(Permissions.ManageChannels | Permissions.ManageRoles)]
        [SlashCommandCooldown(2, 60, CooldownBucketType.User)]
        public async Task TicketCommand(
            InteractionContext ctx,
            [Option("subject", "In a few words, what this is about")] string assunto,
            // As escolhas vem do TicketTypeChoiceProvider, e nao de [Choice]
            // escrito a mao: era a segunda copia da lista de tipos.
            [ChoiceProvider(typeof(TicketTypeChoiceProvider))]
            [Option("type", "What kind of ticket this is")] string tipo = "duvida",
            [Option("details", "Tell us what happened")] string? detalhes = null)
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
                error ?? Embeds.Ok("Ticket opened", $"Continue in {channel!.Mention}.")));
        }

        [SlashCommand("ticket-panel", "Post the ticket panel in this channel",
            (long)Permissions.ManageGuild, allowedContexts: new[] { InteractionContextType.Guild })]
        [ApplicationCommandRequireUserPermissions(Permissions.ManageGuild)]
        [ApplicationCommandRequireBotPermissions(Permissions.SendMessages)]
        public async Task PanelCommand(
            InteractionContext ctx,
            [Option("title", "Panel title")] string? titulo = null,
            [Option("text", "Panel description text")] string? texto = null)
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
                .WithTitle(Embeds.SafeTrim(titulo ?? "Need to talk to the staff?", 200))
                .WithDescription(Embeds.SafeTrim(texto ??
                    "Pick the kind of help you need below. A private channel will be created just for you and the staff.", 2000))
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
                    Embeds.Error("Couldn't post the panel", Embeds.Trim(ex.Message, 500))));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Panel posted", $"The panel is live in {ctx.Channel.Mention}.")));
        }

        [SlashCommand("ticket-close", "Close the ticket in this channel", allowedContexts: new[] { InteractionContextType.Guild })]
        [ApplicationCommandRequireBotPermissions(Permissions.ManageChannels)]
        public async Task CloseCommand(
            InteractionContext ctx,
            [Option("reason", "How it was resolved")] string? motivo = null)
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
                    Embeds.Error("No permission", "Only the person who opened the ticket or the staff can close it.")));
                return;
            }

            var result = await TicketService.CloseAsync(
                ctx.Client, ctx.Guild!, ctx.Channel, ticket, ctx.User, motivo, settings!);

            if (result.Outcome == CloseOutcome.KeptChannel)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Closed, but not archived", result.Detail!)));
                return;
            }

            // O canal deixou de existir; a edicao da resposta vai falhar e tudo
            // bem.
            try
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Ok("Ticket closed", "The conversation was archived in the log channel.")));
            }
            catch
            {
                // Esperado.
            }
        }

        [SlashCommand("ticket-add", "Add someone to the ticket in this channel", allowedContexts: new[] { InteractionContextType.Guild })]
        [ApplicationCommandRequireBotPermissions(Permissions.ManageRoles)]
        public Task AddCommand(
            InteractionContext ctx,
            [Option("user", "Who to add to the ticket")] DiscordUser user) =>
            SetParticipantAsync(ctx, user, add: true);

        [SlashCommand("ticket-remove", "Remove someone from the ticket in this channel", allowedContexts: new[] { InteractionContextType.Guild })]
        [ApplicationCommandRequireBotPermissions(Permissions.ManageRoles)]
        public Task RemoveCommand(
            InteractionContext ctx,
            [Option("user", "Who to remove from the ticket")] DiscordUser user) =>
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
                    Embeds.Error("Staff only", "Only staff can add people to or remove them from a ticket.")));
                return;
            }

            if (user.Id == ticket!.openerId && !add)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Invalid target",
                        "You can't remove the person who opened the ticket. Close the ticket instead.")));
                return;
            }

            var member = await Hierarchy.TryGetMemberAsync(ctx, user.Id);
            if (member is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Not in this server", $"{user.Mention} isn't in this server.")));
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
                        add ? $"added to ticket {ticket.id}" : $"removed from ticket {ticket.id}"));
            }
            catch (Exception ex)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Couldn't change access", Embeds.Trim(ex.Message, 500))));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(add
                ? Embeds.Ok("Added", $"{user.Mention} can now see this ticket.")
                : Embeds.Ok("Removed", $"{user.Mention} can no longer see this ticket.")));
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
                return (settings, null, Embeds.Error("Not a ticket",
                    "Use this command inside a ticket channel."));
            }

            if (!ticket.IsOpen)
                return (settings, null, Embeds.Error("Ticket closed", "This ticket has already been closed."));

            return (settings, ticket, null);
        }
    }
}
