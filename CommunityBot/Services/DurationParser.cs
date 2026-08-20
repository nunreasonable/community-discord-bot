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
                error = "Informe uma duração, por exemplo `10m`, `2h30m` ou `1d`.";
                return false;
            }

            var total = TimeSpan.Zero;
            var number = new StringBuilder();
            var sawUnit = false;

            foreach (var ch in text)
            {
                if (char.IsDigit(ch))
                {
                    number.Append(ch);
                    continue;
                }

                if (ch == ' ')
                    continue;

                if (number.Length == 0)
                {
                    error = $"Faltou o número antes de `{ch}`. Use algo como `10m` ou `2h30m`.";
                    return false;
                }

                // long, nao int: "999999999999m" cabe aqui e estoura no
                // TimeSpan - e melhor cair no teto abaixo do que num overflow.
                if (!long.TryParse(number.ToString(), NumberStyles.None, CultureInfo.InvariantCulture, out var value))
                {
                    error = "Número grande demais.";
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
                        error = $"Unidade `{ch}` não existe. Use `s`, `m`, `h` ou `d`.";
                        return false;
                }

                try
                {
                    total += unit * value;
                }
                catch (OverflowException)
                {
                    error = "Duração grande demais.";
                    return false;
                }

                if (total > MaxTimeout)
                {
                    error = $"O Discord não aceita mais que {MaxTimeout.TotalDays:0} dias.";
                    return false;
                }

                number.Clear();
                sawUnit = true;
            }

            // Numero sem unidade no fim ("10") seria adivinhacao: minutos? horas?
            // Melhor recusar e dizer o formato.
            if (number.Length > 0)
            {
                error = "Falta a unidade no fim. Use `s`, `m`, `h` ou `d` — por exemplo `10m`.";
                return false;
            }

            if (!sawUnit || total <= TimeSpan.Zero)
            {
                error = "A duração precisa ser maior que zero.";
                return false;
            }

            duration = total;
            return true;
        }

        /// <summary>Escreve a duracao de volta em portugues, para o embed de confirmacao.</summary>
        public static string Describe(TimeSpan duration)
        {
            var parts = new System.Collections.Generic.List<string>();

            if (duration.Days > 0) parts.Add($"{duration.Days} dia(s)");
            if (duration.Hours > 0) parts.Add($"{duration.Hours} hora(s)");
            if (duration.Minutes > 0) parts.Add($"{duration.Minutes} minuto(s)");
            if (duration.Seconds > 0) parts.Add($"{duration.Seconds} segundo(s)");

            return parts.Count == 0 ? "0 segundos" : string.Join(", ", parts);
        }
    }
}
