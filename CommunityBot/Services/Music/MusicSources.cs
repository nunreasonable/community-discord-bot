using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Net.Http;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using CommunityBot.config;

namespace CommunityBot.Services.Music
{
    /// <summary>
    /// Transforma o que alguem digitou no /play em faixas, e as faixas ainda
    /// sem audio (Spotify, entrada de set) em algo que toca.
    ///
    /// E o unico lugar que decide que URL segue adiante. So passam hosts do
    /// YouTube, do SoundCloud e do Spotify; o resto e recusado antes de chegar
    /// ao yt-dlp, cujo extrator generico baixaria qualquer endereco - inclusive
    /// interno desta rede.
    /// </summary>
    internal static class MusicSources
    {
        internal sealed record Request(List<TrackInfo> Tracks, string? CollectionName, TrackSource Source, int Truncated);

        private static readonly HashSet<string> s_youtubeHosts = new(StringComparer.OrdinalIgnoreCase)
        {
            "youtube.com", "www.youtube.com", "m.youtube.com", "music.youtube.com", "youtu.be"
        };

        private static readonly HashSet<string> s_soundCloudHosts = new(StringComparer.OrdinalIgnoreCase)
        {
            "soundcloud.com", "www.soundcloud.com", "m.soundcloud.com"
        };

        private static readonly HashSet<string> s_spotifyHosts = new(StringComparer.OrdinalIgnoreCase)
        {
            "open.spotify.com", "play.spotify.com"
        };

        // Links curtos: so redirecionam. O destino final passa pelas mesmas
        // listas acima.
        private static readonly HashSet<string> s_shortHosts = new(StringComparer.OrdinalIgnoreCase)
        {
            "on.soundcloud.com", "spotify.link", "spotify.app.link"
        };

        // Segundo trecho de uma URL do SoundCloud que NAO e faixa: perfil, set,
        // curtidas etc. soundcloud.com/artista/faixa e o formato de uma faixa.
        private static readonly HashSet<string> s_soundCloudNonTrack = new(StringComparer.OrdinalIgnoreCase)
        {
            "sets", "likes", "tracks", "albums", "reposts", "popular-tracks", "followers", "following",
            "comments", "spotlight", "toptracks"
        };

        // A API do SoundCloud: sets grandes listam as faixas a partir da sexta
        // como api-v2.soundcloud.com/tracks/ID, e o yt-dlp resolve esse formato.
        private static readonly HashSet<string> s_soundCloudApiHosts = new(StringComparer.OrdinalIgnoreCase)
        {
            "api.soundcloud.com", "api-v2.soundcloud.com"
        };

        // Versao que nao e a musica: se aparece no SoundCloud e nao no titulo do
        // Spotify, o candidato e descartado mesmo com a duracao batendo - um
        // backing track tem, por definicao, a duracao do original.
        private static readonly string[] s_variantWords =
        {
            "backing track", "karaoke", "instrumental", "cover", "remix", "sped up", "slowed", "nightcore",
            "reverb", "8d", "acapella", "a cappella", "live", "acoustic", "tribute", "bass boosted", "mashup",
            "bootleg", "type beat"
        };

        private const string Unsupported =
            "I can play **YouTube**, **SoundCloud** and **Spotify** links. You can also just type what to search for.";

        // ------------------------------------------------------------------
        // Pedido
        // ------------------------------------------------------------------

