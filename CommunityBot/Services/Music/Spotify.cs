using System;
using System.Collections.Generic;
using System.Linq;
using System.Net;
using System.Net.Http;
using System.Text.RegularExpressions;
using System.Threading;
using System.Threading.Tasks;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;

namespace CommunityBot.Services.Music
{
    /// <summary>
    /// O que um link do Spotify diz sobre a musica - nome, artistas, duracao -,
    /// lido da pagina PUBLICA de embed (open.spotify.com/embed/...).
    ///
    /// Sem Web API de proposito: ela exige registrar uma aplicacao e guardar um
    /// client secret, e para faixa, album e playlist publica o embed ja entrega
    /// tudo o que o casamento precisa. O preco e depender do formato da pagina:
    /// se o Spotify mudar o __NEXT_DATA__, e aqui que quebra, com uma linha
    /// clara no log.
    ///
    /// Audio o Spotify nao entrega a ninguem (DRM). Quem toca e o SoundCloud ou
    /// o YouTube - ver MusicSources.MatchSpotifyAsync.
    /// </summary>
    internal static class Spotify
    {
        internal sealed record Item(string Id, string Title, string Artists, TimeSpan Duration, string? ImageUrl);

        internal sealed record Collection(string Kind, string? Name, List<Item> Tracks);

        private static readonly TimeSpan s_timeout = TimeSpan.FromSeconds(10);

        private static readonly Regex s_nextData = new(
            "<script id=\"__NEXT_DATA__\" type=\"application/json\">(.*?)</script>",
            RegexOptions.Singleline | RegexOptions.Compiled);

        // Os ids do Spotify sao base62 de 22 caracteres. Validar aqui e o que
        // garante que a URL do embed montada abaixo nao carrega nada alheio.
        private static readonly Regex s_id = new("^[A-Za-z0-9]{22}$", RegexOptions.Compiled);

        /// <summary>
        /// open.spotify.com/track/ID, /intl-pt/album/ID, /embed/playlist/ID ou
        /// spotify:track:ID. Artista, podcast e episodio ficam de fora.
        /// </summary>
        public static bool TryParse(string input, out string kind, out string id)
        {
            kind = id = string.Empty;
            string[] parts;

            if (input.StartsWith("spotify:", StringComparison.OrdinalIgnoreCase))
                parts = input.Split(':', StringSplitOptions.RemoveEmptyEntries).Skip(1).ToArray();
            else if (Uri.TryCreate(input, UriKind.Absolute, out var uri))
                parts = uri.AbsolutePath.Split('/', StringSplitOptions.RemoveEmptyEntries)
                    .Where(p => !p.StartsWith("intl-", StringComparison.OrdinalIgnoreCase) && p != "embed")
                    .ToArray();
            else
                return false;

            if (parts.Length < 2 || parts[0] is not ("track" or "album" or "playlist") || !s_id.IsMatch(parts[1]))
                return false;

            kind = parts[0];
            id = parts[1];
            return true;
        }

        public static async Task<Collection> FetchAsync(string kind, string id, CancellationToken ct)
        {
            using var cts = CancellationTokenSource.CreateLinkedTokenSource(ct);
            cts.CancelAfter(s_timeout);

            string html;
            try
            {
                using var response = await HttpClientProvider.Shared.GetAsync(
                    $"https://open.spotify.com/embed/{kind}/{id}", cts.Token);

                if (response.StatusCode is HttpStatusCode.NotFound or HttpStatusCode.BadRequest)
                    throw new MusicException("That Spotify link doesn't exist or isn't public.");

                if (!response.IsSuccessStatusCode)
                {
                    Console.WriteLine($"[musica] aviso: embed do Spotify respondeu {(int)response.StatusCode} para {kind}/{id}");
                    throw new MusicException("Spotify didn't answer. Try again in a moment.");
                }

                html = await response.Content.ReadAsStringAsync(cts.Token);
            }
            catch (Exception ex) when (ex is HttpRequestException or OperationCanceledException && !ct.IsCancellationRequested)
            {
                Console.WriteLine($"[musica] aviso: embed do Spotify falhou para {kind}/{id}: {ex.Message}");
                throw new MusicException("Spotify didn't answer. Try again in a moment.");
            }

            JToken entity;
            try
            {
                var match = s_nextData.Match(html);
                entity = match.Success
                    ? JObject.Parse(match.Groups[1].Value).SelectToken("props.pageProps.state.data.entity")
                      ?? throw new JsonException("sem props.pageProps.state.data.entity")
                    : throw new JsonException("sem __NEXT_DATA__");
            }
            catch (JsonException ex)
            {
                // A pagina mudou de formato. E o ponto fragil documentado acima.
                Console.WriteLine($"[musica] falha: o embed do Spotify mudou de formato ({kind}/{id}): {ex.Message}");
                throw new MusicException("I couldn't read that Spotify link. Try a YouTube or SoundCloud link instead.");
            }

            var image = LargestImage(entity);

            if (kind == "track")
            {
                var artists = string.Join(", ", (entity["artists"] as JArray ?? new JArray())
                    .Select(a => (string?)a["name"]).Where(n => !string.IsNullOrWhiteSpace(n)));

                var track = new Item(
                    id,
                    (string?)entity["name"] ?? (string?)entity["title"] ?? "Unknown track",
                    artists,
                    TimeSpan.FromMilliseconds((double?)entity["duration"] ?? 0),
                    image);

                return new Collection(kind, null, new List<Item> { track });
            }

            var tracks = (entity["trackList"] as JArray ?? new JArray())
                .Where(t => (bool?)t["isPlayable"] != false)
                .Select(t => new Item(
                    ((string?)t["uri"])?.Split(':').LastOrDefault() ?? string.Empty,
                    (string?)t["title"] ?? "Unknown track",
                    // "A, B": o embed separa os artistas com espaco rigido.
                    Regex.Replace((string?)t["subtitle"] ?? string.Empty, @"\s+", " ").Trim(),
                    TimeSpan.FromMilliseconds((double?)t["duration"] ?? 0),
                    image))
                .Where(t => s_id.IsMatch(t.Id))
                .ToList();

            return new Collection(kind, (string?)entity["name"] ?? (string?)entity["title"], tracks);
        }

        /// <summary>A maior capa do embed; so https do CDN de imagens do Spotify.</summary>
        private static string? LargestImage(JToken entity)
        {
            var url = (entity.SelectToken("visualIdentity.image") as JArray ?? new JArray())
                .OrderByDescending(i => (int?)i["maxWidth"] ?? 0)
                .Select(i => (string?)i["url"])
                .FirstOrDefault(u => u is not null);

            return Uri.TryCreate(url, UriKind.Absolute, out var uri) && uri.Scheme == Uri.UriSchemeHttps &&
                   (uri.Host.EndsWith(".spotifycdn.com", StringComparison.OrdinalIgnoreCase) ||
                    uri.Host.EndsWith(".scdn.co", StringComparison.OrdinalIgnoreCase))
                ? uri.AbsoluteUri
                : null;
        }

        public static string TrackUrl(string id) => $"https://open.spotify.com/track/{id}";
    }
}
