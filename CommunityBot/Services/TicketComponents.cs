using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading.Tasks;
using DisCatSharp;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.EventArgs;

namespace CommunityBot.Services
{
    /// <summary>
    /// Botoes e modais dos tickets.
    ///
    /// Registrado como um handler PROPRIO de ComponentInteractionCreated, sem
    /// tocar no HandleComponentInteraction do Program. Os handlers rodam na ordem
    /// de inscricao e a cadeia so para em quem marca Handled - e aquele retorna
    /// sem marcar quando a mensagem nao e dele, inclusive no early-return de
    /// ModalSubmit. Entao os dois convivem, e aquele guard de modal nao bloqueia
    /// nada daqui.
    ///
    /// Este marca Handled APENAS para custom id que comeca com "ticket:".
    /// Marcar por engano num id alheio mataria a paginacao do /logs.
    /// </summary>
    internal static class TicketComponents
    {
        public static Task OnComponent(DiscordClient client, ComponentInteractionCreateEventArgs e)
        {
            var id = e.Interaction.Data?.CustomId ?? e.Id;
            if (string.IsNullOrEmpty(id) || !id.StartsWith(TicketService.Prefix, StringComparison.Ordinal))
                return Task.CompletedTask;

            // Sincrono, antes de qualquer await: o despachante so olha Handled
            // depois que este metodo retorna.
            e.Handled = true;

            var interaction = e.Interaction;
            _ = Task.Run(async () =>
            {
                try
                {
                    await RouteAsync(client, e, id, interaction);
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"[tickets] falha ao tratar '{id}' de {e.User.Id}: {ex}");
                    await TryFailAsync(interaction, "Something broke while processing this action. Let the bot's admin know.");
                }
            });

            return Task.CompletedTask;
        }

        private static async Task RouteAsync(DiscordClient client, ComponentInteractionCreateEventArgs e,
            string id, DiscordInteraction interaction)
        {
            var parts = id.Split(':');
            if (parts.Length < 3)
            {
                // Handled ja foi marcado la em cima, entao ninguem mais vai
                // responder: sair calado aqui deixa o Discord mostrar "essa
                // interacao falhou" sem uma linha de log. Um painel publicado ha
                // meses pode carregar um id de um esquema antigo.
                Console.WriteLine($"[tickets] custom id fora do formato: '{id}'");
                await TryFailAsync(interaction,
                    "This button is from an old version of the panel. Ask for it to be reposted.");
                return;
            }

            var action = parts[1];
            var argument = parts[2];

            // Capturados em locais nao-nulos logo apos a checagem: o
            // compilador nao carrega essa garantia para dentro dos metodos
            // chamados abaixo, e passar `e.Member` cru rendia CS8604 em cada um.
            // Este projeto ja pagou por CS8604 ignorado uma vez - o docstring do
            // Hierarchy conta a historia.
            if (e.Guild is not { } guild || e.Member is not { } member)
            {
                await TryFailAsync(interaction, "This only works inside a server.");
                return;
            }

            var settings = TicketService.TryGetSettings(guild.Id);
            if (settings is null)
            {
                await RespondAsync(interaction, TicketService.NotConfigured());
                return;
            }

            switch (action)
            {
                case "open":
                    await OpenModalAsync(interaction, argument);
                    return;

                case "new":
                    await CreateFromModalAsync(client, guild, member, interaction, argument, settings);
                    return;

                case "claim":
                    await ClaimAsync(guild.Id, member, interaction, argument, settings);
                    return;

                case "close":
                    await CloseModalAsync(interaction, argument);
                    return;

                case "closed":
                    await CloseFromModalAsync(client, guild, member, e.Channel, interaction, argument, settings);
                    return;

                default:
                    Console.WriteLine($"[tickets] acao desconhecida no custom id '{id}'");
                    await TryFailAsync(interaction,
                        "This button is from an old version of the panel. Ask for it to be reposted.");
                    return;
            }
        }

        // ------------------------------------------------------------------
        // Abertura
        // ------------------------------------------------------------------

