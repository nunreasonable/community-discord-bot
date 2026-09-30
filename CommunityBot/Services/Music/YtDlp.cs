using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using CommunityBot.config;
using Newtonsoft.Json.Linq;

namespace CommunityBot.Services.Music
{
    /// <summary>
    /// Falha que tem uma frase pronta para quem usou o comando. A mensagem vai
    /// direto para o Discord, entao e sempre em ingles e nunca carrega caminho de
    /// arquivo nem saida crua de processo.
    /// </summary>
    internal sealed class MusicException : Exception
    {
        public MusicException(string userMessage) : base(userMessage) { }
    }

    /// <summary>
    /// A ponte com o yt-dlp.
    ///
    /// Duas regras valem para todo processo daqui: argumentos sempre por
    /// ArgumentList (nada de string montada, que seria injecao de opcao por
    /// titulo de video), e sempre um "--" antes do que veio do usuario, para que
    /// uma busca comecando com "-" nao vire flag.
    /// </summary>
    internal static class YtDlp
    {
        private static readonly TimeSpan s_resolveTimeout = TimeSpan.FromSeconds(45);
        private static readonly TimeSpan s_downloadTimeout = TimeSpan.FromMinutes(3);

        // Download e banda e disco desta maquina, compartilhados por todos os
        // servidores. Dois de cada vez cobrem o prefetch de dois servidores sem
        // deixar um /play em massa afogar a conexao.
        private static readonly SemaphoreSlim s_downloads = new(2, 2);

        // So hosts do YouTube. Qualquer outra URL e recusada ANTES de chegar ao
        // yt-dlp: o extrator generico dele baixa o que for apontado, inclusive
        // endereco interno desta rede - seria um SSRF com a cara do bot.
        private static readonly HashSet<string> s_youtubeHosts = new(StringComparer.OrdinalIgnoreCase)
        {
            "youtube.com", "www.youtube.com", "m.youtube.com", "music.youtube.com", "youtu.be"
        };

        /// <summary>
        /// Transforma o que a pessoa digitou no alvo do yt-dlp: a propria URL, se
        /// for do YouTube, ou uma busca de um resultado so.
        /// </summary>
        public static string BuildTarget(string input)
        {
            var text = input.Trim();

            if (text.Length == 0)
                throw new MusicException("Tell me what to play: a YouTube link or what to search for.");

            // Sem esquema tambem conta como link: quem cola "youtu.be/abc" nao
            // quer procurar pelo texto "youtu.be/abc".
            var candidate = text;
            if (!candidate.Contains("://", StringComparison.Ordinal) && !candidate.Contains(' ') &&
                candidate.Contains('.') && candidate.Contains('/'))
                candidate = "https://" + candidate;

            if (Uri.TryCreate(candidate, UriKind.Absolute, out var uri) &&
                (uri.Scheme == Uri.UriSchemeHttp || uri.Scheme == Uri.UriSchemeHttps))
            {
                if (!s_youtubeHosts.Contains(uri.Host))
                    throw new MusicException("I can only play YouTube links. You can also just type what to search for.");

                if (uri.AbsolutePath.StartsWith("/playlist", StringComparison.OrdinalIgnoreCase))
                    throw new MusicException("Playlists aren't supported. Send a link to a single video.");

                return uri.AbsoluteUri;
            }

            if (text.Contains("://", StringComparison.Ordinal))
                throw new MusicException("That doesn't look like a valid link.");

            return "ytsearch1:" + text;
        }

