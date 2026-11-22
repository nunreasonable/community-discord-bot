using System;
using System.Threading.Tasks;
using DisCatSharp;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.EventArgs;

namespace CommunityBot.Services.Ai
{
    /// <summary>
    /// Botoes do /quiz ("quiz:"). Mesmo esquema dos outros: Handled so para o
    /// prefixo daqui, sincrono, e o trabalho em segundo plano.
    /// </summary>
    internal static class AiQuizComponents
    {
        public static Task OnComponent(DiscordClient client, ComponentInteractionCreateEventArgs e)
        {
            var id = e.Interaction.Data?.CustomId ?? string.Empty;
            if (!id.StartsWith(AiQuiz.Prefix, StringComparison.Ordinal))
                return Task.CompletedTask;

            e.Handled = true;

            var interaction = e.Interaction;
            _ = Task.Run(async () =>
            {
                try
                {
                    await ClickAsync(e, id);
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"[quiz] falha ao tratar '{id}' de {e.User.Id}: {ex}");
                    await TryFailAsync(interaction, "Something broke while handling that button.");
                }
            });

            return Task.CompletedTask;
        }

        private static async Task ClickAsync(ComponentInteractionCreateEventArgs e, string id)
        {
            var interaction = e.Interaction;
            var parts = id.Split(':');

            if (parts.Length != 4 || !int.TryParse(parts[2], out var questionIndex) || !int.TryParse(parts[3], out var optionIndex))
            {
                Console.WriteLine($"[quiz] custom id fora do formato: '{id}'");
                await TryFailAsync(interaction, "This button is from an older version of the bot.");
                return;
            }

            // So gente de verdade joga. Nao deveria chegar aqui vindo da UI do
            // Discord, mas uma conta de bot que tentasse clicar via API nao leva
            // premio nenhum.
            if (e.User.IsBot)
            {
                await Ephemeral(interaction, Embeds.Info("Can't play", "Bots don't play the quiz."));
                return;
            }

            if (AiQuiz.Find(parts[1]) is not { } game)
            {
                await Ephemeral(interaction, Embeds.Info("Quiz over", "This quiz already ended."));
                return;
            }

            var outcome = AiQuiz.Resolve(game, questionIndex, optionIndex, e.User.Id);
            switch (outcome)
            {
                case QuizClick.Stale:
                    await Ephemeral(interaction, Embeds.Info("Too late", "This question's time is up."));
                    return;
                case QuizClick.AlreadyDecided:
                    await Ephemeral(interaction, Embeds.Info("Already answered", "Someone already got this one — wait for the next question."));
                    return;
                case QuizClick.LockedOut:
                    await Ephemeral(interaction, Embeds.Info("Already tried", "You already picked wrong on this question — sit this one out."));
                    return;
                case QuizClick.Wrong:
                    await Ephemeral(interaction, Embeds.Error("Wrong answer", "That's not it. You're out for this question, but the next one is fresh."));
                    return;
                case QuizClick.Won:
                    // So um "parabens" efemero pra quem clicou: a revelacao
                    // publica (resposta certa, quem ganhou) e o RunAsync que
                    // posta, depois que o laco da pergunta acordar.
                    await Ephemeral(interaction, Embeds.Ok("Correct!", "Nice — you got it first. The result shows up in the channel in a moment."));
                    return;
            }
        }

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
            catch (Exception ex)
            {
                Console.WriteLine($"[quiz] nao consegui avisar a falha do botao: {ex.Message}");
            }
        }
    }
}
