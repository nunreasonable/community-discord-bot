using System;
using System.Collections.Generic;
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
    /// A ponte com o yt-dlp, para YouTube e SoundCloud.
    ///
    /// Duas regras valem para todo processo daqui: argumentos sempre por
    /// ArgumentList (nada de string montada, que seria injecao de opcao por
    /// titulo de video), e sempre um "--" antes do alvo, para que uma busca
    /// comecando com "-" nao vire flag. O alvo em si nunca e uma URL qualquer:
    /// o MusicSources so deixa chegar aqui host do YouTube, do SoundCloud ou uma
    /// busca com prefixo fixo.
    /// </summary>
    internal static class YtDlp
    {
        private static readonly TimeSpan s_resolveTimeout = TimeSpan.FromSeconds(45);
        private static readonly TimeSpan s_downloadTimeout = TimeSpan.FromMinutes(3);

        // Download e banda e disco desta maquina, compartilhados por todos os
        // servidores. Dois de cada vez cobrem o prefetch de dois servidores sem
        // deixar um /play em massa afogar a conexao.
        private static readonly SemaphoreSlim s_downloads = new(2, 2);

        // ------------------------------------------------------------------
        // Metadados
        // ------------------------------------------------------------------

        /// <summary>
        /// Consulta uma faixa sem baixar nada, e recusa o que nao pode tocar:
        /// live, longa demais, previa de 30s do SoundCloud Go+, ou um extrator
        /// que nao seja o esperado.
        /// </summary>
        public static async Task<TrackInfo> ResolveAsync(string target, TrackSource source, MusicSettings settings,
            CancellationToken ct)
        {
            var result = await RunAsync(settings, ct, s_resolveTimeout, "--skip-download", "--dump-json", "--", target);

            var line = JsonLines(result.StdOut).FirstOrDefault();
            if (line is null)
            {
                if (result.ExitCode == 0 && IsSearch(target))
                    throw new MusicException("I couldn't find anything for that search.");

                Console.WriteLine($"[musica] aviso: yt-dlp nao resolveu '{target}' (saida {result.ExitCode}): {LastLines(result.StdErr)}");
                throw new MusicException(Explain(result.StdErr, source));
            }

            return Validate(line, source, settings) switch
            {
                (TrackInfo track, null) => track,
                (_, var reason) => throw new MusicException(reason!)
            };
        }

        /// <summary>
        /// Busca no SoundCloud e devolve so o que pode tocar, na ordem do
        /// resultado. Usada no casamento das faixas do Spotify.
        /// </summary>
        public static async Task<List<TrackInfo>> SearchSoundCloudAsync(string query, int count, MusicSettings settings,
            CancellationToken ct)
        {
            var result = await RunAsync(settings, ct, s_resolveTimeout, "--skip-download", "--dump-json", "--",
                $"scsearch{count}:{query}");

            return JsonLines(result.StdOut)
                .Select(l => Validate(l, TrackSource.SoundCloud, settings).Track)
                .Where(t => t is not null)
                .Select(t => t!)
                .ToList();
        }

        /// <summary>
        /// Ids de video da busca de MUSICAS do YouTube Music - a secao "songs"
        /// traz primeiro a gravacao oficial (os canais "- Topic"), que e o que o
        /// casamento com o Spotify quer. So a lista: nada e consultado a fundo.
        /// </summary>
        public static async Task<List<string>> SearchYouTubeMusicAsync(string query, int count, MusicSettings settings,
            CancellationToken ct)
        {
            // Host e caminho fixos; o texto do usuario so entra escapado no q=.
            var url = $"https://music.youtube.com/search?q={Uri.EscapeDataString(query)}#songs";
            var result = await RunAsync(settings, ct, s_resolveTimeout, allowPlaylist: true,
                "--flat-playlist", "--dump-single-json", "--playlist-items", $"1:{count}", "--", url);

            var json = JsonLines(result.StdOut).FirstOrDefault();
            if (json is null)
                return new List<string>();

            return (JObject.Parse(json)["entries"] as JArray ?? new JArray())
                .Select(e => (string?)e["id"])
                .Where(id => !string.IsNullOrWhiteSpace(id))
                .Select(id => id!)
                .ToList();
        }

        /// <summary>
        /// Lista um set (ou perfil) do SoundCloud sem consultar cada faixa: a
        /// listagem so traz as URLs, e o resto e buscado quando chegar a vez.
        /// </summary>
        public static async Task<(string? Title, List<string> Urls)> ListSoundCloudAsync(string url, int max,
            MusicSettings settings, CancellationToken ct)
        {
            var result = await RunAsync(settings, ct, s_resolveTimeout, allowPlaylist: true,
                "--flat-playlist", "--dump-single-json", "--playlist-items", $"1:{max}", "--", url);

            var json = JsonLines(result.StdOut).FirstOrDefault();
            if (json is null)
            {
                Console.WriteLine($"[musica] aviso: yt-dlp nao listou '{url}' (saida {result.ExitCode}): {LastLines(result.StdErr)}");
                throw new MusicException(Explain(result.StdErr, TrackSource.SoundCloud));
            }

            var obj = JObject.Parse(json);
            var urls = (obj["entries"] as JArray ?? new JArray())
                .Select(e => (string?)e["url"])
                .Where(u => u is not null && MusicSources.IsSoundCloudTrackUrl(u))
                .Select(u => u!)
                .ToList();

            return ((string?)obj["title"], urls);
        }

        // ------------------------------------------------------------------
        // Audio
        // ------------------------------------------------------------------

        /// <summary>
        /// URL do audio de uma faixa do SoundCloud, para o ffmpeg ler direto.
        /// Pedida na hora de tocar, e nao guardada: os links de HLS do SoundCloud
        /// expiram.
        /// </summary>
        public static async Task<string> StreamUrlAsync(TrackInfo track, MusicSettings settings, CancellationToken ct)
        {
            var result = await RunAsync(settings, ct, s_resolveTimeout, "--format", "bestaudio/best", "--get-url",
                "--", track.WebpageUrl);

            var url = result.StdOut
                .Split('\n', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
                .FirstOrDefault();

            // So https: o ffmpeg vai abrir isto, e com a lista de protocolos
            // restrita a https ele nao aceitaria outra coisa de qualquer forma.
            if (!Uri.TryCreate(url, UriKind.Absolute, out var uri) || uri.Scheme != Uri.UriSchemeHttps)
            {
                Console.WriteLine($"[musica] aviso: sem URL de audio para {track.WebpageUrl} (saida {result.ExitCode}): {LastLines(result.StdErr)}");
                throw new MusicException(Explain(result.StdErr, TrackSource.SoundCloud));
            }

            return uri.AbsoluteUri;
        }

        /// <summary>
        /// Baixa so o audio de um video do YouTube para dentro de
        /// <paramref name="directory"/> e devolve o caminho. Quem chama e dono do
        /// arquivo e tem de apaga-lo.
        /// </summary>
        public static async Task<string> DownloadAsync(TrackInfo track, string directory, MusicSettings settings,
            CancellationToken ct)
        {
            Directory.CreateDirectory(directory);

            // Nome aleatorio, e nao o id do video: a mesma faixa pode estar
            // tocando e sendo pre-baixada ao mesmo tempo (loop de fila com uma
            // faixa so), e um apagaria o arquivo do outro.
            var stem = Guid.NewGuid().ToString("N");

            await s_downloads.WaitAsync(ct);
            try
            {
                var result = await RunAsync(settings, ct, s_downloadTimeout,
                    "--no-progress",
                    "--format", "bestaudio/best",
                    "--max-filesize", $"{settings.MaxFileSizeMegabytes}M",
                    "--match-filters", $"duration <= {(int)settings.MaxTrackLength.TotalSeconds} & !is_live",
                    "--paths", directory,
                    "--output", stem + ".%(ext)s",
                    "--print", "after_move:filepath",
                    "--no-simulate",
                    "--", track.WebpageUrl);

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
                    : Explain(result.StdErr, TrackSource.YouTube));
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
            var result = await ProcessRunner.RunAsync(settings.ytDlpPath, new[] { "--version" }, TimeSpan.FromSeconds(15), ct);
            if (result.ExitCode != 0)
                throw new InvalidOperationException($"yt-dlp --version saiu com {result.ExitCode}: {LastLines(result.StdErr)}");

            return result.StdOut.Trim();
        }

        // ------------------------------------------------------------------
        // Interno
        // ------------------------------------------------------------------

        private static Task<ProcessRunner.ProcessResult> RunAsync(MusicSettings settings, CancellationToken ct,
            TimeSpan timeout, params string[] args) =>
            RunAsync(settings, ct, timeout, allowPlaylist: false, args);

        private static Task<ProcessRunner.ProcessResult> RunAsync(MusicSettings settings, CancellationToken ct,
            TimeSpan timeout, bool allowPlaylist, params string[] args)
        {
            // --ignore-config: um ~/.config/yt-dlp/config desta conta nao pode
            // mudar o formato, a pasta ou ligar o download de playlist por baixo
            // do bot.
            var all = new List<string>
            {
                "--ignore-config", "--no-warnings", "--no-color", "--socket-timeout", "20"
            };

            // Listagem de set e busca do YouTube Music SAO playlists; o resto
            // nunca pode virar uma (um link de video com &list= baixaria a lista).
            if (!allowPlaylist)
                all.Add("--no-playlist");

            if (!string.IsNullOrWhiteSpace(settings.jsRuntimes))
            {
                all.Add("--js-runtimes");
                all.Add(settings.jsRuntimes);
            }

            all.AddRange(args);
            return ProcessRunner.RunAsync(settings.ytDlpPath, all, timeout, ct);
        }

        /// <summary>O JSON de uma faixa virou TrackInfo, ou a frase de por que ela nao toca.</summary>
        private static (TrackInfo? Track, string? Reason) Validate(string line, TrackSource source, MusicSettings settings)
        {
            JObject json;
            try
            {
                json = JObject.Parse(line);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[musica] aviso: JSON do yt-dlp ilegivel: {ex.Message}");
                return (null, "I got back something I couldn't read. Try again.");
            }

            // Uma URL que redireciona para outro site cairia em outro extrator;
            // e o que o allowlist de host nao ve.
            var expected = source == TrackSource.SoundCloud ? "Soundcloud" : "Youtube";
            if (!string.Equals((string?)json["extractor_key"], expected, StringComparison.Ordinal))
                return (null, source == TrackSource.SoundCloud
                    ? "That link isn't a SoundCloud track."
                    : "That link isn't a YouTube video.");

            var liveStatus = (string?)json["live_status"];
            if ((bool?)json["is_live"] == true || liveStatus is "is_live" or "is_upcoming")
                return (null, "Live streams and premieres can't be played.");

            // SoundCloud Go+: sem assinatura, so vem uma previa de 30s. Tocar
            // isso como se fosse a musica seria pior que recusar.
            if (source == TrackSource.SoundCloud &&
                json["formats"] is JArray formats && formats.Count > 0 &&
                formats.All(f => ((string?)f["format_id"])?.Contains("preview", StringComparison.OrdinalIgnoreCase) == true))
                return (null, "That SoundCloud track is Go+ only — SoundCloud just gives me a 30-second preview.");

            var id = (string?)json["id"];
            var url = (string?)json["webpage_url"];
            var seconds = (double?)json["duration"];

            if (string.IsNullOrWhiteSpace(id) || string.IsNullOrWhiteSpace(url) || seconds is null or <= 0)
                return (null, "That track is missing information I need. Try another one.");

            var duration = TimeSpan.FromSeconds(seconds.Value);
            var limit = source == TrackSource.YouTube ? settings.MaxTrackLength : settings.MaxStreamLength;
            if (duration > limit)
                return (null, $"That track is {MusicFormat.Duration(duration)} long; the limit is {MusicFormat.Duration(limit)}.");

            return (new TrackInfo(
                id,
                (string?)json["title"] ?? id,
                url,
                duration,
                (string?)json["channel"] ?? (string?)json["uploader"],
                (string?)json["thumbnail"],
                source), null);
        }

        private static bool IsSearch(string target) =>
            target.StartsWith("ytsearch", StringComparison.Ordinal) || target.StartsWith("scsearch", StringComparison.Ordinal);

        private static IEnumerable<string> JsonLines(string stdout) =>
            stdout.Split('\n', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
                .Where(l => l.StartsWith('{'));

        /// <summary>A frase para o usuario a partir do stderr do yt-dlp.</summary>
        private static string Explain(string stderr, TrackSource source)
        {
            bool Has(string s) => stderr.Contains(s, StringComparison.OrdinalIgnoreCase);
            var site = source == TrackSource.SoundCloud ? "SoundCloud" : "YouTube";

            if (Has("confirm your age") || Has("age-restricted") || Has("inappropriate for some users"))
                return "That video is age-restricted, so I can't play it.";
            if (Has("Private video") || Has("private"))
                return "That track is private.";
            if (Has("not a bot") || Has("HTTP Error 429"))
                return $"{site} is blocking me right now. Try again later.";
            if (Has("not available in your country") || Has("blocked it in your country") || Has("geo restrict"))
                return "That track isn't available where I'm hosted.";
            if (Has("members-only") || Has("Join this channel"))
                return "That video is for channel members only.";
            if (Has("Video unavailable") || Has("This video is unavailable") || Has("has been removed") ||
                Has("HTTP Error 404"))
                return "That track is unavailable.";

            return "I couldn't load that. Try another link or search.";
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
    }
}
