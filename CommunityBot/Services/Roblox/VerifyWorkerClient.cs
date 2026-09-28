using System;
using System.Globalization;
using System.IO;
using System.Net;
using System.Net.Http;
using System.Net.Http.Headers;
using System.Threading;
using System.Threading.Tasks;
using CommunityBot.config;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;

namespace CommunityBot.Services.Roblox
{
    /// <summary>O que o Worker devolve depois dos dois OAuth.</summary>
    internal sealed record VerifiedAccount(ulong DiscordId, long RobloxId, string Name, string DisplayName, DateTimeOffset? Created);

    internal enum TakeStatus { Pending, Found, Failed }

    internal readonly record struct TakeResult(TakeStatus Status, VerifiedAccount? Account, string? Error);

    /// <summary>
    /// Busca no Worker o resultado de uma verificacao.
    ///
    /// O bot nao tem porta aberta para a internet, entao quem termina o OAuth
    /// (o Worker) nao consegue avisar o bot. O caminho e o inverso: o Worker
    /// guarda o resultado por meia hora, indexado pelo id do Discord que o
    /// navegador PROVOU pelo OAuth do Discord, e o bot vem buscar. A leitura
    /// consome: o mesmo resultado nao e entregue duas vezes.
    /// </summary>
    internal static class VerifyWorkerClient
    {
        public static async Task<TakeResult> TakeAsync(RobloxVerifySettings settings, ulong discordId, CancellationToken ct = default)
        {
            if (!settings.IsConfigured)
                return new TakeResult(TakeStatus.Failed, null, "verification is not configured");

            var url = $"{settings.resultUrl}?d={discordId.ToString(CultureInfo.InvariantCulture)}";

            try
            {
                using var request = new HttpRequestMessage(HttpMethod.Get, url);
                request.Headers.Authorization = new AuthenticationHeaderValue("Bearer", settings.apiSecret);

                using var response = await HttpClientProvider.Shared.SendAsync(request, ct).ConfigureAwait(false);

                if (response.StatusCode == HttpStatusCode.NotFound)
                    return new TakeResult(TakeStatus.Pending, null, null);

                if (response.StatusCode == HttpStatusCode.Unauthorized)
                {
                    // Nao e falha passageira: o segredo daqui e o do Worker nao
                    // batem. Vai para o log como erro para aparecer no /logs.
                    Console.WriteLine("[roblox] erro: o Worker recusou o apiSecret (401). " +
                                      "Confira robloxVerify.apiSecret contra o BOT_API_SECRET do Worker.");
                    return new TakeResult(TakeStatus.Failed, null, "the verification server rejected the bot's credentials");
                }

                if (!response.IsSuccessStatusCode)
                    return new TakeResult(TakeStatus.Failed, null, $"the verification server responded with {(int)response.StatusCode}");

                var body = await response.Content.ReadAsStringAsync(ct).ConfigureAwait(false);
                using var reader = new JsonTextReader(new StringReader(body)) { DateParseHandling = DateParseHandling.None };
                var json = JObject.Load(reader);

                // Os ids vem como string: o id do Discord nao cabe num double, e e
                // isso que o JavaScript do Worker tem como numero.
                if (!ulong.TryParse(json.Value<string>("discordId"), NumberStyles.None, CultureInfo.InvariantCulture, out var returnedId) ||
                    !long.TryParse(json.Value<string>("robloxId"), NumberStyles.None, CultureInfo.InvariantCulture, out var robloxId) ||
                    robloxId <= 0)
                {
                    return new TakeResult(TakeStatus.Failed, null, "the verification server returned an invalid response");
                }

                // O Worker indexa pelo id pedido, entao isto so falharia com um
                // Worker quebrado. Custa uma comparacao e fecha a porta de vez.
                if (returnedId != discordId)
                    return new TakeResult(TakeStatus.Failed, null, "the verification result belonged to a different Discord account");

                DateTimeOffset? created = json.Value<long?>("createdAt") is { } unix and > 0
                    ? DateTimeOffset.FromUnixTimeSeconds(unix)
                    : null;

                return new TakeResult(TakeStatus.Found, new VerifiedAccount(
                    returnedId,
                    robloxId,
                    json.Value<string>("name") ?? string.Empty,
                    json.Value<string>("displayName") ?? json.Value<string>("name") ?? string.Empty,
                    created), null);
            }
            catch (OperationCanceledException) when (!ct.IsCancellationRequested)
            {
                return new TakeResult(TakeStatus.Failed, null, "the verification server took too long to respond");
            }
            catch (Exception ex) when (ex is HttpRequestException or JsonException)
            {
                Console.WriteLine($"[roblox] aviso: falha ao consultar o Worker: {ex.Message}");
                return new TakeResult(TakeStatus.Failed, null, "I couldn't reach the verification server");
            }
        }
    }
}
