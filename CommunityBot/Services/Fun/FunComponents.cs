using System;
using System.Linq;
using System.Threading.Tasks;
using DisCatSharp;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.EventArgs;

namespace CommunityBot.Services.Fun
{
    /// <summary>
    /// Botoes dos comandos fun: o "Return" do /roleplay ("rp:") e o tabuleiro do
    /// /tictactoe ("ttt:"). Mesmo esquema dos tickets: Handled so para os
    /// prefixos daqui, sincrono, e o trabalho em segundo plano.
    /// </summary>
    internal static class FunComponents
    {
        public static Task OnComponent(DiscordClient client, ComponentInteractionCreateEventArgs e)
        {
            var id = e.Interaction.Data?.CustomId ?? e.Id;
            if (string.IsNullOrEmpty(id))
                return Task.CompletedTask;

            Func<Task>? route = null;
            if (id.StartsWith(RoleplayActions.Prefix, StringComparison.Ordinal))
                route = () => ReturnAsync(client, e, id);
            else if (id.StartsWith(TicTacToe.Prefix, StringComparison.Ordinal))
                route = () => MoveAsync(e, id);

            if (route is null)
                return Task.CompletedTask;

            e.Handled = true;

            var interaction = e.Interaction;
            _ = Task.Run(async () =>
            {
                try
                {
                    await route();
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"[fun] falha ao tratar '{id}' de {e.User.Id}: {ex}");
                    await TryFailAsync(interaction, "Something broke while handling that button.");
                }
            });

            return Task.CompletedTask;
        }

        // ------------------------------------------------------------------
        // /roleplay: Return
        // ------------------------------------------------------------------

        private static async Task ReturnAsync(DiscordClient client, ComponentInteractionCreateEventArgs e, string id)
        {
            var interaction = e.Interaction;
            var parts = id.Split(':');

            if (parts.Length != 5 || RoleplayActions.Find(parts[1]) is not { } action ||
                !ulong.TryParse(parts[2], out var fromId) || !ulong.TryParse(parts[3], out var toId) ||
                !int.TryParse(parts[4], out var combo))
            {
                Console.WriteLine($"[fun] custom id de roleplay fora do formato: '{id}'");
                await TryFailAsync(interaction, "This button is from an older version of the bot.");
                return;
            }

            if (e.User.Id != toId)
            {
                await Ephemeral(interaction, Embeds.Info("Not for you",
                    $"Only <@{toId}> can return this. Start your own with `/roleplay`."));
                return;
            }

            // Desliga o botao ANTES de responder com o cartao novo: sem isso o
            // mesmo "Return" podia ser clicado de novo e o combo andava sozinho.
            var original = e.Message;
            var update = new DiscordInteractionResponseBuilder()
                .AddEmbeds(original?.Embeds ?? Array.Empty<DiscordEmbed>())
                .AddComponents(RoleplayActions.ReturnButton(action, fromId, toId, combo, enabled: false));
            await interaction.CreateResponseAsync(InteractionResponseType.UpdateMessage, update);

            var from = await client.GetUserAsync(fromId);
            var (actor, target, note) = RoleplayActions.Resolve(action, e.User, from, client.CurrentUser);

            if (actor is null)
            {
                await interaction.CreateFollowupMessageAsync(new DiscordFollowupMessageBuilder().AddEmbed(
                    new DiscordEmbedBuilder()
                        .WithDescription($"😳 {e.User.Mention}, I'm flattered, but I'm just a bot. How about a hug instead?")
                        .WithColor(action.Color)));
                return;
            }

            var (embed, buttons) = await RoleplayActions.BuildAsync(action, actor, target,
                Math.Min(combo + 1, 999), note);

            var followup = new DiscordFollowupMessageBuilder().AddEmbed(embed);
            if (buttons.Length > 0)
                followup.AddComponents(buttons);

            await interaction.CreateFollowupMessageAsync(followup);
        }

        // ------------------------------------------------------------------
        // /tictactoe
        // ------------------------------------------------------------------

        private static async Task MoveAsync(ComponentInteractionCreateEventArgs e, string id)
        {
            var interaction = e.Interaction;
            var parts = id.Split(':');

            if (parts.Length != 3 || TicTacToe.Find(parts[1]) is not { } game)
            {
                await Ephemeral(interaction, Embeds.Info("Game over", "This game already ended. Start a new one with `/tictactoe`."));
                return;
            }

            var userId = e.User.Id;
            string? refusal = null;
            bool finished;

            lock (game.Gate)
            {
                if (game.Result is not null)
                    refusal = "This game already ended.";
                else if (userId != game.X && userId != game.O)
                    refusal = "This isn't your game. Start your own with `/tictactoe`.";
                else if (parts[2] == TicTacToe.ForfeitCell)
                {
                    var winner = userId == game.X ? game.O : game.X;
                    game.Result = $"🏳️ <@{userId}> gave up — <@{winner}> wins!";
                }
                else if (userId != game.CurrentPlayer)
                    refusal = "Wait for your turn.";
                else if (!int.TryParse(parts[2], out var cell) || cell is < 0 or > 8)
                    refusal = "That square doesn't exist.";
                else if (!TicTacToe.Play(game, cell))
                    refusal = "That square is already taken.";

                if (refusal is null)
                    game.LastInteraction = interaction;

                finished = game.Result is not null;
            }

            if (refusal is not null)
            {
                await Ephemeral(interaction, Embeds.Info("Can't do that", refusal));
                return;
            }

            if (finished)
                TicTacToe.End(game);
            else
                TicTacToe.ArmIdleTimer(game);

            await interaction.CreateResponseAsync(InteractionResponseType.UpdateMessage,
                new DiscordInteractionResponseBuilder()
                    .WithContent(TicTacToe.Content(game))
                    .AddComponents(TicTacToe.Board(game))
                    .WithAllowedMentions(Array.Empty<IMention>()));
        }

        // ------------------------------------------------------------------

        private static Task Ephemeral(DiscordInteraction interaction, DiscordEmbed embed) =>
            interaction.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(embed).AsEphemeral());

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
                    // Ja respondeu com o UpdateMessage: so cabe follow-up.
                    await interaction.CreateFollowupMessageAsync(new DiscordFollowupMessageBuilder()
                        .AddEmbed(Embeds.Error("Error", message)).AsEphemeral());
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"[fun] nao consegui avisar a falha do botao: {ex.Message}");
                }
            }
        }
    }
}
