using System;
using System.Security.Cryptography;
using System.Threading.Tasks;
using CommunityBot.Services;
using CommunityBot.Services.Fun;
using DisCatSharp.ApplicationCommands;
using DisCatSharp.ApplicationCommands.Attributes;
using DisCatSharp.ApplicationCommands.Context;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.Enums.Core;

namespace CommunityBot.commands
{
    /// <summary>
    /// Joguinhos: pedra-papel-tesoura contra o bot (o /jankenpon da Loritta) e
    /// jogo da velha entre duas pessoas.
    /// </summary>
    internal class Games : ApplicationCommandsModule
    {
        private const string Rock = "rock";
        private const string Paper = "paper";
        private const string Scissors = "scissors";

        private static readonly string[] s_moves = { Rock, Paper, Scissors };

        private static string Show(string move) => move switch
        {
            Rock => "🪨 **Rock**",
            Paper => "📄 **Paper**",
            _ => "✂️ **Scissors**"
        };

        [SlashCommand("rps", "Play rock, paper, scissors against me",
            allowedContexts: new[] { InteractionContextType.Guild, InteractionContextType.BotDm, InteractionContextType.PrivateChannel },
            integrationTypes: new[] { ApplicationCommandIntegrationTypes.GuildInstall, ApplicationCommandIntegrationTypes.UserInstall })]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task RpsCommand(
            InteractionContext ctx,
            [Choice("🪨 Rock", Rock)]
            [Choice("📄 Paper", Paper)]
            [Choice("✂️ Scissors", Scissors)]
            [Option("move", "Your move")] string jogada)
        {
            var mine = s_moves[RandomNumberGenerator.GetInt32(s_moves.Length)];

            var youWin = (jogada, mine) is (Rock, Scissors) or (Paper, Rock) or (Scissors, Paper);
            var (verdict, color) = jogada == mine
                ? ("🤝 **It's a draw!**", DiscordColor.Gold)
                : youWin
                    ? ("🎉 **You win!**", DiscordColor.SpringGreen)
                    // Derrota nao e erro: sem vermelho (ver Embeds).
                    : ("😎 **I win!**", DiscordColor.Blurple);

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(new DiscordEmbedBuilder()
                    .WithTitle("Rock, paper, scissors!")
                    .WithDescription($"You chose {Show(jogada)}, I chose {Show(mine)}.\n\n{verdict}")
                    .WithColor(color)));
        }

        [SlashCommand("tictactoe", "Challenge someone to a game of tic-tac-toe",
            allowedContexts: new[] { InteractionContextType.Guild, InteractionContextType.BotDm, InteractionContextType.PrivateChannel },
            integrationTypes: new[] { ApplicationCommandIntegrationTypes.GuildInstall, ApplicationCommandIntegrationTypes.UserInstall })]
        [SlashCommandCooldown(2, 30, CooldownBucketType.User)]
        public async Task TicTacToeCommand(
            InteractionContext ctx,
            [Option("opponent", "Who to play against")] DiscordUser adversario)
        {
            string? refusal = null;
            if (adversario.Id == ctx.User.Id)
                refusal = "You can't play against yourself. Pick someone else!";
            else if (adversario.IsBot)
                refusal = "Bots can't click buttons. Challenge a person instead.";

            if (refusal is not null)
            {
                await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                    new DiscordInteractionResponseBuilder().AddEmbed(Embeds.Error("Can't start the game", refusal)).AsEphemeral());
                return;
            }

            // Quem desafiou comeca com o X.
            var game = TicTacToe.Create(ctx.User.Id, adversario.Id);
            game.LastInteraction = ctx.Interaction;

            // So o adversario e notificado: e ele quem precisa saber que o jogo
            // comecou. As edicoes seguintes nao notificam ninguem.
            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder()
                    .WithContent(TicTacToe.Content(game))
                    .AddComponents(TicTacToe.Board(game))
                    .WithAllowedMentions(new IMention[] { new UserMention(adversario.Id) }));

            TicTacToe.ArmIdleTimer(game);
        }
    }
}