        /// <summary>
        /// As faixas de um /play. Link de faixa do YouTube ou do SoundCloud ja
        /// sai consultado (e recusado aqui mesmo se nao puder tocar); playlist,
        /// album e set saem como lista, e cada faixa so e buscada na vez dela.
        /// </summary>
        public static async Task<Request> ResolveAsync(string input, TrackSource searchSource, MusicSettings settings,
            CancellationToken ct)
        {
            var text = input.Trim();
            if (text.Length == 0)
                throw new MusicException("Tell me what to play: a link or what to search for.");

            if (text.StartsWith("spotify:", StringComparison.OrdinalIgnoreCase))
                return await FromSpotifyAsync(text, settings, ct);

            // Sem esquema tambem conta como link: quem cola "youtu.be/abc" nao
            // quer procurar pelo texto "youtu.be/abc".
            var candidate = text;
            if (!candidate.Contains("://", StringComparison.Ordinal) && !candidate.Contains(' ') &&
                candidate.Contains('.') && candidate.Contains('/'))
                candidate = "https://" + candidate;

            if (Uri.TryCreate(candidate, UriKind.Absolute, out var uri) &&
                (uri.Scheme == Uri.UriSchemeHttp || uri.Scheme == Uri.UriSchemeHttps))
            {
                if (s_shortHosts.Contains(uri.Host))
                    uri = await ExpandShortLinkAsync(uri, ct);

                if (s_youtubeHosts.Contains(uri.Host))
                {
                    if (uri.AbsolutePath.StartsWith("/playlist", StringComparison.OrdinalIgnoreCase))
                        throw new MusicException("YouTube playlists aren't supported. Send a link to a single video.");

                    var video = await YtDlp.ResolveAsync(uri.AbsoluteUri, TrackSource.YouTube, settings, ct);
                    return new Request(new List<TrackInfo> { video }, null, TrackSource.YouTube, 0);
                }

                if (s_soundCloudHosts.Contains(uri.Host))
                    return await FromSoundCloudAsync(uri, settings, ct);

                if (s_spotifyHosts.Contains(uri.Host))
                    return await FromSpotifyAsync(uri.AbsoluteUri, settings, ct);

                throw new MusicException(Unsupported);
            }

            if (text.Contains("://", StringComparison.Ordinal))
                throw new MusicException("That doesn't look like a valid link.");

            // Busca por texto.
            if (searchSource == TrackSource.SoundCloud)
            {
                // Tres bastam: cada resultado e uma consulta inteira ao SoundCloud.
                var found = await YtDlp.SearchSoundCloudAsync(text, 3, settings, ct);
                if (found.Count == 0)
                    throw new MusicException("I couldn't find anything playable on SoundCloud for that.");

                return new Request(new List<TrackInfo> { found[0] }, null, TrackSource.SoundCloud, 0);
            }

            var result = await YtDlp.ResolveAsync("ytsearch1:" + text, TrackSource.YouTube, settings, ct);
            return new Request(new List<TrackInfo> { result }, null, TrackSource.YouTube, 0);
        }

        private static async Task<Request> FromSoundCloudAsync(Uri uri, MusicSettings settings, CancellationToken ct)
        {
            var url = uri.GetLeftPart(UriPartial.Path);

            if (IsSoundCloudTrackUrl(url))
            {
                var track = await YtDlp.ResolveAsync(url, TrackSource.SoundCloud, settings, ct);
                return new Request(new List<TrackInfo> { track }, null, TrackSource.SoundCloud, 0);
            }

            // Set, perfil, curtidas: a listagem so traz URLs. Pede uma a mais
            // que o limite para saber se houve corte.
            var (title, urls) = await YtDlp.ListSoundCloudAsync(url, settings.MaxPlaylist + 1, settings, ct);
            if (urls.Count == 0)
                throw new MusicException("I couldn't find any tracks in that SoundCloud link.");

            var truncated = Math.Max(0, urls.Count - settings.MaxPlaylist);
            var tracks = urls.Take(settings.MaxPlaylist).Select(Placeholder).ToList();
            return new Request(tracks, title ?? "SoundCloud set", TrackSource.SoundCloud, truncated);
        }

        private static async Task<Request> FromSpotifyAsync(string link, MusicSettings settings, CancellationToken ct)
        {
            if (!Spotify.TryParse(link, out var kind, out var id))
                throw new MusicException("Send a Spotify **track**, **album** or **playlist** link. Artists and podcasts aren't supported.");

            var collection = await Spotify.FetchAsync(kind, id, ct);
            if (collection.Tracks.Count == 0)
                throw new MusicException("That Spotify link has no playable tracks.");

            var truncated = Math.Max(0, collection.Tracks.Count - settings.MaxPlaylist);
            var tracks = collection.Tracks
                .Take(settings.MaxPlaylist)
                .Select(t => new TrackInfo(t.Id, t.Title, Spotify.TrackUrl(t.Id), t.Duration,
                    string.IsNullOrWhiteSpace(t.Artists) ? null : t.Artists, t.ImageUrl, TrackSource.Spotify))
                .ToList();

            // Faixa avulsa longa demais nao cabe em fonte nenhuma: recusa ja.
            if (kind == "track" && tracks[0].Duration > settings.MaxStreamLength)
                throw new MusicException(
                    $"That track is {MusicFormat.Duration(tracks[0].Duration)} long; the limit is {MusicFormat.Duration(settings.MaxStreamLength)}.");

            return new Request(tracks, kind == "track" ? null : collection.Name ?? "Spotify " + kind,
                TrackSource.Spotify, truncated);
        }

