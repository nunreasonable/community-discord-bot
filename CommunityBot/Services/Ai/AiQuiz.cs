using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Linq;
using System.Security.Cryptography;
using System.Threading;
using System.Threading.Tasks;
using CommunityBot.config;
// LocalAi e Embeds moram no namespace pai (CommunityBot.Services); C# nao
// enxerga isso sozinho so por este arquivo estar em CommunityBot.Services.Ai.
using CommunityBot.Services;
using CommunityBot.Services.Economy;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.Enums.Core;

namespace CommunityBot.Services.Ai
{
    /// <summary>
    /// Um quiz em andamento num canal. Mora em memoria: um reinicio do bot no
    /// meio so faz os botoes pararem de responder, igual ao jogo da velha.
    /// </summary>
    internal sealed class QuizGame
    {
        public QuizGame(string id, ulong channelId)
        {
            Id = id;
            ChannelId = channelId;
        }

        public string Id { get; }
        public ulong ChannelId { get; }
        public List<QuizQuestion> Questions { get; set; } = new();

        /// <summary>
        /// Paga SOL$ ao vencedor de cada rodada. So quando o servidor ligou a
        /// economia (/config economy): onde ela esta desligada, vale so o placar.
        /// </summary>
        public bool Paid { get; set; }

        /// <summary>Indice da pergunta valendo agora; -1 antes da primeira.</summary>
        public int CurrentIndex { get; set; } = -1;

        public ulong? Winner { get; set; }

        /// <summary>Quem ja errou a pergunta atual - fora da rodada, mas pode tentar a proxima.</summary>
        public HashSet<ulong> WrongUsers { get; } = new();

        /// <summary>Resolvido na hora que alguem acerta, para o laco da pergunta nao esperar os 20s inteiros.</summary>
        public TaskCompletionSource<bool>? Resolved { get; set; }

        /// <summary>Quantas rodadas cada pessoa ganhou, para o placar final.</summary>
        public Dictionary<ulong, int> Wins { get; } = new();

        public object Gate { get; } = new();
    }

    internal enum QuizReserve { Ok, Active, Cooldown }

    internal enum QuizClick { Won, Wrong, LockedOut, Stale, AlreadyDecided }

    /// <summary>
    /// /quiz: tema vira perguntas via IA local (um pedido so, com json_schema),
    /// cada pergunta e um round de 20s por botao, e quem acerta primeiro leva
    /// SOL$. Nao tem a ver com o personagem Sollarety - usa LocalAi.AskAsync,
    /// nao o CompleteAsync do chat por mencao.
    /// </summary>
    internal static class AiQuiz
    {
        public const string Prefix = "quiz:";
        public const long Reward = 50;

        private static readonly TimeSpan QuestionTime = TimeSpan.FromSeconds(20);
        private static readonly TimeSpan ChannelCooldown = TimeSpan.FromMinutes(10);

        // Um jogo por canal (chave de reserva) e tambem por id (chave do botao,
        // que nao carrega o canal). Os dois mapas sempre andam juntos.
        private static readonly ConcurrentDictionary<ulong, QuizGame> s_byChannel = new();
        private static readonly ConcurrentDictionary<string, QuizGame> s_byId = new();
        private static readonly ConcurrentDictionary<ulong, DateTimeOffset> s_cooldownUntil = new();

        // ------------------------------------------------------------------
        // Reserva do canal
        // ------------------------------------------------------------------

        /// <summary>
        /// Tenta comecar um quiz neste canal. TryAdd e atomico: se dois /quiz
        /// caírem juntos no mesmo canal, so um volta Ok.
        /// </summary>
        public static QuizReserve TryReserve(ulong channelId, out QuizGame? game, out DateTimeOffset cooldownUntil)
        {
            game = null;
            cooldownUntil = default;

            if (s_cooldownUntil.TryGetValue(channelId, out var until) && until > DateTimeOffset.UtcNow)
            {
                cooldownUntil = until;
                return QuizReserve.Cooldown;
            }

            string id;
            do
            {
                id = Convert.ToHexString(RandomNumberGenerator.GetBytes(4)).ToLowerInvariant();
            }
            while (s_byId.ContainsKey(id));

            var candidate = new QuizGame(id, channelId);
            if (!s_byChannel.TryAdd(channelId, candidate))
                return QuizReserve.Active;

            s_byId[id] = candidate;
            game = candidate;
            return QuizReserve.Ok;
        }

        /// <summary>A IA nao devolveu um quiz valido: sai do mapa sem acender cooldown, pra poder tentar de novo na hora.</summary>
        public static void Cancel(QuizGame game)
        {
            s_byChannel.TryRemove(game.ChannelId, out _);
            s_byId.TryRemove(game.Id, out _);
        }

