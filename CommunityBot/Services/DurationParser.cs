using System;
using System.Globalization;
using System.Text;

namespace CommunityBot.Services
{
    /// <summary>
    /// Le duracao escrita como gente escreve: "10m", "2h30m", "1d", "45s".
    ///
    /// Existe porque a alternativa seria pedir minutos como numero, e "1440" nao
    /// diz a ninguem que sao 24 horas. Aceita varias unidades na mesma string e
    /// soma todas.
    /// </summary>
    internal static class DurationParser
    {
        /// <summary>
        /// Teto do timeout no Discord. Pedir mais que isto e recusado pela API,
        /// entao e melhor recusar aqui com uma mensagem que explica.
        /// </summary>
        public static readonly TimeSpan MaxTimeout = TimeSpan.FromDays(28);

        public static bool TryParse(string? input, out TimeSpan duration, out string? error)
        {
            duration = TimeSpan.Zero;
            error = null;

            var text = (input ?? string.Empty).Trim().ToLowerInvariant();
            if (text.Length == 0)
            {
                error = "Enter a duration, for example `10m`, `2h30m` or `1d`.";
                return false;
            }

            var total = TimeSpan.Zero;
            var number = new StringBuilder();
            var sawUnit = false;

            foreach (var ch in text)
            {
                // ASCII 0-9 apenas, e nao char.IsDigit: este ultimo aceita toda a
                // categoria Unicode Nd (٣, ৩, ๓...), que o long.TryParse com
                // NumberStyles.None/InvariantCulture depois recusa - o resultado
                // era a mensagem de erro ERRADA ("Number too large.") para uma
                // entrada que na verdade tem caractere invalido.
                if (ch >= '0' && ch <= '9')
                {
                    number.Append(ch);
                    continue;
                }

                if (ch == ' ')
                    continue;

                if (number.Length == 0)
                {
                    error = $"Missing a number before `{ch}`. Use something like `10m` or `2h30m`.";
                    return false;
                }

                // long, nao int: "999999999999m" cabe aqui e estoura no
                // TimeSpan - e melhor cair no teto abaixo do que num overflow.
                if (!long.TryParse(number.ToString(), NumberStyles.None, CultureInfo.InvariantCulture, out var value))
                {
                    error = "Number too large.";
                    return false;
                }

                TimeSpan unit;
                switch (ch)
                {
                    case 's': unit = TimeSpan.FromSeconds(1); break;
                    case 'm': unit = TimeSpan.FromMinutes(1); break;
                    case 'h': unit = TimeSpan.FromHours(1); break;
                    case 'd': unit = TimeSpan.FromDays(1); break;
                    default:
                        error = $"Unknown unit `{ch}`. Use `s`, `m`, `h` or `d`.";
                        return false;
                }

                try
                {
                    total += unit * value;
                }
                catch (OverflowException)
                {
                    error = "Duration too large.";
                    return false;
                }

                if (total > MaxTimeout)
                {
                    error = $"Discord doesn't allow more than {MaxTimeout.TotalDays:0} days.";
                    return false;
                }

                number.Clear();
                sawUnit = true;
            }

            // Numero sem unidade no fim ("10") seria adivinhacao: minutos? horas?
            // Melhor recusar e dizer o formato.
            if (number.Length > 0)
            {
                error = "Missing a unit at the end. Use `s`, `m`, `h` or `d` — for example `10m`.";
                return false;
            }

            if (!sawUnit || total <= TimeSpan.Zero)
            {
                error = "The duration must be greater than zero.";
                return false;
            }

            duration = total;
            return true;
        }

        /// <summary>Escreve a duracao de volta em ingles, para o embed de confirmacao.</summary>
        public static string Describe(TimeSpan duration)
        {
            var parts = new System.Collections.Generic.List<string>();

            if (duration.Days > 0) parts.Add($"{duration.Days} day(s)");
            if (duration.Hours > 0) parts.Add($"{duration.Hours} hour(s)");
            if (duration.Minutes > 0) parts.Add($"{duration.Minutes} minute(s)");
            if (duration.Seconds > 0) parts.Add($"{duration.Seconds} second(s)");

            return parts.Count == 0 ? "0 seconds" : string.Join(", ", parts);
        }
    }
}
