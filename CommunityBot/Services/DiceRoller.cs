using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Security.Cryptography;

namespace CommunityBot.Services
{
    internal readonly record struct DiceResult(int Count, int Sides, IReadOnlyList<int> Rolls, long Total);

    /// <summary>
    /// Le e rola dados no formato "NdM" ("2d6", "d20").
    ///
    /// Os tetos abaixo nao sao decoracao: sem eles, "999999d999999" faz o bot
    /// alocar um vetor gigante e montar uma mensagem que o Discord recusaria de
    /// qualquer forma. Um comando de diversao nao pode ser um jeito de derrubar
    /// o processo.
    /// </summary>
    internal static class DiceRoller
    {
        public const int MaxDice = 100;
        public const int MaxSides = 1000;

        public static bool TryRoll(string? input, out DiceResult result, out string? error)
        {
            result = default;
            error = null;

            var text = (input ?? string.Empty).Trim().ToLowerInvariant().Replace(" ", string.Empty);
            if (text.Length == 0)
            {
                error = "Enter the dice to roll, for example `2d6` or `d20`.";
                return false;
            }

            var separator = text.IndexOf('d');
            if (separator < 0)
            {
                error = "Invalid format. Use `NdM`, for example `2d6`.";
                return false;
            }

            var countText = text[..separator];
            var sidesText = text[(separator + 1)..];

            // "d20" sem numero na frente quer dizer um dado so.
            var count = 1;
            if (countText.Length > 0 &&
                !int.TryParse(countText, NumberStyles.None, CultureInfo.InvariantCulture, out count))
            {
                error = "Invalid number of dice.";
                return false;
            }

            if (!int.TryParse(sidesText, NumberStyles.None, CultureInfo.InvariantCulture, out var sides))
            {
                error = "Invalid number of sides.";
                return false;
            }

            if (count < 1 || count > MaxDice)
            {
                error = $"The number of dice must be between 1 and {MaxDice}.";
                return false;
            }

            if (sides < 2 || sides > MaxSides)
            {
                error = $"The number of sides must be between 2 and {MaxSides}.";
                return false;
            }

            var rolls = new int[count];
            for (var i = 0; i < count; i++)
                rolls[i] = RandomNumberGenerator.GetInt32(1, sides + 1);

            // long no total: 100 dados de 1000 lados cabem em int, mas somar em
            // long tira a pergunta de cima da mesa se os tetos mudarem.
            result = new DiceResult(count, sides, rolls, rolls.Sum(r => (long)r));
            return true;
        }
    }
}
