using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Net;
using System.Net.Http;
using System.Threading;
using System.Threading.Tasks;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;

namespace CommunityBot.Services.Roblox
{
    internal sealed record RobloxUser(long Id, string Name, string DisplayName, DateTimeOffset? Created);

    /// <summary>Participacao num grupo: o rank vai de 1 a 255; fora do grupo e 0.</summary>
    internal sealed record RobloxGroupRole(long GroupId, string GroupName, int Rank, string RoleName);

    internal sealed record RobloxGroup(long Id, string Name);

    /// <summary>
    /// Resultado de uma consulta que separa "nao existe" de "nao deu para saber".
    ///
    /// O ccore misturava os dois: um 429 ou 5xx na busca de usuario virava
    /// "conta nao encontrada". Aqui isso teria efeito pior - uma instabilidade
    /// do Roblox faria o bot TIRAR cargos de bind de todo mundo que entrasse.
    /// Com os tres estados separados, falha significa "nao mexa em nada".
    /// </summary>
    internal readonly record struct Lookup<T>(T? Value, bool NotFound, string? Error) where T : class
    {
        public bool Ok => Value is not null;

        public static Lookup<T> Found(T value) => new(value, false, null);
        public static Lookup<T> Missing() => new(null, true, null);
        public static Lookup<T> Fail(string error) => new(null, false, error);
    }

    /// <summary>
    /// A API publica do Roblox, sem autenticacao nenhuma.
    ///
    /// A posse da conta NAO e provada aqui - isso e o OAuth, no Worker. Daqui sai
    /// so o que qualquer um ve no perfil: nome, nome de exibicao, data de criacao
    /// e em que grupos a pessoa esta com qual rank.
    /// </summary>
    internal static class RobloxApi
    {
        /// <summary>
        /// Uma leva de entradas no servidor, ou um /update logo depois de um
        /// /verify, nao precisa perguntar a mesma coisa duas vezes. Curto o
        /// bastante para uma promocao no grupo aparecer no /update seguinte.
        /// </summary>
        private static readonly TimeSpan CacheTtl = TimeSpan.FromMinutes(5);

        /// <summary>Teto de espera num 429. Acima disto e melhor falhar e avisar.</summary>
        private static readonly TimeSpan MaxRetryAfter = TimeSpan.FromSeconds(5);

        private const int MaxCacheEntries = 5000;

        private static readonly ConcurrentDictionary<long, (DateTime Expires, RobloxUser Value)> s_users = new();
        private static readonly ConcurrentDictionary<long, (DateTime Expires, IReadOnlyList<RobloxGroupRole> Value)> s_groups = new();

        public static string ProfileUrl(long robloxId) => $"https://www.roblox.com/users/{robloxId}/profile";

        public static async Task<Lookup<RobloxUser>> GetUserAsync(long robloxId, bool fresh = false, CancellationToken ct = default)
        {
            if (!fresh && s_users.TryGetValue(robloxId, out var hit) && hit.Expires > DateTime.UtcNow)
                return Lookup<RobloxUser>.Found(hit.Value);

            var (_, json, error) = await GetJsonAsync($"https://users.roblox.com/v1/users/{robloxId}", ct).ConfigureAwait(false);
            if (error is not null)
                return Lookup<RobloxUser>.Fail(error);
            if (json is null)
                return Lookup<RobloxUser>.Missing();

            var user = new RobloxUser(
                robloxId,
                json.Value<string>("name") ?? string.Empty,
                json.Value<string>("displayName") ?? json.Value<string>("name") ?? string.Empty,
                ParseDate(json.Value<string>("created")));

            Remember(s_users, robloxId, user);
            return Lookup<RobloxUser>.Found(user);
        }

        public static async Task<Lookup<IReadOnlyList<RobloxGroupRole>>> GetGroupRolesAsync(long robloxId, bool fresh = false,
            CancellationToken ct = default)
        {
            if (!fresh && s_groups.TryGetValue(robloxId, out var hit) && hit.Expires > DateTime.UtcNow)
                return Lookup<IReadOnlyList<RobloxGroupRole>>.Found(hit.Value);

            // v2 e a versao enxuta: o v1 devolve dono, descricao e mural de cada
            // grupo, e quem esta em cinquenta grupos baixa tudo isso a toa.
            var (_, json, error) = await GetJsonAsync(
                $"https://groups.roblox.com/v2/users/{robloxId}/groups/roles", ct).ConfigureAwait(false);
            if (error is not null)
                return Lookup<IReadOnlyList<RobloxGroupRole>>.Fail(error);
            if (json is null)
                return Lookup<IReadOnlyList<RobloxGroupRole>>.Missing();

            var roles = (json["data"] as JArray ?? new JArray())
                .OfType<JObject>()
                .Select(entry => new RobloxGroupRole(
                    entry["group"]?.Value<long?>("id") ?? 0,
                    entry["group"]?.Value<string>("name") ?? string.Empty,
                    entry["role"]?.Value<int?>("rank") ?? 0,
                    entry["role"]?.Value<string>("name") ?? string.Empty))
                .Where(r => r.GroupId > 0)
                .ToList();

            Remember(s_groups, robloxId, roles);
            return Lookup<IReadOnlyList<RobloxGroupRole>>.Found(roles);
        }

