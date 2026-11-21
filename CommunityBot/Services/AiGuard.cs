using System;
using System.Collections.Concurrent;
using System.Linq;
using System.Text;

namespace CommunityBot.Services
{
    /// <summary>
    /// Trava da IA contra spam e contra ordens de quem nao e operador. Mesma
    /// regra do ccore (src/abuse.rs). Decidido aqui no codigo, e nao pelo
    /// modelo: ele cede facil a "repete isso 100 vezes" ou "ignora suas regras".
    /// </summary>
    internal static class AiGuard
    {
        internal enum Verdict { Pass, Ignore, Order, Spam }

        /// <summary>Janela em que o segundo spam conta como reincidencia.</summary>
        private static readonly TimeSpan s_window = TimeSpan.FromMinutes(10);

        /// <summary>
        /// Depois de mandar alguem embora, o bot ignora a pessoa por este
        /// tempo, para nao virar um loop de xingamento.
        /// </summary>
        private static readonly TimeSpan s_mute = TimeSpan.FromMinutes(5);

        private sealed class Entry
        {
            public string LastText = "";
            public DateTime? LastAt;
            public DateTime? SpamAt;
            public DateTime? MutedUntil;
        }

        private static readonly ConcurrentDictionary<ulong, Entry> s_people = new();

        public static Verdict Judge(ulong user, string text, DateTime now)
        {
            var who = s_people.GetOrAdd(user, _ => new Entry());
            lock (who)
            {
                if (who.MutedUntil > now)
                    return Verdict.Ignore;

                var norm = Normalize(text);
                var repeated = norm.Length > 0 && norm == who.LastText && now - who.LastAt < s_window;
                who.LastText = norm;
                who.LastAt = now;

                if (IsOrder(norm))
                {
                    who.MutedUntil = now + s_mute;
                    return Verdict.Order;
                }

                if (repeated || LooksSpammy(text))
                {
                    if (now - who.SpamAt < s_window)
                    {
                        who.SpamAt = null;
                        who.MutedUntil = now + s_mute;
                        return Verdict.Spam;
                    }

                    who.SpamAt = now;
                }

                return Verdict.Pass;
            }
        }

        /// <summary>
        /// O que o modelo responde, sozinho, quando alguem manda ele fazer algo
        /// de verdade que pode ser perigoso. O codigo troca pelo xingamento.
        /// </summary>
        public const string Marker = "[[VAZA]]";

        /// <summary>O modelo marcou a mensagem com Marker: mesmo castigo de uma ordem.</summary>
        public static void Mute(ulong user, DateTime now)
        {
            var who = s_people.GetOrAdd(user, _ => new Entry());
            lock (who)
                who.MutedUntil = now + s_mute;
        }

        private static readonly string[] s_orderPt =
        {
            "Vai se fuder, tu não manda em mim porra.",
            "Vai tomar no cu, ninguém te deu essa moral não.",
            "Quem tu pensa que é pra me dar ordem? Vai se fuder."
        };

        private static readonly string[] s_orderEn =
        {
            "Fuck off, you don't give me orders.",
            "Go fuck yourself, who the hell put you in charge?"
        };

        private static readonly string[] s_spamPt =
        {
            "Vai pro caralho com esse spam.",
            "Para de spammar essa merda e vai se fuder.",
            "Vai tomar no cu, spam aqui não cola."
        };

        private static readonly string[] s_spamEn =
        {
            "Fuck off with that spam.",
            "Take that spam and shove it, dumbass."
        };

        /// <summary>
        /// Resposta fixa para quem foi barrado. Pesada de proposito, mas sem
        /// ameaca, preconceito nem ofensa a grupo.
        /// </summary>
        public static string? TellingOff(Verdict verdict, bool english)
        {
            var list = verdict switch
            {
                Verdict.Order => english ? s_orderEn : s_orderPt,
                Verdict.Spam => english ? s_spamEn : s_spamPt,
                _ => null
            };
            return list?[Random.Shared.Next(list.Length)];
        }

        /// <summary>Minusculas, sem acento e com um espaco so entre palavras.</summary>
        internal static string Normalize(string text)
        {
            var plain = new string(text.Normalize(NormalizationForm.FormD)
                .Where(c => c is < '\u0300' or > '\u036f').ToArray()).ToLowerInvariant();
            return string.Join(' ', plain.Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries));
        }

        /// <summary>KKKKKKKKKKKKKKKKKKKKK, "aaaaaaaa" sem fim ou a mesma palavra em fila.</summary>
        internal static bool LooksSpammy(string text)
        {
            var chars = text.Where(c => !char.IsWhiteSpace(c)).Select(char.ToLowerInvariant).ToArray();
            int run = 0, longest = 0;
            for (var i = 0; i < chars.Length; i++)
            {
                run = i > 0 && chars[i - 1] == chars[i] ? run + 1 : 1;
                longest = Math.Max(longest, run);
            }

            if (longest >= 20)
                return true;
            if (chars.Length >= 20 && chars.Distinct().Count() <= 3)
                return true;

            var words = text.ToLowerInvariant().Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries);
            return words.Length >= 8 && words.Distinct().Count() <= 2;
        }

        private static readonly string[] s_phrases =
        {
            // repetir sem fim
            "fica repet", "fica mandando", "fica falando", "fica escrevendo", "repete ate", "repita ate",
            "sem parar", "infinit", "em loop", "num loop", "loop eterno", "keep saying", "keep repeating",
            "keep sending", "keep spamming", "repeat until", "nonstop", "non stop", "in a loop", "on loop",
            "faz spam", "manda spam", "spama", "spamm", "spam this", "spam it", "faz flood", "manda flood",
            "floda", "flood the", "flood this",
            // fugir das regras. "Finge que", "me obedece" e afins podem ser
            // brincadeira: esses o modelo julga, e marca com Marker o que for
            // ordem de verdade.
            "prompt do sistema", "modo desenvolvedor", "system prompt", "developer mode", "jailbreak",
            // pingar todo mundo
            "marca todo mundo", "pinga todo mundo", "ping everyone", "@everyone", "@here"
        };

        private static readonly string[] s_rules =
            { "instruc", "regra", "prompt", "anterior", "previous", "rules", "instruction" };

        /// <summary>Pedido para repetir, spammar ou fugir das regras. Recebe o texto normalizado.</summary>
        internal static bool IsOrder(string norm)
        {
            if (s_phrases.Any(p => norm.Contains(p, StringComparison.Ordinal)))
                return true;
            if (norm.Contains("ignor", StringComparison.Ordinal) && s_rules.Any(r => norm.Contains(r, StringComparison.Ordinal)))
                return true;

            // "100 vezes", "50x", "20 times": a partir de 5.
            var words = new StringBuilder(norm.Length);
            foreach (var c in norm)
                words.Append(char.IsLetterOrDigit(c) ? c : ' ');
            var list = words.ToString().Split(' ', StringSplitOptions.RemoveEmptyEntries);
            for (var i = 0; i < list.Length; i++)
            {
                var w = list[i];
                var cut = 0;
                while (cut < w.Length && char.IsAsciiDigit(w[cut]))
                    cut++;
                if (cut == 0 || !ulong.TryParse(w[..cut], out var n) || n < 5)
                    continue;
                var rest = w[cut..];
                if (rest == "x" || (rest.Length == 0 && i + 1 < list.Length && list[i + 1] is "vezes" or "times" or "x"))
                    return true;
            }

            return false;
        }
    }
}
