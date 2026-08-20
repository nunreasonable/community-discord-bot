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
    /// HttpClient compartilhado do bot.
    ///
    /// Antes cada comando fazia `using var http = new HttpClient()`, o que acumula
    /// sockets em TIME_WAIT e, sob uso continuo, esgota portas locais. Um unico
    /// cliente estatico com SocketsHttpHandler resolve isso e ainda recicla as
    /// conexoes periodicamente para nao ficar preso a um DNS antigo.
    ///
    /// IMPORTANTE: nunca altere Shared.Timeout depois do primeiro request -
    /// isso lanca InvalidOperationException. Use um CancellationTokenSource
    /// para prazos por requisicao (lembrando que um CTS so encurta o prazo,
    /// nunca estende alem do Timeout do cliente - para downloads longos use
    /// LongRunning).
    /// </summary>
    internal static class HttpClientProvider
    {
        /// <summary>
        /// Quanto cada endereco IP tem para completar o handshake TCP antes de
        /// passarmos para o proximo. Curto de proposito: e so o connect, nao a
        /// resposta inteira.
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

        public static readonly HttpClient Shared = new(s_handler, disposeHandler: false)
        {
            Timeout = TimeSpan.FromSeconds(20)
        };

        /// <summary>
        /// Mesmo pool de conexoes do Shared, porem sem teto proprio de tempo:
        /// quem usa este cliente E OBRIGADO a passar um CancellationToken, senao
        /// a requisicao pode ficar pendurada para sempre. Existe para downloads
        /// volumosos (ex.: o CSV inteiro da planilha) que nao cabem nos 20s.
        /// </summary>
        public static readonly HttpClient LongRunning = new(s_handler, disposeHandler: false)
        {
            Timeout = Timeout.InfiniteTimeSpan
        };

        /// <summary>
        /// Conecta tentando IPv4 antes de IPv6, com prazo curto por endereco.
        ///
        /// A rede onde o bot roda anuncia AAAA para hosts do Google
        /// (docs.google.com) mas nao roteia IPv6: o handshake simplesmente
        /// trava. O connect padrao do .NET percorre os enderecos resolvidos em
        /// ordem, sem Happy Eyeballs, entao ele ficava preso no IPv6 ate o
        /// Timeout do HttpClient matar a requisicao - era isso que quebrava o
        /// /audit-import. Curl nao sofria porque cai para IPv4 em ~200ms.
        /// </summary>
        private static async ValueTask<Stream> ConnectAsync(SocketsHttpConnectionContext context, CancellationToken ct)
        {
            var host = context.DnsEndPoint.Host;
            var port = context.DnsEndPoint.Port;

            IPAddress[] resolved = IPAddress.TryParse(host, out var literal)
                ? new[] { literal }
                : await Dns.GetHostAddressesAsync(host, ct).ConfigureAwait(false);

            // IPv4 primeiro; o IPv6 continua como fallback para redes que so tem v6.
            var ordered = resolved
                .OrderBy(a => a.AddressFamily == AddressFamily.InterNetworkV6 ? 1 : 0)
                .ToArray();

            if (ordered.Length == 0)
                throw new SocketException((int)SocketError.HostNotFound);

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

                    // Cancelamento do chamador (prazo do comando) nao e falha de
                    // endereco: nao adianta tentar o proximo.
                    ct.ThrowIfCancellationRequested();

                    lastError = ex is OperationCanceledException
                        ? new SocketException((int)SocketError.TimedOut)
                        : ex;
                }
            }

            throw lastError ?? new SocketException((int)SocketError.HostUnreachable);
        }
    }
}