        /// <summary>
        /// Busca os metadados sem baixar nada. E aqui que se recusa live, video
        /// longo demais e qualquer coisa que nao seja do YouTube - o download
        /// depois parte SO da URL canonica devolvida aqui, nunca da entrada crua.
        /// </summary>
        public static async Task<TrackInfo> ResolveAsync(string input, MusicSettings settings, CancellationToken ct)
        {
            var target = BuildTarget(input);

            var args = BaseArgs(settings);
            args.AddRange(new[] { "--skip-download", "--dump-json", "--", target });

            var result = await RunAsync(settings.ytDlpPath, args, s_resolveTimeout, ct);

            var line = result.StdOut
                .Split('\n', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
                .FirstOrDefault(l => l.StartsWith('{'));

            if (line is null)
            {
                if (result.ExitCode == 0 && target.StartsWith("ytsearch1:", StringComparison.Ordinal))
                    throw new MusicException("I couldn't find anything for that search.");

                Console.WriteLine($"[musica] aviso: yt-dlp nao resolveu '{target}' (saida {result.ExitCode}): {LastLines(result.StdErr)}");
                throw new MusicException(Explain(result.StdErr));
            }

            JObject json;
            try
            {
                json = JObject.Parse(line);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[musica] aviso: JSON do yt-dlp ilegivel: {ex.Message}");
                throw new MusicException("YouTube sent back something I couldn't read. Try again.");
            }

            // Uma URL de youtube.com que redireciona para outro site (um link de
            // "attribution" antigo, por exemplo) cairia em outro extrator.
            var extractor = (string?)json["extractor_key"];
            if (!string.Equals(extractor, "Youtube", StringComparison.Ordinal))
                throw new MusicException("I can only play YouTube videos.");

            var liveStatus = (string?)json["live_status"];
            if ((bool?)json["is_live"] == true || liveStatus is "is_live" or "is_upcoming")
                throw new MusicException("Live streams and premieres can't be played — I'd have to download them first.");

            var id = (string?)json["id"];
            var url = (string?)json["webpage_url"];
            var seconds = (double?)json["duration"];

            if (string.IsNullOrWhiteSpace(id) || string.IsNullOrWhiteSpace(url) || seconds is null)
                throw new MusicException("That video is missing information I need. Try another one.");

            var duration = TimeSpan.FromSeconds(seconds.Value);
            if (duration > settings.MaxTrackLength)
                throw new MusicException(
                    $"That video is {MusicFormat.Duration(duration)} long; the limit is {MusicFormat.Duration(settings.MaxTrackLength)}.");

            return new TrackInfo(
                id,
                (string?)json["title"] ?? id,
                url,
                duration,
                (string?)json["channel"] ?? (string?)json["uploader"],
                (string?)json["thumbnail"]);
        }

        /// <summary>
        /// Baixa so o audio para dentro de <paramref name="directory"/> e devolve
        /// o caminho do arquivo. Quem chama e dono do arquivo e tem de apaga-lo.
        /// </summary>
        public static async Task<string> DownloadAsync(TrackInfo track, string directory, MusicSettings settings,
            CancellationToken ct)
        {
            Directory.CreateDirectory(directory);

            // Nome aleatorio, e nao o id do video: a mesma faixa pode estar
            // tocando e sendo pre-baixada ao mesmo tempo (loop de fila com uma
            // faixa so), e um apagaria o arquivo do outro.
            var stem = Guid.NewGuid().ToString("N");

            var args = BaseArgs(settings);
            args.AddRange(new[]
            {
                "--no-progress",
                "--format", "bestaudio/best",
                "--max-filesize", $"{settings.MaxFileSizeMegabytes}M",
                "--match-filters", $"duration <= {(int)settings.MaxTrackLength.TotalSeconds} & !is_live",
                "--paths", directory,
                "--output", stem + ".%(ext)s",
                "--print", "after_move:filepath",
                "--no-simulate",
                "--", track.WebpageUrl
            });

            await s_downloads.WaitAsync(ct);
            try
            {
                var result = await RunAsync(settings.ytDlpPath, args, s_downloadTimeout, ct);

                var printed = result.StdOut
                    .Split('\n', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
                    .LastOrDefault();

                var root = Path.GetFullPath(directory) + Path.DirectorySeparatorChar;

                // Confere que o caminho impresso e mesmo daqui de dentro antes de
                // entregar a quem vai apaga-lo depois.
                if (printed is not null && File.Exists(printed) && Path.GetFullPath(printed).StartsWith(root, StringComparison.Ordinal))
                    return printed;

                // Filtro de duracao/tamanho: o yt-dlp PULA o video e sai com 0,
                // sem imprimir caminho. Sobra no maximo um .part.
                DeleteMatching(directory, stem);

                Console.WriteLine($"[musica] aviso: download de {track.Id} nao gerou arquivo (saida {result.ExitCode}): {LastLines(result.StdErr)}");
                throw new MusicException(result.StdErr.Contains("larger than max-filesize", StringComparison.OrdinalIgnoreCase)
                    ? "That audio file is too large for me to download."
                    : Explain(result.StdErr));
            }
            catch (Exception) when (ct.IsCancellationRequested)
            {
                DeleteMatching(directory, stem);
                throw;
            }
            catch (TimeoutException)
            {
                DeleteMatching(directory, stem);
                throw new MusicException("The download took too long, so I gave up on it.");
            }
            finally
            {
                s_downloads.Release();
            }
        }

        /// <summary>Versao do yt-dlp, para o relatorio de subida. Lanca se nao houver yt-dlp.</summary>
        public static async Task<string> ProbeAsync(MusicSettings settings, CancellationToken ct)
        {
            var result = await RunAsync(settings.ytDlpPath, new[] { "--version" }, TimeSpan.FromSeconds(15), ct);
            if (result.ExitCode != 0)
                throw new InvalidOperationException($"yt-dlp --version saiu com {result.ExitCode}: {LastLines(result.StdErr)}");

            return result.StdOut.Trim();
        }

        private static List<string> BaseArgs(MusicSettings settings)
        {
            // --ignore-config: um ~/.config/yt-dlp/config desta conta nao pode
            // mudar o formato, a pasta ou ligar o download de playlist por baixo
            // do bot.
            var args = new List<string>
            {
                "--ignore-config", "--no-playlist", "--no-warnings", "--no-color",
                "--socket-timeout", "20"
            };

            if (!string.IsNullOrWhiteSpace(settings.jsRuntimes))
            {
                args.Add("--js-runtimes");
                args.Add(settings.jsRuntimes);
            }

            return args;
        }

        /// <summary>A frase para o usuario a partir do stderr do yt-dlp.</summary>
        private static string Explain(string stderr)
        {
            bool Has(string s) => stderr.Contains(s, StringComparison.OrdinalIgnoreCase);

            if (Has("confirm your age") || Has("age-restricted") || Has("inappropriate for some users"))
                return "That video is age-restricted, so I can't play it.";
            if (Has("Private video"))
                return "That video is private.";
            if (Has("not a bot") || Has("HTTP Error 429"))
                return "YouTube is blocking my downloads right now. Try again later.";
            if (Has("not available in your country") || Has("blocked it in your country"))
                return "That video isn't available where I'm hosted.";
            if (Has("members-only") || Has("Join this channel"))
                return "That video is for channel members only.";
            if (Has("Video unavailable") || Has("This video is unavailable") || Has("has been removed"))
                return "That video is unavailable.";

            return "I couldn't load that video. Try another link or search.";
        }

        private static string LastLines(string text) =>
            Embeds.Trim(string.Join(" | ", text
                .Split('\n', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
                .TakeLast(3)), 600);

        private static void DeleteMatching(string directory, string stem)
        {
            try
            {
                foreach (var file in Directory.EnumerateFiles(directory, stem + ".*"))
                    File.Delete(file);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[musica] aviso: nao consegui limpar {stem}.*: {ex.Message}");
            }
        }

        internal readonly record struct ProcessResult(int ExitCode, string StdOut, string StdErr);

        /// <summary>
        /// Roda um processo ate o fim, com prazo. Estourou o prazo ou cancelou,
        /// a arvore inteira morre: o yt-dlp abre o node (e as vezes o ffmpeg) por
        /// baixo, e matar so o pai deixaria os filhos segurando o arquivo.
        /// </summary>
        internal static async Task<ProcessResult> RunAsync(string executable, IEnumerable<string> args, TimeSpan timeout,
            CancellationToken ct)
        {
            var psi = new ProcessStartInfo(executable)
            {
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false,
                CreateNoWindow = true
            };
            foreach (var arg in args)
                psi.ArgumentList.Add(arg);

            using var process = new Process { StartInfo = psi };
            try
            {
                process.Start();
            }
            catch (Win32Exception ex)
            {
                throw new FileNotFoundException($"nao consegui executar '{executable}': {ex.Message}", executable, ex);
            }

            // Os dois fluxos sao lidos em paralelo com a espera: um stderr cheio e
            // nunca lido trava o processo filho no write.
            var stdout = process.StandardOutput.ReadToEndAsync(CancellationToken.None);
            var stderr = process.StandardError.ReadToEndAsync(CancellationToken.None);

            using var deadline = CancellationTokenSource.CreateLinkedTokenSource(ct);
            deadline.CancelAfter(timeout);

            try
            {
                await process.WaitForExitAsync(deadline.Token);
            }
            catch (OperationCanceledException)
            {
                Kill(process);

                // Com o processo morto os pipes fecham e as leituras terminam;
                // espera-las evita excecao nao observada.
                try { await Task.WhenAll(stdout, stderr).WaitAsync(TimeSpan.FromSeconds(5), CancellationToken.None); }
                catch { /* ja se esta saindo por erro */ }

                ct.ThrowIfCancellationRequested();
                throw new TimeoutException($"'{Path.GetFileName(executable)}' passou de {timeout.TotalSeconds:0}s");
            }

            return new ProcessResult(process.ExitCode, await stdout, await stderr);
        }

        internal static void Kill(Process process)
        {
            try
            {
                if (!process.HasExited)
                    process.Kill(entireProcessTree: true);
            }
            catch (Exception ex) when (ex is InvalidOperationException or Win32Exception or NotSupportedException)
            {
                // Terminou sozinho entre o HasExited e o Kill.
            }
        }
    }
}
