using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Linq;
using System.Security.Cryptography;
using System.Threading;
using System.Threading.Tasks;
using DisCatSharp.Entities;
using DisCatSharp.Enums;

namespace CommunityBot.Services.Fun
{
    /// <summary>
    /// Jogo da velha entre duas pessoas, num tabuleiro de botoes.
    ///
    /// O estado mora em memoria: um jogo dura minutos, e um reinicio do bot no
    /// meio so faz os botoes responderem "este jogo acabou". Nao vale um arquivo.
    /// </summary>
    internal static class TicTacToe
    {
        public const string Prefix = "ttt:";
        public const string ForfeitCell = "forfeit";

        private static readonly TimeSpan s_idleLimit = TimeSpan.FromMinutes(3);

        private static readonly int[][] s_lines =
        {
            new[] { 0, 1, 2 }, new[] { 3, 4, 5 }, new[] { 6, 7, 8 },
            new[] { 0, 3, 6 }, new[] { 1, 4, 7 }, new[] { 2, 5, 8 },
            new[] { 0, 4, 8 }, new[] { 2, 4, 6 }
        };

        private static readonly ConcurrentDictionary<string, Game> s_games = new();

        internal sealed class Game
        {
            public Game(string id, ulong x, ulong o)
            {
                Id = id;
                X = x;
                O = o;
            }

            public string Id { get; }
            public ulong X { get; }
            public ulong O { get; }
            public char[] Board { get; } = Enumerable.Repeat(' ', 9).ToArray();
            public bool XTurn { get; set; } = true;
            public string? Result { get; set; }
            public int[]? WinningLine { get; set; }

            /// <summary>
            /// A interacao mais recente do jogo. E por ela que o tabuleiro e
            /// editado quando expira: o token vale 15 minutos, e o prazo de
            /// inatividade e 3.
            /// </summary>
            public DiscordInteraction? LastInteraction { get; set; }

            public CancellationTokenSource? IdleTimer { get; set; }
            public object Gate { get; } = new();

            public ulong CurrentPlayer => XTurn ? X : O;
        }

        public static Game Create(ulong x, ulong o)
        {
            // 8 hex bastam para distinguir os jogos em andamento, e cabem folgado
            // nos 100 caracteres de um custom id.
            string id;
            Game game;
            do
            {
                id = Convert.ToHexString(RandomNumberGenerator.GetBytes(4)).ToLowerInvariant();
                game = new Game(id, x, o);
            }
            while (!s_games.TryAdd(id, game));

            return game;
        }

        public static Game? Find(string id) => s_games.TryGetValue(id, out var g) ? g : null;

        public static void End(Game game)
        {
            s_games.TryRemove(game.Id, out _);
            game.IdleTimer?.Cancel();
        }

        /// <summary>Joga na casa; devolve false se a casa ja estava ocupada.</summary>
        public static bool Play(Game game, int cell)
        {
            if (game.Board[cell] != ' ')
                return false;

            var mark = game.XTurn ? 'X' : 'O';
            game.Board[cell] = mark;

            var line = s_lines.FirstOrDefault(l => l.All(i => game.Board[i] == mark));
            if (line is not null)
            {
                game.WinningLine = line;
                game.Result = $"🎉 <@{game.CurrentPlayer}> wins!";
                return true;
            }

            if (game.Board.All(c => c != ' '))
            {
                game.Result = "🤝 It's a draw!";
                return true;
            }

            game.XTurn = !game.XTurn;
            return true;
        }

        public static string Content(Game game)
        {
            var header = $"❌ <@{game.X}> vs ⭕ <@{game.O}>";
            var status = game.Result ?? $"It's <@{game.CurrentPlayer}>'s turn ({(game.XTurn ? "❌" : "⭕")}).";
            return $"{header}\n\n{status}";
        }

        /// <summary>O tabuleiro em tres fileiras de botoes, mais a de desistir.</summary>
        public static IEnumerable<DiscordActionRowComponent> Board(Game game)
        {
            var over = game.Result is not null;

            for (var row = 0; row < 3; row++)
            {
                var buttons = new List<DiscordComponent>();
                for (var col = 0; col < 3; col++)
                {
                    var cell = row * 3 + col;
                    var mark = game.Board[cell];
                    var winning = game.WinningLine?.Contains(cell) == true;

                    var style = winning ? ButtonStyle.Success : mark switch
                    {
                        'X' => ButtonStyle.Primary,
                        'O' => ButtonStyle.Secondary,
                        _ => ButtonStyle.Secondary
                    };

                    // "\u200b" e o rotulo vazio: botao exige rotulo ou emoji, e a
                    // casa livre nao tem nenhum dos dois.
                    buttons.Add(mark == ' '
                        ? new DiscordButtonComponent(style, $"{Prefix}{game.Id}:{cell}", "\u200b", over)
                        : new DiscordButtonComponent(style, $"{Prefix}{game.Id}:{cell}", null!, true,
                            new DiscordComponentEmoji(mark == 'X' ? "✖️" : "⭕")));
                }

                yield return new DiscordActionRowComponent(buttons);
            }

            yield return new DiscordActionRowComponent(new DiscordComponent[]
            {
                new DiscordButtonComponent(ButtonStyle.Secondary, $"{Prefix}{game.Id}:{ForfeitCell}", "Give up", over,
                    new DiscordComponentEmoji("🏳️"))
            });
        }

        /// <summary>
        /// Rearma o prazo de inatividade. Estourou, o jogo termina e o tabuleiro
        /// e desligado pela ultima interacao conhecida.
        /// </summary>
        public static void ArmIdleTimer(Game game)
        {
            var cts = new CancellationTokenSource();
            CancellationTokenSource? previous;
            lock (game.Gate)
            {
                previous = game.IdleTimer;
                game.IdleTimer = cts;
            }

            previous?.Cancel();

            _ = Task.Run(async () =>
            {
                try
                {
                    await Task.Delay(s_idleLimit, cts.Token);
                }
                catch (OperationCanceledException)
                {
                    return;
                }

                DiscordInteraction? interaction;
                lock (game.Gate)
                {
                    if (game.Result is not null)
                        return;

                    game.Result = $"⌛ Game over — <@{game.CurrentPlayer}> didn't move for {s_idleLimit.TotalMinutes:0} minutes.";
                    interaction = game.LastInteraction;
                }

                End(game);

                if (interaction is null)
                    return;

                try
                {
                    await interaction.EditOriginalResponseAsync(new DiscordWebhookBuilder()
                        .WithContent(Content(game))
                        .AddComponents(Board(game)));
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"[tictactoe] aviso: nao consegui encerrar o jogo {game.Id} expirado: {ex.Message}");
                }
            });
        }
    }
}