        /// <summary>Quiz jogado até o fim: sai do mapa e liga os 10 minutos de cooldown do canal.</summary>
        public static void Finish(QuizGame game)
        {
            s_byChannel.TryRemove(game.ChannelId, out _);
            s_byId.TryRemove(game.Id, out _);
            s_cooldownUntil[game.ChannelId] = DateTimeOffset.UtcNow + ChannelCooldown;
        }

        public static QuizGame? Find(string id) => s_byId.TryGetValue(id, out var g) ? g : null;

        // ------------------------------------------------------------------
        // Geracao
        // ------------------------------------------------------------------

        /// <summary>Um pedido so para TODAS as perguntas, via json_schema. Tenta de novo uma vez se sair invalido.</summary>
        public static async Task<List<QuizQuestion>?> GenerateAsync(LocalAiSettings settings, string theme, int count)
        {
            var schema = QuizSchema.Build(count);
            var language = LocalAi.IsEnglish(theme)
                ? "Write entirely in English."
                : "Escreva tudo em português do Brasil.";
            var system = "You write short trivia quizzes. Reply ONLY with JSON matching the given schema: an " +
                         $"object with a \"questions\" array of exactly {count} item(s). Each item has \"question\" " +
                         "(one factual question), \"options\" (exactly 4 short, distinct answers) and \"answer\" " +
                         "(the 0-based index of the single correct option). Keep every question unambiguous and " +
                         "strictly factual - no opinions, no trick questions. Avoid offensive, sexual, violent or " +
                         "otherwise sensitive topics; if the theme leans that way, write a safer, more general " +
                         "question about it instead. No text outside the JSON object. " + language;
            var user = $"Theme: {theme}";
            var maxTokens = 220 * count + 200;

            // Uma tentativa e mais uma: o suficiente para um tropeco de formatacao
            // sem deixar quem pediu esperando varias rodadas de pedido.
            for (var attempt = 0; attempt < 2; attempt++)
            {
                var raw = await LocalAi.AskAsync(settings, system, user, maxTokens, schema).ConfigureAwait(false);
                if (QuizSchema.ParseAndValidate(raw, count) is { } parsed)
                    return parsed;
            }

            return null;
        }

        // ------------------------------------------------------------------
        // Clique no botao
        // ------------------------------------------------------------------

        /// <summary>
        /// Decide o resultado sob o Gate do jogo - sincrono de proposito, porque
        /// o Gate so protege o estado em memoria. Premio e resposta ao Discord
        /// ficam para depois, fora do lock.
        /// </summary>
        internal static QuizClick Resolve(QuizGame game, int questionIndex, int optionIndex, ulong userId)
        {
            lock (game.Gate)
            {
                if (questionIndex != game.CurrentIndex)
                    return QuizClick.Stale;
                if (game.Winner is not null)
                    return QuizClick.AlreadyDecided;
                if (game.WrongUsers.Contains(userId))
                    return QuizClick.LockedOut;

                if (game.Questions[questionIndex].Answer != optionIndex)
                {
                    game.WrongUsers.Add(userId);
                    return QuizClick.Wrong;
                }

                game.Winner = userId;
                game.Wins[userId] = game.Wins.GetValueOrDefault(userId) + 1;
                game.Resolved?.TrySetResult(true);
                return QuizClick.Won;
            }
        }

        // ------------------------------------------------------------------
        // O quiz jogando: uma pergunta de cada vez, publicada no canal
        // ------------------------------------------------------------------

        /// <summary>
        /// Toca o quiz inteiro, uma pergunta apos a outra, e no final posta o
        /// placar. Chamado em segundo plano pelo /quiz - a interacao ja foi
        /// respondida antes disto comecar.
        /// </summary>
        public static async Task RunAsync(DiscordChannel channel, QuizGame game)
        {
            try
            {
                for (var i = 0; i < game.Questions.Count; i++)
                {
                    TaskCompletionSource<bool> resolved;
                    lock (game.Gate)
                    {
                        game.CurrentIndex = i;
                        game.Winner = null;
                        game.WrongUsers.Clear();
                        resolved = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
                        game.Resolved = resolved;
                    }

                    var question = game.Questions[i];
                    DiscordMessage message;
                    try
                    {
                        message = await channel.SendMessageAsync(QuestionMessage(game.Id, i, game.Questions.Count, question, game.Paid))
                            .ConfigureAwait(false);
                    }
                    catch (Exception ex)
                    {
                        Console.WriteLine($"[quiz] nao consegui postar a pergunta {i + 1} no canal {channel.Id}: {ex.Message}");
                        break;
                    }

                    await Task.WhenAny(resolved.Task, Task.Delay(QuestionTime)).ConfigureAwait(false);

                    ulong? winnerId;
                    lock (game.Gate)
                        winnerId = game.Winner;

                    if (game.Paid && winnerId is { } wid)
                    {
                        try
                        {
                            await EconomyService.AwardAsync(wid, Reward).ConfigureAwait(false);
                        }
                        catch (Exception ex)
                        {
                            Console.WriteLine($"[quiz] nao consegui creditar o premio de {wid}: {ex.Message}");
                        }
                    }

                    try
                    {
                        await message.ModifyAsync(RevealMessage(game.Id, i, game.Questions.Count, question, winnerId, game.Paid), ModifyMode.Replace)
                            .ConfigureAwait(false);
                    }
                    catch (Exception ex)
                    {
                        Console.WriteLine($"[quiz] nao consegui revelar a resposta da pergunta {i + 1}: {ex.Message}");
                    }
                }
            }
            finally
            {
                // Sempre libera o canal e liga o cooldown, mesmo se uma pergunta
                // falhou no meio - sem isto um erro de rede deixava o canal
                // trancado "com um quiz rodando" para sempre.
                Finish(game);
            }

            try
            {
                await channel.SendMessageAsync(ScoreboardMessage(game)).ConfigureAwait(false);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[quiz] nao consegui postar o placar final: {ex.Message}");
            }
        }