        /// <summary>
        /// Segue o redirecionamento de um link curto e devolve o destino. Pelo
        /// cliente compartilhado, que ja recusa endereco interno - e o destino
        /// ainda passa pelas listas de host de quem chamou.
        /// </summary>
        private static async Task<Uri> ExpandShortLinkAsync(Uri uri, CancellationToken ct)
        {
            using var cts = CancellationTokenSource.CreateLinkedTokenSource(ct);
            cts.CancelAfter(TimeSpan.FromSeconds(10));

            try
            {
                using var response = await HttpClientProvider.Shared.GetAsync(uri,
                    HttpCompletionOption.ResponseHeadersRead, cts.Token);

                var final = response.RequestMessage?.RequestUri;
                if (!response.IsSuccessStatusCode || final is null || s_shortHosts.Contains(final.Host))
                    throw new MusicException("That short link didn't lead anywhere I can play.");

                return final;
            }
            catch (Exception ex) when (ex is HttpRequestException or OperationCanceledException && !ct.IsCancellationRequested)
            {
                throw new MusicException("I couldn't open that short link. Paste the full link instead.");
            }
        }

        public static bool IsSoundCloudTrackUrl(string url)
        {
            if (!Uri.TryCreate(url, UriKind.Absolute, out var uri) || uri.Scheme != Uri.UriSchemeHttps)
                return false;

            var parts = uri.AbsolutePath.Split('/', StringSplitOptions.RemoveEmptyEntries);

            if (s_soundCloudApiHosts.Contains(uri.Host))
                return parts.Length == 2 && parts[0] == "tracks" && parts[1].All(char.IsAsciiDigit);

            if (!s_soundCloudHosts.Contains(uri.Host))
                return false;

            // /artista/faixa, ou /artista/faixa/s-TOKEN de uma faixa privada
            // compartilhada por link.
            return (parts.Length == 2 || (parts.Length == 3 && parts[2].StartsWith("s-", StringComparison.Ordinal))) &&
                   !s_soundCloudNonTrack.Contains(parts[1]);
        }

        /// <summary>
        /// Entrada de set antes da consulta: o nome sai do endereco
        /// ("sleepy-fish-cable-knit-sweater-1" vira "Sleepy fish cable knit
        /// sweater 1") ate a vez dela chegar.
        /// </summary>
        private static TrackInfo Placeholder(string url)
        {
            var uri = new Uri(url);
            var slug = uri.AbsolutePath.Split('/', StringSplitOptions.RemoveEmptyEntries).ElementAtOrDefault(1) ?? url;

            // Pela API nao ha nome no endereco, so o numero.
            var title = s_soundCloudApiHosts.Contains(uri.Host) ? "SoundCloud track" : slug.Replace('-', ' ').Replace('_', ' ');
            if (title.Length > 0)
                title = char.ToUpperInvariant(title[0]) + title[1..];

            return new TrackInfo(slug, title, url, TimeSpan.Zero, null, null, TrackSource.SoundCloud);
        }

        // ------------------------------------------------------------------
        // Na vez da faixa
        // ------------------------------------------------------------------

        /// <summary>
        /// A faixa do YouTube ou do SoundCloud que vai tocar no lugar desta
        /// entrada. Consulta a entrada de set, e casa a faixa do Spotify.
        /// </summary>
        public static async Task<TrackInfo> ResolvePlayableAsync(QueuedTrack entry, MusicSettings settings,
            CancellationToken ct)
        {
            if (entry.Playable is { } ready)
                return ready;

            var track = entry.Track;
            return track.Source == TrackSource.Spotify
                ? await MatchSpotifyAsync(track, settings, ct)
                : await YtDlp.ResolveAsync(track.WebpageUrl, track.Source, settings, ct);
        }

