using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Text;

namespace CommunityBot.Services.Fun
{
    /// <summary>
    /// As transformacoes do /text e o codigo Morse do /morse.
    ///
    /// Tudo aqui trabalha por elemento de texto (StringInfo), e nao por char: um
    /// emoji ou letra com acento combinado e mais de um char, e inverter ou
    /// separar char a char quebraria o par e deixaria "?" na tela.
    /// </summary>
    internal static class TextTransforms
    {
        private static IEnumerable<string> Elements(string text)
        {
            var e = StringInfo.GetTextElementEnumerator(text);
            while (e.MoveNext())
                yield return e.GetTextElement();
        }

        /// <summary>ｖａｐｏｒｗａｖｅ: ASCII visivel vai para a forma de largura cheia.</summary>
        public static string Vaporwave(string text)
        {
            var sb = new StringBuilder(text.Length);
            foreach (var c in text)
            {
                if (c == ' ')
                    sb.Append('\u3000');
                else if (c >= '!' && c <= '~')
                    sb.Append((char)(c + 0xFEE0));
                else
                    sb.Append(c);
            }

            return sb.ToString();
        }

        /// <summary>Q U A L I T Y</summary>
        public static string Quality(string text) =>
            string.Join(' ', Elements(text.ToUpperInvariant()).Where(e => !string.IsNullOrWhiteSpace(e)));

        /// <summary>👏 clap 👏 between 👏 words 👏</summary>
        public static string Clap(string text, string emoji)
        {
            var words = text.Split(' ', StringSplitOptions.RemoveEmptyEntries);
            return words.Length == 0 ? emoji : $"{emoji} {string.Join($" {emoji} ", words)} {emoji}";
        }

        /// <summary>
        /// mOcKiNg TeXt. A caixa de cada letra sai de uma semente do proprio
        /// texto, entao o mesmo texto sempre zomba do mesmo jeito.
        /// </summary>
        public static string Mock(string text)
        {
            var random = new Random((int)StableHash.Of(text));
            var sb = new StringBuilder(text.Length);
            foreach (var c in text)
                sb.Append(random.Next(2) == 0 ? char.ToLowerInvariant(c) : char.ToUpperInvariant(c));
            return sb.ToString();
        }

        public static string Reverse(string text) => string.Concat(Elements(text).Reverse());

        private static readonly Dictionary<char, char> s_flip = BuildFlipTable();

        private static Dictionary<char, char> BuildFlipTable()
        {
            const string from = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.,!?'\"()[]{}<>&_;";
            const string to   = "ɐqɔpǝɟƃɥᴉɾʞlɯuodbɹsʇnʌʍxʎz∀ꓭƆꓷƎℲ⅁HIſꓘ˥WNOԀꝹꓤSꓕՈΛMX⅄Z0⇂ᄅƐㄣϛ9ㄥ86˙'¡¿,„)(][}{><⅋‾؛";

            var map = new Dictionary<char, char>();
            var targets = Elements(to).ToList();
            for (var i = 0; i < from.Length && i < targets.Count; i++)
            {
                // So mapeia o que cabe num char; o resto fica como esta.
                if (targets[i].Length == 1)
                    map[from[i]] = targets[i][0];
            }

            return map;
        }

        /// <summary>ɐʍoɹp ǝpᴉsdn: cada letra virada, e a frase de tras para a frente.</summary>
        public static string UpsideDown(string text) =>
            string.Concat(Elements(text).Reverse().Select(e => e.Length == 1 && s_flip.TryGetValue(e[0], out var f) ? f.ToString() : e));

        // ------------------------------------------------------------------
        // Morse
        // ------------------------------------------------------------------

