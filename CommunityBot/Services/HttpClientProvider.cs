using System;
using System.IO;
using System.Linq;
using System.Net;
using System.Net.Http;
using System.Net.Sockets;
using System.Threading;
using System.Threading.Tasks;

namespace CommunityBot.Services
{
    /// <summary>
    /// HttpClient compartilhado do bot. Veio do ccore, onde a mesma licao ja foi
    /// paga: um `new HttpClient()` por chamada acumula sockets em TIME_WAIT e,
    /// sob uso continuo, esgota portas locais. Um cliente estatico com
    /// SocketsHttpHandler resolve isso e ainda recicla as conexoes para nao
    /// ficar preso a um DNS antigo.
    ///
    /// Ate a verificacao Roblox este bot so falava com o Discord, e pela
    /// biblioteca. Quem usa isto hoje: a API publica do Roblox e o Worker de
    /// verificacao em daeese.me.
    ///
    /// IMPORTANTE: nunca altere Shared.Timeout depois do primeiro request -
    /// isso lanca InvalidOperationException. Prazo por requisicao e com
    /// CancellationTokenSource, que so encurta, nunca estende.
    /// </summary>
    internal static class HttpClientProvider
    {
        /// <summary>
        /// Quanto cada endereco IP tem para completar o handshake TCP antes de
        /// passar para o proximo. Curto de proposito: e so o connect.
        /// </summary>
        private static readonly TimeSpan s_connectAttemptTimeout = TimeSpan.FromSeconds(5);

        private static readonly SocketsHttpHandler s_handler = new()
        {
            PooledConnectionLifetime = TimeSpan.FromMinutes(5),
            PooledConnectionIdleTimeout = TimeSpan.FromMinutes(2),
            MaxConnectionsPerServer = 20,
            AutomaticDecompression = DecompressionMethods.All,
            ConnectCallback = ConnectAsync
        };

        public static readonly HttpClient Shared = CreateShared();

        private static HttpClient CreateShared()
        {
            var client = new HttpClient(s_handler, disposeHandler: false)
            {
                Timeout = TimeSpan.FromSeconds(20)
            };

            // A API do Roblox atende sem User-Agent, mas um cliente identificado
            // e o que da para achar num log do outro lado se algo der errado.
            client.DefaultRequestHeaders.UserAgent.ParseAdd("Sollarety/1.0 (+https://ccore.daeese.me/fun/)");
            return client;
        }

        /// <summary>
        /// Conecta tentando IPv4 antes de IPv6, com prazo curto por endereco.
        ///
        /// A rede das maquinas do bot anuncia IPv6 mas nao o roteia: o
        /// handshake simplesmente trava. O connect padrao do .NET percorre os
        /// enderecos em ordem, sem Happy Eyeballs, e ficava preso no IPv6 ate o
        /// Timeout matar a requisicao. A unit ja poe DOTNET_SYSTEM_NET_DISABLEIPV6,
        /// mas um `dotnet run` fora dela nao tem isso.
        /// </summary>
        private static async ValueTask<Stream> ConnectAsync(SocketsHttpConnectionContext context, CancellationToken ct)
        {
            var host = context.DnsEndPoint.Host;
            var port = context.DnsEndPoint.Port;

            IPAddress[] resolved = IPAddress.TryParse(host, out var literal)
                ? new[] { literal }
                : await Dns.GetHostAddressesAsync(host, ct).ConfigureAwait(false);

            // Defesa SSRF: tudo o que passa por aqui aponta para servico publico
            // (Roblox, Worker). A URL do Worker vem do config, e recusar
            // loopback/privado/link-local impede que uma entrada adulterada leve
            // o bot - e o segredo que ele manda no header - para a rede interna.
            var ordered = resolved
                .Where(a => !IsBlockedAddress(a))
                .OrderBy(a => a.AddressFamily == AddressFamily.InterNetworkV6 ? 1 : 0)
                .ToArray();

            if (ordered.Length == 0)
                throw new SocketException((int)SocketError.AccessDenied);

            Exception? lastError = null;

            foreach (var address in ordered)
            {
                ct.ThrowIfCancellationRequested();

                var socket = new Socket(address.AddressFamily, SocketType.Stream, ProtocolType.Tcp) { NoDelay = true };
                using var attemptCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
                attemptCts.CancelAfter(s_connectAttemptTimeout);

                try
                {
                    await socket.ConnectAsync(new IPEndPoint(address, port), attemptCts.Token).ConfigureAwait(false);
                    return new NetworkStream(socket, ownsSocket: true);
                }
                catch (Exception ex)
                {
                    socket.Dispose();

                    // Cancelamento de quem chamou nao e falha de endereco: nao
                    // adianta tentar o proximo.
                    ct.ThrowIfCancellationRequested();

                    lastError = ex is OperationCanceledException
                        ? new SocketException((int)SocketError.TimedOut)
                        : ex;
                }
            }

            throw lastError ?? new SocketException((int)SocketError.HostUnreachable);
        }

        /// <summary>
        /// Loopback, "qualquer", privados (RFC 1918), CGNAT, link-local (inclui o
        /// 169.254.169.254 de metadata) e o equivalente IPv6.
        /// </summary>
        private static bool IsBlockedAddress(IPAddress address)
        {
            if (IPAddress.IsLoopback(address))
                return true;

            if (address.AddressFamily == AddressFamily.InterNetworkV6)
            {
                if (address.IsIPv6LinkLocal || address.IsIPv6SiteLocal || address.IsIPv6UniqueLocal)
                    return true;

                if (address.Equals(IPAddress.IPv6Any) || address.Equals(IPAddress.IPv6None))
                    return true;

                // IPv4 embrulhado em IPv6 (::ffff:a.b.c.d) segue as regras de IPv4.
                return address.IsIPv4MappedToIPv6 && IsBlockedAddress(address.MapToIPv4());
            }

            if (address.Equals(IPAddress.Any) || address.Equals(IPAddress.None) || address.Equals(IPAddress.Broadcast))
                return true;

            var o = address.GetAddressBytes();
            if (o.Length != 4)
                return false;

            return o[0] == 10
                || o[0] == 127
                || (o[0] == 172 && o[1] >= 16 && o[1] <= 31)
                || (o[0] == 192 && o[1] == 168)
                || (o[0] == 169 && o[1] == 254)
                || (o[0] == 100 && o[1] >= 64 && o[1] <= 127);
        }
    }
}