        /// <summary>
        /// Procura a faixa do Spotify primeiro no SoundCloud (toca por streaming)
        /// e, se nada bater, no YouTube Music (baixa antes, como todo YouTube).
        ///
        /// "Bater" no SoundCloud e exigente de proposito - la ha cover, remix e
        /// backing track de tudo. Tem de valer tudo junto: a duracao coincide (3s
        /// ou 3%), o titulo contem o nome da musica, o artista e quem subiu (ou
        /// esta no titulo, num reupload "Artista - Musica"), e nada de palavra de
        /// versao (s_variantWords) que o Spotify nao tenha. No YouTube Music a
        /// secao de musicas ja traz a gravacao oficial primeiro; ali a duracao so
        /// desempata entre os tres primeiros.
        /// </summary>
        private static async Task<TrackInfo> MatchSpotifyAsync(TrackInfo spotify, MusicSettings settings,
            CancellationToken ct)
        {
            var artist = (spotify.Uploader ?? string.Empty).Split(',')[0].Trim();
            var title = BaseTitle(spotify.Title);
            var query = $"{artist} {title}".Trim();

            var tolerance = TimeSpan.FromSeconds(Math.Max(3, spotify.Duration.TotalSeconds * 0.03));
            var wantedTitle = Normalize(title);
            var wantedArtist = Normalize(artist);
            var spotifyTitle = Normalize(spotify.Title);

            bool Accept(TrackInfo c)
            {
                if ((c.Duration - spotify.Duration).Duration() > tolerance || wantedTitle.Length == 0)
                    return false;

                var candTitle = Normalize(c.Title);
                var candUploader = Normalize(c.Uploader ?? string.Empty);
                var candAll = $"{candTitle} {candUploader}";

                if (s_variantWords.Any(w => HasWord(candAll, w) && !HasWord(spotifyTitle, w)))
                    return false;

                if (!candTitle.Contains(wantedTitle, StringComparison.Ordinal))
                    return false;

                return wantedArtist.Length > 0 &&
                       (candUploader == wantedArtist || candUploader.StartsWith(wantedArtist + " ", StringComparison.Ordinal) ||
                        candTitle.Contains(wantedArtist, StringComparison.Ordinal));
            }

            try
            {
                // Tres: cada resultado e uma consulta inteira, e a gravacao oficial,
                // quando esta no SoundCloud, vem no topo.
                var candidates = await YtDlp.SearchSoundCloudAsync(query, 3, settings, ct);
                var match = candidates.FirstOrDefault(Accept);

                if (match is not null)
                {
                    Console.WriteLine($"[musica] spotify {spotify.Id} -> soundcloud {match.Id}");
                    return match;
                }
            }
            catch (Exception ex) when (ex is not OperationCanceledException)
            {
                // SoundCloud fora do ar nao impede o YouTube de tentar.
                Console.WriteLine($"[musica] aviso: busca no SoundCloud falhou para spotify {spotify.Id}: {ex.Message}");
            }

            var ids = await YtDlp.SearchYouTubeMusicAsync(query, 3, settings, ct);
            TrackInfo? first = null;

            foreach (var id in ids)
            {
                TrackInfo video;
                try
                {
                    video = await YtDlp.ResolveAsync($"https://www.youtube.com/watch?v={id}", TrackSource.YouTube, settings, ct);
                }
                catch (MusicException)
                {
                    continue;
                }

                first ??= video;
                if ((video.Duration - spotify.Duration).Duration() <= TimeSpan.FromSeconds(15))
                {
                    Console.WriteLine($"[musica] spotify {spotify.Id} -> youtube {video.Id}");
                    return video;
                }
            }

            if (first is not null)
            {
                Console.WriteLine($"[musica] spotify {spotify.Id} -> youtube {first.Id} (sem duracao igual; o primeiro resultado)");
                return first;
            }

            throw new MusicException("I couldn't find this song on SoundCloud or YouTube.");
        }

        /// <summary>"Money - 2011 Remaster" e "Song (feat. X)" viram so o nome da musica.</summary>
        private static string BaseTitle(string title)
        {
            var cut = title.IndexOf(" - ", StringComparison.Ordinal);
            if (cut > 0)
                title = title[..cut];

            var paren = title.IndexOf(" (", StringComparison.Ordinal);
            if (paren > 0)
                title = title[..paren];

            return title.Trim();
        }

        /// <summary>
        /// Palavra (ou expressao) inteira, no singular ou no plural: "live" nao
        /// casa com "olive", nem "cover" com "discover" - mas "backing track"
        /// casa com o canal "Pink Floyd Backing Tracks".
        /// </summary>
        private static bool HasWord(string normalized, string word)
        {
            var padded = $" {normalized} ";
            return padded.Contains($" {word} ", StringComparison.Ordinal) ||
                   padded.Contains($" {word}s ", StringComparison.Ordinal);
        }

        /// <summary>Minusculo, sem acento e so letra, digito e espaco simples.</summary>
        private static string Normalize(string text)
        {
            var sb = new StringBuilder(text.Length);
            foreach (var c in text.Normalize(NormalizationForm.FormD))
            {
                if (CharUnicodeInfo.GetUnicodeCategory(c) == UnicodeCategory.NonSpacingMark)
                    continue;

                sb.Append(char.IsLetterOrDigit(c) ? char.ToLowerInvariant(c) : ' ');
            }

            return string.Join(' ', sb.ToString().Split(' ', StringSplitOptions.RemoveEmptyEntries));
        }
    }
}