        private static async Task OpenModalAsync(DiscordInteraction interaction, string typeKey)
        {
            var type = TicketTypes.Find(typeKey);
            if (type is null)
            {
                await RespondAsync(interaction, Embeds.Error("Type unavailable",
                    "This ticket type no longer exists. Ask the bot's admin to repost the panel."));
                return;
            }

            await interaction.CreateInteractionModalResponseAsync(new DiscordInteractionModalBuilder()
                .WithTitle($"Open ticket — {type.Label}")
                .WithCustomId($"{TicketService.Prefix}new:{type.Key}")
                // ATENCAO A ORDEM: o construtor e (style, LABEL, CUSTOM ID, ...).
                // O rotulo vem antes do id, e trocar os dois compila sem um
                // pio - sao duas strings. O efeito seria o ReadModalValue nunca
                // achar o campo, e todo ticket nascer sem assunto.
                .AddTextComponent(new DiscordTextInputComponent(
                    TextComponentStyle.Small, "Subject", "assunto",
                    "In a few words, what this is about", 3, 200, true))
                .AddTextComponent(new DiscordTextInputComponent(
                    TextComponentStyle.Paragraph, "Details", "descricao",
                    "Tell us what happened, in as much detail as you can", null, 1500, false)));
        }

        private static async Task CreateFromModalAsync(DiscordClient client, DiscordGuild guild, DiscordMember member,
            DiscordInteraction interaction, string typeKey, TicketSettings settings)
        {
            await interaction.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var subject = ReadModalValue(interaction, "assunto") ?? "(no subject)";
            var description = ReadModalValue(interaction, "descricao");

            var (channel, error) = await TicketService.OpenAsync(client, guild, member, settings, typeKey, subject, description);

            await interaction.EditOriginalResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                error ?? Embeds.Ok("Ticket opened", $"Continue in {channel!.Mention}.")));
        }

        // ------------------------------------------------------------------
        // Assumir
        // ------------------------------------------------------------------

        private static async Task ClaimAsync(ulong guildId, DiscordMember member, DiscordInteraction interaction,
            string ticketId, TicketSettings settings)
        {
            if (!TicketService.IsStaff(member, settings))
            {
                await RespondAsync(interaction, Embeds.Error("Staff only",
                    "Only staff can claim a ticket."));
                return;
            }

            /*
             * Defer ANTES de mexer no arquivo.
             *
             * O Discord da tres segundos para a interacao ser reconhecida, e este
             * era o unico ramo que fazia trabalho de disco antes disso: ler o
             * config, e depois ler, reserializar e fsync do tickets.json inteiro,
             * tudo atras do mesmo semaforo estatico que uma abertura ou um
             * fechamento pode estar segurando. Estourando o prazo, o
             * CreateResponseAsync lancava "Unknown interaction" e a pessoa via "a
             * aplicacao nao respondeu" - com o claim JA GRAVADO. Ela clicava de
             * novo e ouvia "Already claimed".
             */
            await interaction.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder());

            var claimed = await TicketStore.Instance.UpdateAsync(edit =>
            {
                // Preso ao servidor, como o ByIdAsync. Hoje um id e unico no
                // arquivo inteiro, mas duas regras de busca diferentes para a
                // mesma chave e o tipo de coisa que deixa de dar no mesmo.
                var stored = edit.File.tickets.FirstOrDefault(t =>
                    t.guildId == guildId &&
                    string.Equals(t.id, ticketId, StringComparison.OrdinalIgnoreCase));

                if (stored is null || !stored.IsOpen)
                    return (Ticket: stored, Taken: false);

                // Ja assumido por outra pessoa: nao rouba. A conferencia mora
                // dentro do lock porque dois cliques simultaneos passariam os
                // dois por uma checagem feita fora.
                // Inclui o proprio: sem isto, reclicar reescrevia o arquivo
                // inteiro e postava um segundo "Ticket claimed" identico.
                if (stored.claimedById is not null)
                    return (Ticket: stored, Taken: false);

                stored.claimedById = member.Id;
                edit.MarkChanged();
                return (Ticket: stored, Taken: true);
            });

            if (claimed.Ticket is null)
            {
                await interaction.EditOriginalResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Unknown ticket",
                        "I couldn't find this ticket in the records. It may have been closed.")));
                return;
            }

            if (!claimed.Taken)
            {
                await interaction.EditOriginalResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    claimed.Ticket.IsOpen
                        ? Embeds.Info("Already claimed", $"<@{claimed.Ticket.claimedById}> is already handling this ticket.")
                        : Embeds.Error("Ticket closed", "This ticket has already been closed.")));
                return;
            }

            await interaction.EditOriginalResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Ticket claimed", $"{member.Mention} will handle this ticket.")));
        }

        // ------------------------------------------------------------------
        // Fechamento
        // ------------------------------------------------------------------

        private static async Task CloseModalAsync(DiscordInteraction interaction, string ticketId)
        {
            // O modal e a confirmacao. Fechar apaga o canal, e um botao vermelho
            // que destroi a conversa com um clique so e cilada.
            await interaction.CreateInteractionModalResponseAsync(new DiscordInteractionModalBuilder()
                .WithTitle("Close ticket")
                .WithCustomId($"{TicketService.Prefix}closed:{ticketId}")
                .AddTextComponent(new DiscordTextInputComponent(
                    TextComponentStyle.Paragraph, "Reason (optional)", "motivo",
                    "How was it resolved?", null, 500, false)));
        }

        private static async Task CloseFromModalAsync(DiscordClient client, DiscordGuild guild, DiscordMember member,
            DiscordChannel channel, DiscordInteraction interaction, string ticketId, TicketSettings settings)
        {
            await interaction.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var reason = ReadModalValue(interaction, "motivo");
            var ticket = await TicketStore.Instance.ByIdAsync(guild.Id, ticketId);

            if (ticket is null || !ticket.IsOpen)
            {
                await interaction.EditOriginalResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Ticket unavailable", "This ticket no longer exists or has already been closed.")));
                return;
            }

            if (ticket.openerId != member.Id && !TicketService.IsStaff(member, settings))
            {
                await interaction.EditOriginalResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("No permission", "Only the person who opened the ticket or the staff can close it.")));
                return;
            }

            var result = await TicketService.CloseAsync(client, guild, channel, ticket, member, reason, settings);

            if (result.Outcome == CloseOutcome.KeptChannel)
            {
                await interaction.EditOriginalResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Closed, but not archived", result.Detail!)));
                return;
            }

            // O canal acabou de ser apagado: nao ha resposta que sobreviva. A
            // tentativa vai num try porque editar a resposta de um canal que
            // deixou de existir e erro esperado, nao falha do fechamento.
            try
            {
                await interaction.EditOriginalResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Ok("Ticket closed", "The conversation was archived in the log channel.")));
            }
            catch
            {
                // Esperado.
            }
        }

        // ------------------------------------------------------------------
        // Leitura de modal
        // ------------------------------------------------------------------

        /// <summary>
        /// Valor de um campo de modal pelo custom id.
        ///
        /// A arvore precisa ser percorrida: ModalComponents e uma lista do tipo
        /// BASE DiscordComponent, que so expoe CustomId - o valor mora no
        /// DiscordTextInputComponent. E o Discord pode entregar o campo solto,
        /// dentro de um ActionRow, ou dentro de um Label (o
        /// DiscordTextInputComponent implementa ILabelComponent). Aguentar as tres
        /// formas e mais barato do que descobrir em producao qual delas veio.
        /// </summary>
        private static string? ReadModalValue(DiscordInteraction interaction, string customId)
        {
            var components = interaction.Data?.ModalComponents;
            if (components is null)
                return null;

            foreach (var component in Flatten(components))
            {
                if (component is DiscordTextInputComponent input &&
                    string.Equals(input.CustomId, customId, StringComparison.Ordinal))
                {
                    return string.IsNullOrWhiteSpace(input.Value) ? null : input.Value;
                }
            }

            return null;
        }

        private static IEnumerable<DiscordComponent> Flatten(IEnumerable<DiscordComponent> components)
        {
            foreach (var component in components)
            {
                yield return component;

                switch (component)
                {
                    case DiscordActionRowComponent row when row.Components is not null:
                        foreach (var child in Flatten(row.Components))
                            yield return child;
                        break;

                    case DiscordLabelComponent label when label.Component is DiscordComponent inner:
                        foreach (var child in Flatten(new[] { inner }))
                            yield return child;
                        break;
                }
            }
        }

        // ------------------------------------------------------------------
        // Respostas
        // ------------------------------------------------------------------

        private static Task RespondAsync(DiscordInteraction interaction, DiscordEmbed embed) =>
            interaction.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(embed).AsEphemeral());

        /// <summary>
        /// Ultimo recurso quando algo estourou. Sem isto o clique morre em "a
        /// aplicacao nao respondeu" e a pessoa nao sabe se funcionou.
        /// </summary>
        private static async Task TryFailAsync(DiscordInteraction interaction, string message)
        {
            try
            {
                await interaction.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                    new DiscordInteractionResponseBuilder().AddEmbed(Embeds.Error("Error", message)).AsEphemeral());
            }
            catch
            {
                try
                {
                    // Ja tinha deferido: editar e o unico caminho que resta.
                    await interaction.EditOriginalResponseAsync(new DiscordWebhookBuilder()
                        .AddEmbed(Embeds.Error("Error", message)));
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"[tickets] nao consegui avisar sobre a falha: {ex.Message}");
                }
            }
        }
    }
}
