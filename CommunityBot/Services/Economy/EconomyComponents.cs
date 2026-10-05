using System;
using System.Threading.Tasks;
using DisCatSharp;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.Enums.Core;
using DisCatSharp.EventArgs;

namespace CommunityBot.Services.Economy
{
    /// <summary>
    /// Botoes de confirmacao do /pay ("eco:pay:NONCE" e "eco:cancel:NONCE").
    /// Mesmo esquema dos outros handlers: Handled so para o proprio prefixo.
    ///
    /// O botao so carrega a chave do pedido. Valor, quem paga e quem recebe
    /// ficam em memoria no EconomyService - um custom id e enviado pelo cliente,
    /// e confiar nele seria deixar qualquer um escolher o valor.
    /// </summary>
    internal static class EconomyComponents
    {
        public const string Prefix = "eco:";

        public static Task OnComponent(DiscordClient client, ComponentInteractionCreateEventArgs e)
        {
            var id = e.Interaction.Data?.CustomId ?? e.Id;
            if (string.IsNullOrEmpty(id) || !id.StartsWith(Prefix, StringComparison.Ordinal))
                return Task.CompletedTask;

            e.Handled = true;

            var interaction = e.Interaction;
            var user = e.User;
            _ = Task.Run(async () =>
            {
                try
                {
                    await RouteAsync(interaction, user, id);
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"[economia] falha ao tratar '{id}' de {user.Id}: {ex}");
                    await TryFailAsync(interaction, "Something broke while handling that button.");
                }
            });

            return Task.CompletedTask;
        }

        private static async Task RouteAsync(DiscordInteraction interaction, DiscordUser user, string id)
        {
            var parts = id.Split(':');
            if (parts.Length != 3 || parts[1] is not ("pay" or "cancel"))
            {
                Console.WriteLine($"[economia] custom id fora do formato: '{id}'");
                await EphemeralAsync(interaction, Embeds.Error("Unknown button", "This button is from an older version of the bot."));
                return;
            }

            var now = DateTimeOffset.UtcNow;
            var pending = EconomyService.TakePending(parts[2], user.Id, now, out var notYours);

            if (notYours)
            {
                await EphemeralAsync(interaction, Embeds.Info("Not your payment", "Only the person sending the SOL$ can confirm it."));
                return;
            }

            if (pending is null)
            {
                await FinishAsync(interaction, Embeds.Info("Payment expired",
                    "This payment expired or was already handled. Run `/pay` again if you still want to send it."));
                return;
            }

            if (parts[1] == "cancel")
            {
                await FinishAsync(interaction, Embeds.Info("Payment cancelled", "Nothing was sent."));
                return;
            }

            // Saldo revalidado AQUI, na transferencia: entre o /pay e o clique
            // a pessoa pode ter gastado o dinheiro em outro canal.
            var result = await EconomyService.TransferAsync(pending.PayerId, pending.TargetId, pending.Amount);
            if (!result.Done)
            {
                await FinishAsync(interaction, Embeds.Error("Payment failed", result.Refusal ?? "The payment didn't go through."));
                return;
            }

            await FinishAsync(interaction, new DiscordEmbedBuilder()
                .WithTitle("💸 Payment sent")
                .WithDescription($"<@{pending.PayerId}> sent **{EconomyService.Format(pending.Amount)}** to <@{pending.TargetId}>.\n" +
                                 $"-# Their balance is now {EconomyService.Format(result.PayerBalance)}.")
                .WithColor(DiscordColor.SpringGreen)
                .Build());

            Console.WriteLine($"[economia] pay {pending.PayerId} -> {pending.TargetId}: {pending.Amount}");
        }

        /// <summary>Troca a mensagem do pedido pelo resultado, sem os botoes.</summary>
        private static async Task FinishAsync(DiscordInteraction interaction, DiscordEmbed embed)
        {
            await interaction.CreateResponseAsync(InteractionResponseType.DeferredMessageUpdate);
            await interaction.EditOriginalResponseAsync(new DiscordWebhookBuilder()
                .AddEmbed(embed)
                .WithAllowedMentions(Array.Empty<IMention>()), ModifyMode.Replace);
        }

        private static Task EphemeralAsync(DiscordInteraction interaction, DiscordEmbed embed) =>
            interaction.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(embed).AsEphemeral());

        private static async Task TryFailAsync(DiscordInteraction interaction, string message)
        {
            try
            {
                await EphemeralAsync(interaction, Embeds.Error("Error", message));
            }
            catch
            {
                try
                {
                    await interaction.CreateFollowupMessageAsync(new DiscordFollowupMessageBuilder()
                        .AddEmbed(Embeds.Error("Error", message)).AsEphemeral());
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"[economia] nao consegui avisar a falha do botao: {ex.Message}");
                }
            }
        }
    }
}