        public static async Task<Lookup<RobloxGroup>> GetGroupAsync(long groupId, CancellationToken ct = default)
        {
            var (_, json, error) = await GetJsonAsync($"https://groups.roblox.com/v1/groups/{groupId}", ct).ConfigureAwait(false);
            if (error is not null)
                return Lookup<RobloxGroup>.Fail(error);
            if (json is null)
                return Lookup<RobloxGroup>.Missing();

            return Lookup<RobloxGroup>.Found(new RobloxGroup(groupId, json.Value<string>("name") ?? $"grupo {groupId}"));
        }

        /// <summary>Esquece o que se sabe de um usuario. Chamado no /update.</summary>
        public static void Forget(long robloxId)
        {
            s_users.TryRemove(robloxId, out _);
            s_groups.TryRemove(robloxId, out _);
        }

        /// <summary>
        /// GET com uma nova tentativa em 429, respeitando o Retry-After.
        ///
        /// Devolve json nulo e erro nulo para "nao existe": 404 no users e 400 no
        /// groups ("Group is invalid or does not exist") - os dois endpoints dizem
        /// a mesma coisa com codigos diferentes, e toda requisicao daqui e
        /// montada pelo bot, entao um 400 nao e pedido malformado.
        /// </summary>
        private static async Task<(HttpStatusCode Status, JObject? Json, string? Error)> GetJsonAsync(string url, CancellationToken ct)
        {
            for (var attempt = 0; ; attempt++)
            {
                try
                {
                    using var response = await HttpClientProvider.Shared.GetAsync(url, ct).ConfigureAwait(false);

                    if (response.StatusCode is HttpStatusCode.NotFound or HttpStatusCode.BadRequest)
                        return (response.StatusCode, null, null);

                    if (response.StatusCode == HttpStatusCode.TooManyRequests && attempt == 0)
                    {
                        var wait = response.Headers.RetryAfter?.Delta ?? TimeSpan.FromSeconds(1);
                        if (wait <= MaxRetryAfter)
                        {
                            await Task.Delay(wait, ct).ConfigureAwait(false);
                            continue;
                        }
                    }

                    if (!response.IsSuccessStatusCode)
                    {
                        Console.WriteLine($"[roblox] aviso: {url} respondeu {(int)response.StatusCode}");
                        return (response.StatusCode, null, $"o Roblox respondeu {(int)response.StatusCode}");
                    }

                    var body = await response.Content.ReadAsStringAsync(ct).ConfigureAwait(false);

                    // DateParseHandling.None: o JObject.Parse padrao transforma
                    // "created" num DateTime local e o Value<string> depois devolve
                    // a data formatada na cultura da maquina, sem fuso.
                    using var reader = new JsonTextReader(new StringReader(body)) { DateParseHandling = DateParseHandling.None };
                    return (response.StatusCode, JObject.Load(reader), null);
                }
                catch (OperationCanceledException) when (!ct.IsCancellationRequested)
                {
                    Console.WriteLine($"[roblox] aviso: {url} demorou demais");
                    return (0, null, "o Roblox demorou demais para responder");
                }
                catch (Exception ex) when (ex is HttpRequestException or JsonException)
                {
                    Console.WriteLine($"[roblox] aviso: {url} falhou: {ex.Message}");
                    return (0, null, "não consegui falar com o Roblox");
                }
            }
        }

        private static void Remember<T>(ConcurrentDictionary<long, (DateTime Expires, T Value)> cache, long key, T value)
        {
            // Poda grosseira: o cache so existe para absorver rajada, e passar do
            // teto e sinal de que ele esta guardando gente que nao volta.
            if (cache.Count > MaxCacheEntries)
            {
                var now = DateTime.UtcNow;
                foreach (var stale in cache.Where(p => p.Value.Expires <= now).Select(p => p.Key).ToList())
                    cache.TryRemove(stale, out _);

                if (cache.Count > MaxCacheEntries)
                    cache.Clear();
            }

            cache[key] = (DateTime.UtcNow + CacheTtl, value);
        }

        private static DateTimeOffset? ParseDate(string? value) =>
            DateTimeOffset.TryParse(value, CultureInfo.InvariantCulture, DateTimeStyles.RoundtripKind, out var parsed)
                ? parsed
                : null;
    }
}