        private static readonly Dictionary<char, string> s_morse = new()
        {
            ['A'] = ".-", ['B'] = "-...", ['C'] = "-.-.", ['D'] = "-..", ['E'] = ".", ['F'] = "..-.",
            ['G'] = "--.", ['H'] = "....", ['I'] = "..", ['J'] = ".---", ['K'] = "-.-", ['L'] = ".-..",
            ['M'] = "--", ['N'] = "-.", ['O'] = "---", ['P'] = ".--.", ['Q'] = "--.-", ['R'] = ".-.",
            ['S'] = "...", ['T'] = "-", ['U'] = "..-", ['V'] = "...-", ['W'] = ".--", ['X'] = "-..-",
            ['Y'] = "-.--", ['Z'] = "--..",
            ['0'] = "-----", ['1'] = ".----", ['2'] = "..---", ['3'] = "...--", ['4'] = "....-",
            ['5'] = ".....", ['6'] = "-....", ['7'] = "--...", ['8'] = "---..", ['9'] = "----.",
            ['.'] = ".-.-.-", [','] = "--..--", ['?'] = "..--..", ['\''] = ".----.", ['!'] = "-.-.--",
            ['/'] = "-..-.", ['('] = "-.--.", [')'] = "-.--.-", ['&'] = ".-...", [':'] = "---...",
            [';'] = "-.-.-.", ['='] = "-...-", ['+'] = ".-.-.", ['-'] = "-....-", ['_'] = "..--.-",
            ['"'] = ".-..-.", ['$'] = "...-..-", ['@'] = ".--.-."
        };

        private static readonly Dictionary<string, char> s_morseBack =
            s_morse.ToDictionary(kv => kv.Value, kv => kv.Key);

        /// <summary>Letras separadas por espaco, palavras por " / ". Devolve tambem o que nao tem codigo.</summary>
        public static (string Code, IReadOnlyCollection<string> Unknown) MorseEncode(string text)
        {
            var unknown = new SortedSet<string>(StringComparer.Ordinal);
            var words = new List<string>();

            // Remove acento antes: "é" vira "E" em vez de ser descartado.
            var plain = RemoveDiacritics(text).ToUpperInvariant();

            foreach (var word in plain.Split(' ', StringSplitOptions.RemoveEmptyEntries))
            {
                var letters = new List<string>();
                foreach (var element in Elements(word))
                {
                    if (element.Length == 1 && s_morse.TryGetValue(element[0], out var code))
                        letters.Add(code);
                    else
                        unknown.Add(element);
                }

                if (letters.Count > 0)
                    words.Add(string.Join(' ', letters));
            }

            return (string.Join(" / ", words), unknown);
        }

        public static (string Text, int Unknown) MorseDecode(string code)
        {
            // Aceita os pontos e tracos "bonitos" que teclado de celular e
            // tradutor online costumam colar no lugar de . e -.
            var normalized = code
                .Replace('·', '.').Replace('•', '.').Replace('∙', '.')
                .Replace('−', '-').Replace('–', '-').Replace('—', '-').Replace('_', '-')
                .Replace("|", " / ");

            var unknown = 0;
            var words = new List<string>();

            foreach (var word in normalized.Split('/', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries))
            {
                var sb = new StringBuilder();
                foreach (var letter in word.Split(' ', StringSplitOptions.RemoveEmptyEntries))
                {
                    if (s_morseBack.TryGetValue(letter, out var c))
                        sb.Append(c);
                    else
                    {
                        sb.Append('�');
                        unknown++;
                    }
                }

                if (sb.Length > 0)
                    words.Add(sb.ToString());
            }

            return (string.Join(' ', words), unknown);
        }

        private static string RemoveDiacritics(string text)
        {
            var decomposed = text.Normalize(NormalizationForm.FormD);
            var sb = new StringBuilder(decomposed.Length);
            foreach (var c in decomposed)
            {
                if (CharUnicodeInfo.GetUnicodeCategory(c) != UnicodeCategory.NonSpacingMark)
                    sb.Append(c);
            }

            return sb.ToString().Normalize(NormalizationForm.FormC);
        }
    }
}
