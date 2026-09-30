using System;
using System.Net.Http;
using System.Threading;
using System.Threading.Tasks;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;

namespace CommunityBot.Services.Fun
{
    /// <summary>
    /// GIFs do /roleplay, pela API publica do nekos.best (v2, sem chave).
    ///
    /// A categoria vem sempre da tabela do RoleplayActions, nunca do usuario: e
    /// o que mantem a URL montada aqui fixa. Os termos do nekos.best proibem uso
    /// comercial e pedem um User-Agent identificado - o do
    /// HttpClientProvider.Shared ja e "Sollarety/1.0 (+site)".
    /// </summary>
    internal static class NekosBest
    {
        internal sealed record Gif(string Url, string? AnimeName);

        private static readonly TimeSpan s_timeout = TimeSpan.FromSeconds(8);

        /// <summary>Um GIF da categoria, ou null se a API falhar - o comando sai sem imagem, mas sai.</summary>
        public static async Task<Gif?> GetAsync(string category, CancellationToken ct = default)
        {
            using var cts = CancellationTokenSource.CreateLinkedTokenSource(ct);
            cts.CancelAfter(s_timeout);

            try
            {
                using var response = await HttpClientProvider.Shared
                    .GetAsync($"https://nekos.best/api/v2/{category}", cts.Token)
                    .ConfigureAwait(false);

                if (!response.IsSuccessStatusCode)
                {
                    Console.WriteLine($"[roleplay] aviso: nekos.best respondeu {(int)response.StatusCode} para '{category}'");
                    return null;
                }

                var body = await response.Content.ReadAsStringAsync(cts.Token).ConfigureAwait(false);
                var first = JObject.Parse(body)["results"]?.First;
                var url = (string?)first?["url"];

                // Uma resposta estranha nao vira imagem num cartao assinado pelo
                // bot: so https e so do proprio nekos.best.
                if (!Uri.TryCreate(url, UriKind.Absolute, out var uri) || uri.Scheme != Uri.UriSchemeHttps ||
                    !(uri.Host == "nekos.best" || uri.Host.EndsWith(".nekos.best", StringComparison.OrdinalIgnoreCase)))
                {
                    Console.WriteLine($"[roleplay] aviso: nekos.best devolveu uma URL inesperada para '{category}'");
                    return null;
                }

                var anime = (string?)first?["anime_name"];
                return new Gif(uri.AbsoluteUri, string.IsNullOrWhiteSpace(anime) ? null : anime);
            }
            catch (Exception ex) when (ex is HttpRequestException or OperationCanceledException or JsonException
                                           or InvalidCastException)
            {
                if (ct.IsCancellationRequested)
                    throw;

                Console.WriteLine($"[roleplay] aviso: nekos.best falhou para '{category}': {ex.Message}");
                return null;
            }
        }
    }
}
