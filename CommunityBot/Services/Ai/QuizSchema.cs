using System;
using System.Collections.Generic;
using System.Linq;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;

namespace CommunityBot.Services.Ai
{
    /// <summary>Uma pergunta ja validada: 4 opcoes distintas e uma resposta certa entre elas.</summary>
    internal sealed class QuizQuestion
    {
        public QuizQuestion(string question, string[] options, int answer)
        {
            Question = question;
            Options = options;
            Answer = answer;
        }

        public string Question { get; }
        public string[] Options { get; }
        public int Answer { get; }
    }

    /// <summary>
    /// O json_schema que o /quiz manda para o llama.cpp, e a validacao da
    /// resposta. Sem Discord nem rede aqui de proposito: e so JSON puro, o que
    /// deixa testar num console a parte sem precisar simular um servidor.
    /// </summary>
    internal static class QuizSchema
    {
        public static JObject Build(int count) => new()
        {
            ["type"] = "object",
            ["properties"] = new JObject
            {
                ["questions"] = new JObject
                {
                    ["type"] = "array",
                    ["minItems"] = count,
                    ["maxItems"] = count,
                    ["items"] = new JObject
                    {
                        ["type"] = "object",
                        ["properties"] = new JObject
                        {
                            ["question"] = new JObject { ["type"] = "string" },
                            ["options"] = new JObject
                            {
                                ["type"] = "array",
                                ["minItems"] = 4,
                                ["maxItems"] = 4,
                                ["items"] = new JObject { ["type"] = "string" }
                            },
                            ["answer"] = new JObject { ["type"] = "integer", ["minimum"] = 0, ["maximum"] = 3 }
                        },
                        ["required"] = new JArray { "question", "options", "answer" }
                    }
                }
            },
            ["required"] = new JArray { "questions" }
        };

        /// <summary>
        /// So aceita a resposta do modelo se bater TUDO: a contagem exata de
        /// perguntas, 4 opcoes nao vazias e distintas por pergunta, e a resposta
        /// dentro do intervalo. Um JSON "quase certo" (3 opcoes, repetida, indice
        /// fora da faixa) vira um quiz sem clique certo - melhor nao postar nada.
        /// </summary>
        public static List<QuizQuestion>? ParseAndValidate(string? raw, int expectedCount)
        {
            if (string.IsNullOrWhiteSpace(raw))
                return null;

            JObject root;
            try
            {
                root = JObject.Parse(raw);
            }
            catch (JsonException)
            {
                return null;
            }

            if (root["questions"] is not JArray items || items.Count != expectedCount)
                return null;

            var result = new List<QuizQuestion>(expectedCount);
            foreach (var item in items)
            {
                if (item is not JObject obj)
                    return null;

                var question = obj["question"]?.ToString()?.Trim();
                if (string.IsNullOrWhiteSpace(question) || question.Length > 300)
                    return null;

                if (obj["options"] is not JArray optsToken || optsToken.Count != 4)
                    return null;

                var options = optsToken.Select(o => o?.ToString()?.Trim() ?? string.Empty).ToArray();
                if (options.Any(o => o.Length == 0 || o.Length > 150))
                    return null;
                if (options.Distinct(StringComparer.OrdinalIgnoreCase).Count() != 4)
                    return null;

                if (obj["answer"] is not { Type: JTokenType.Integer } answerToken)
                    return null;
                var answer = answerToken.Value<int>();
                if (answer is < 0 or > 3)
                    return null;

                result.Add(new QuizQuestion(question!, options, answer));
            }

            return result;
        }
    }
}
