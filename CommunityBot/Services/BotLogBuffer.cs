using System;
using System.Collections.Generic;
using System.Linq;

namespace CommunityBot.Services
{
    /// <summary>Severidade inferida de uma linha de log.</summary>
    internal enum LogLevelTag
    {
        Info,
        Aviso,
        Erro
    }

    /// <summary>Uma linha ja classificada, pronta para exibicao.</summary>
    internal sealed record LogLine(DateTimeOffset TimestampUtc, LogLevelTag Level, string Tag, string Text);

    /// <summary>
    /// Buffer circular das ultimas linhas que o bot escreveu no console.
    ///
    /// Por que um buffer em memoria e nao um arquivo ou o journald: o bot ja
    /// escreve tudo com Console.WriteLine, e essa saida so era legivel com
    /// acesso SSH a maquina (`journalctl --user -u community-bot`). Guardando as
    /// linhas aqui, o /logs e o dashboard leem o mesmo historico sem depender
    /// de como o processo foi iniciado - servico, `dotnet run` ou terminal.
    ///
    /// O stdout continua intacto (ver <see cref="ConsoleTee"/>), entao o
    /// journald tambem continua recebendo tudo. Isto e uma adicao, nao uma
    /// substituicao.
    /// </summary>
    internal static class BotLogBuffer
    {
        /// <summary>
        /// Quantas linhas ficam guardadas. Mil linhas cobrem folgadamente o que
        /// se quer olhar depois de um incidente e mantem o custo em memoria na
        /// casa de centenas de KB, mesmo com linhas longas de stack trace.
        /// </summary>
        public const int Capacity = 1000;

        /// <summary>
        /// Teto por linha. Um stack trace inteiro numa unica linha encheria o
        /// buffer sozinho e ainda estouraria o limite de embed do Discord.
        /// </summary>
        private const int MaxLineLength = 500;

        private static readonly Queue<LogLine> s_lines = new(Capacity);
        private static readonly object s_lock = new();

        /// <summary>Total de linhas ja capturadas desde que o processo subiu.</summary>
        private static long s_totalSeen;

        public static long TotalSeen
        {
            get { lock (s_lock) return s_totalSeen; }
        }

        /// <summary>
        /// Registra uma linha. NUNCA lanca: e chamada de dentro do
        /// Console.Out, e uma excecao aqui apareceria em qualquer ponto do bot
        /// que escreva log - inclusive nos proprios tratadores de erro. Mesmo
        /// criterio do canario do ThreadPool em Program.cs.
        /// </summary>
        public static void Append(string? line)
        {
            try
            {
                if (line is null)
                    return;

                var text = line.TrimEnd('\r', '\n');
                if (string.IsNullOrWhiteSpace(text))
                    return;

                // Embeds.Trim, e nao Substring: o corte cru podia cair no meio de
                // um par substituto (um emoji num nome de canal, por exemplo) e
                // deixar meio caractere, que o Discord mostra como "<?>". O Trim
                // ja resolve isso e a regra passa a existir num lugar so.
                text = Embeds.Trim(text, MaxLineLength);

                var entry = new LogLine(DateTimeOffset.UtcNow, Classify(text), ExtractTag(text), text);

                lock (s_lock)
                {
                    if (s_lines.Count >= Capacity)
                        s_lines.Dequeue();

                    s_lines.Enqueue(entry);
                    s_totalSeen++;
                }
            }
            catch
            {
                // Diagnostico nunca pode derrubar o processo nem mascarar o erro
                // original que estava sendo logado.
            }
        }

        /// <summary>
        /// Recorte do buffer, do mais recente para o mais antigo - que e a ordem
        /// em que se quer ler um log ao investigar algo.
        /// </summary>
        public static List<LogLine> Snapshot(int take, LogLevelTag? level = null, string? contains = null)
        {
            LogLine[] all;
            lock (s_lock)
            {
                all = s_lines.ToArray();
            }

            IEnumerable<LogLine> query = all.Reverse();

            if (level.HasValue)
                query = query.Where(l => l.Level == level.Value);

            if (!string.IsNullOrWhiteSpace(contains))
                query = query.Where(l => l.Text.Contains(contains, StringComparison.OrdinalIgnoreCase));

            return query.Take(Math.Clamp(take, 1, Capacity)).ToList();
        }

        /// <summary>Quantas linhas de cada nivel estao no buffer agora.</summary>
        public static (int Info, int Aviso, int Erro) Counts()
        {
            LogLine[] all;
            lock (s_lock)
            {
                all = s_lines.ToArray();
            }

            return (
                all.Count(l => l.Level == LogLevelTag.Info),
                all.Count(l => l.Level == LogLevelTag.Aviso),
                all.Count(l => l.Level == LogLevelTag.Erro));
        }

        /// <summary>
        /// Le a etiqueta que o codebase ja usa a mao no inicio da linha
        /// ("[gw]", "[dashboard]", "[audit]"...). Devolve string vazia quando a
        /// linha nao segue a convencao.
        /// </summary>
        private static string ExtractTag(string text)
        {
            if (text.Length < 3 || text[0] != '[')
                return string.Empty;

            var end = text.IndexOf(']');
            if (end <= 1 || end > 24)
                return string.Empty;

            return text.Substring(1, end - 1);
        }

        /// <summary>
        /// Classificacao por palavra-chave. O bot nao usa biblioteca de log, e
        /// portanto nao ha severidade real para ler: o que existe sao as
        /// convencoes de texto que os ~84 Console.WriteLine ja seguem. A
        /// heuristica cobre essas convencoes e erra para o lado de INFO, que e
        /// o barulho aceitavel.
        /// </summary>
        public static LogLevelTag Classify(string text)
        {
            if (Has(text, "[fatal]") || Has(text, "[unobserved]") ||
                Has(text, "erro") || Has(text, "falha") ||
                Has(text, "exception") || Has(text, "unhandled") ||
                // O disjuntor do vigia de canal desliga o softban automatico ate
                // o restart. E a linha mais importante que aquele codigo produz e
                // nao casava com nenhuma palavra daqui - ficava arquivada como
                // INFO, invisivel justamente em `/logs nivel:erro`.
                Has(text, "disjuntor"))
                return LogLevelTag.Erro;

            if (Has(text, "[pool]") || Has(text, "zumbi") ||
                Has(text, "lento") || Has(text, "socket fechado") ||
                Has(text, "descartad") || Has(text, "aviso"))
                return LogLevelTag.Aviso;

            return LogLevelTag.Info;
        }

        private static bool Has(string haystack, string needle) =>
            haystack.Contains(needle, StringComparison.OrdinalIgnoreCase);

        /// <summary>Nome curto do nivel, usado no /logs e na API.</summary>
        public static string LevelName(LogLevelTag level) => level switch
        {
            LogLevelTag.Erro => "ERRO",
            LogLevelTag.Aviso => "AVISO",
            _ => "INFO"
        };

        /// <summary>Converte o texto vindo do usuario/API num nivel. Null = todos.</summary>
        public static LogLevelTag? ParseLevel(string? raw)
        {
            if (string.IsNullOrWhiteSpace(raw))
                return null;

            return raw.Trim().ToLowerInvariant() switch
            {
                "erro" or "error" => LogLevelTag.Erro,
                "aviso" or "warn" or "warning" => LogLevelTag.Aviso,
                "info" => LogLevelTag.Info,
                _ => null
            };
        }
    }
}