        private static DiscordMessageBuilder QuestionMessage(string gameId, int index, int total, QuizQuestion question, bool paid)
        {
            var embed = new DiscordEmbedBuilder()
                .WithTitle($"❓ Question {index + 1}/{total}")
                .WithDescription(Embeds.SafeTrim(question.Question, 2000))
                .WithColor(DiscordColor.Blurple)
                .WithFooter(paid
                    ? $"20 seconds - first correct answer wins {EconomyService.Format(Reward)}"
                    : "20 seconds - first correct answer gets the point");

            var buttons = new DiscordComponent[question.Options.Length];
            for (var i = 0; i < question.Options.Length; i++)
                buttons[i] = new DiscordButtonComponent(ButtonStyle.Secondary, $"{Prefix}{gameId}:{index}:{i}",
                    Truncate($"{(char)('A' + i)}) {question.Options[i]}", 80));

            return new DiscordMessageBuilder()
                .AddEmbed(embed)
                .AddComponents(buttons)
                .WithAllowedMentions(Array.Empty<IMention>());
        }

        private static DiscordMessageBuilder RevealMessage(string gameId, int index, int total, QuizQuestion question, ulong? winnerId, bool paid)
        {
            var lines = new List<string> { Embeds.SafeTrim(question.Question, 1000), string.Empty };
            for (var i = 0; i < question.Options.Length; i++)
            {
                var mark = i == question.Answer ? "✅" : "⬜";
                lines.Add($"{mark} {(char)('A' + i)}) {Embeds.SafeTrim(question.Options[i], 150)}");
            }

            lines.Add(string.Empty);
            lines.Add(winnerId is { } wid
                ? paid
                    ? $"🏆 <@{wid}> got it first — +{EconomyService.Format(Reward)}!"
                    : $"🏆 <@{wid}> got it first!"
                : "⌛ Time's up, nobody got it.");

            var embed = new DiscordEmbedBuilder()
                .WithTitle($"❓ Question {index + 1}/{total} — answered")
                .WithDescription(string.Join("\n", lines))
                .WithColor(winnerId is not null ? DiscordColor.SpringGreen : DiscordColor.Blurple);

            var buttons = new DiscordComponent[question.Options.Length];
            for (var i = 0; i < question.Options.Length; i++)
                buttons[i] = new DiscordButtonComponent(
                    i == question.Answer ? ButtonStyle.Success : ButtonStyle.Secondary,
                    $"{Prefix}{gameId}:{index}:{i}", Truncate($"{(char)('A' + i)}) {question.Options[i]}", 80), disabled: true);

            return new DiscordMessageBuilder()
                .AddEmbed(embed)
                .AddComponents(buttons)
                .WithAllowedMentions(Array.Empty<IMention>());
        }

        private static DiscordMessageBuilder ScoreboardMessage(QuizGame game)
        {
            var ranked = game.Wins.OrderByDescending(kv => kv.Value).ToList();
            var description = ranked.Count == 0
                ? "Nobody got a question right this time."
                : string.Join("\n", ranked.Select((kv, i) => $"**{i + 1}.** <@{kv.Key}> — {kv.Value} correct"));

            var embed = new DiscordEmbedBuilder()
                .WithTitle("🏁 Quiz over — scoreboard")
                .WithDescription(description)
                .WithColor(DiscordColor.Gold);

            return new DiscordMessageBuilder().AddEmbed(embed).WithAllowedMentions(Array.Empty<IMention>());
        }

        private static string Truncate(string text, int max) =>
            text.Length <= max ? text : text[..(max - 1)] + "…";
    }
}
