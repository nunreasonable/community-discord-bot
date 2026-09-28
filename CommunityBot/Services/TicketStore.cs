using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using Newtonsoft.Json;

namespace CommunityBot.Services
{
    internal sealed class Ticket
    {
        /// <summary>Id curto, o que viaja no custom id dos botoes.</summary>
        public string id { get; set; } = string.Empty;

        /// <summary>Sequencial POR SERVIDOR. E o numero que aparece no nome do canal.</summary>
        public ulong number { get; set; }

        public ulong guildId { get; set; }
        public ulong channelId { get; set; }
        public ulong openerId { get; set; }

        /// <summary>Chave do tipo, como em <see cref="TicketTypes"/>.</summary>
        public string type { get; set; } = string.Empty;

        public string subject { get; set; } = string.Empty;

        /// <summary>Quem assumiu o atendimento, se alguem assumiu.</summary>
        public ulong? claimedById { get; set; }

        public DateTimeOffset openedAtUtc { get; set; }
        public DateTimeOffset? closedAtUtc { get; set; }
        public ulong? closedById { get; set; }

        /// <summary>
        /// Quando alguem comecou a fechar este ticket, se alguem comecou.
        ///
        /// Existe porque fechar tem DUAS fases - arquivar e apagar - e so a
        /// segunda e irreversivel. Marcar `closedAtUtc` logo na entrada, como era
        /// antes, tinha um efeito que so aparece quando o arquivamento falha: o
        /// canal fica de pe (correto) mas o registro ja esta fechado, e TODO
        /// caminho de volta - /ticket-close, o botao Close, /ticket-add -
        /// recusa ticket fechado. O canal virava invisivel para o bot, sem como
        /// tentar de novo e sem como apagar a nao ser a mao.
        ///
        /// Agora esta marca e a reserva: ela barra o fechamento duplo sem
        /// declarar fechado o que ainda nao foi arquivado.
        /// </summary>
        public DateTimeOffset? closingSince { get; set; }

        public bool IsOpen => closedAtUtc is null;

        /// <summary>
        /// Ha um fechamento em andamento e recente. A janela evita que uma
        /// tentativa que morreu no meio (processo reiniciado, exceção solta)
        /// tranque o ticket para sempre.
        /// </summary>
        public bool IsClosing(DateTimeOffset now) =>
            closingSince is { } since && now - since < TimeSpan.FromMinutes(2);
    }

    internal sealed class TicketFile
    {
        public List<Ticket> tickets { get; set; } = new();

        /// <summary>
        /// Ultimo numero usado em cada servidor, com a chave em texto porque e
        /// assim que ela sobrevive ao JSON.
        ///
        /// Guardado, e nao calculado com max(number)+1 sobre a lista: a poda
        /// apaga tickets fechados antigos, e um contador derivado da lista
        /// voltaria a repetir numeros ja usados.
        /// </summary>
        public Dictionary<string, ulong> lastNumberByGuild { get; set; } = new();

        public DateTimeOffset? lastUpdatedUtc { get; set; }
    }

    /// <summary>
    /// Visao mutavel do arquivo entregue ao <see cref="TicketStore.UpdateAsync"/>.
    /// Quem muta precisa dizer que mutou - e o unico jeito de o store saber se
    /// vale gravar sem reserializar tudo para comparar.
    /// </summary>
    internal sealed class TicketEdit
    {
        public TicketEdit(TicketFile file) => File = file;

        public TicketFile File { get; }

        public bool Changed { get; private set; }

        public void MarkChanged() => Changed = true;
    }

    /// <summary>
    /// Tickets em disco, no mesmo desenho do <see cref="WarningStore"/>: semaforo
    /// ESTATICO (cada comando cria o seu store, um semaforo de instancia nao
    /// protegeria nada) e escrita atomica com .tmp sincronizado antes do rename.
    ///
    /// Ter isto em disco, e nao num dicionario em memoria, e o que faz um painel
    /// publicado continuar funcionando depois de o bot reiniciar - a mesma razao
    /// que o /poll registra para deixar os votos nas reacoes.
    /// </summary>
    internal sealed class TicketStore
    {
        public static TicketStore Instance { get; } = new();

        private static readonly SemaphoreSlim s_lock = new(1, 1);

        private readonly string _path = AppPaths.Data("tickets.json");

        private TicketStore()
        {
            try
            {
                var orphan = _path + ".tmp";
                if (File.Exists(orphan))
                    File.Delete(orphan);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[tickets] nao foi possivel limpar o .tmp orfao: {ex.Message}");
            }
        }

        public async Task<TicketFile> ReadAsync()
        {
            await s_lock.WaitAsync().ConfigureAwait(false);
            try
            {
                return await ReadJsonAsync().ConfigureAwait(false);
            }
            finally
            {
                s_lock.Release();
            }
        }

        /// <summary>O ticket de um canal, aberto ou nao.</summary>
        public async Task<Ticket?> ByChannelAsync(ulong channelId)
        {
            var file = await ReadAsync().ConfigureAwait(false);
            return file.tickets.FirstOrDefault(t => t.channelId == channelId);
        }

        /// <summary>O ticket de um id curto, dentro do servidor.</summary>
        public async Task<Ticket?> ByIdAsync(ulong guildId, string id)
        {
            var file = await ReadAsync().ConfigureAwait(false);
            return file.tickets.FirstOrDefault(t =>
                t.guildId == guildId && string.Equals(t.id, id, StringComparison.OrdinalIgnoreCase));
        }

        /// <summary>
        /// Unico caminho de escrita. O mutador e SINCRONO de proposito: nao da
        /// para chamar a API do Discord com o arquivo travado, entao todo o
        /// trabalho de rede acontece fora deste bloco.
        /// </summary>
        public async Task<T> UpdateAsync<T>(Func<TicketEdit, T> mutate)
        {
            await s_lock.WaitAsync().ConfigureAwait(false);
            try
            {
                var file = await ReadJsonAsync().ConfigureAwait(false);
                var edit = new TicketEdit(file);
                var result = mutate(edit);

                if (edit.Changed)
                {
                    Prune(file);
                    file.lastUpdatedUtc = DateTimeOffset.UtcNow;
                    await WriteJsonAsync(file).ConfigureAwait(false);
                }

                return result;
            }
            finally
            {
                s_lock.Release();
            }
        }

        /// <summary>
        /// Proximo numero do servidor. Chamado de DENTRO do mutador, com o
        /// arquivo em maos, para dois cliques simultaneos nao tirarem o mesmo.
        /// </summary>
        public static ulong NextNumber(TicketFile file, ulong guildId)
        {
            var key = guildId.ToString();
            var next = (file.lastNumberByGuild.TryGetValue(key, out var last) ? last : 0) + 1;
            file.lastNumberByGuild[key] = next;
            return next;
        }

        /// <summary>Id curto garantidamente unico no arquivo, conferido sem corrida.</summary>
        public static string NewUniqueId(TicketFile file)
        {
            string id;
            do
            {
                id = Guid.NewGuid().ToString("N")[..8];
            }
            while (file.tickets.Any(t => string.Equals(t.id, id, StringComparison.OrdinalIgnoreCase)));

            return id;
        }

        /// <summary>
        /// Teto de tickets FECHADOS guardados por servidor.
        /// </summary>
        private const int MaxClosedPerGuild = 2000;

        /// <summary>
        /// Poda so o que ja fechou.
        ///
        /// Esta e a diferenca que importa em relacao ao WarningStore: um ticket
        /// ABERTO nao pode ser podado em hipotese alguma. Os botoes daquele canal
        /// resolvem pelo id no arquivo, entao apagar o registro de um ticket vivo
        /// deixaria um canal orfao que ninguem mais consegue fechar pelo bot -
        /// exatamente o estado que o sistema existe para nao produzir.
        ///
        /// Por servidor, e nao global, pelo mesmo motivo ja registrado no
        /// WarningStore: um corte global deixaria um servidor movimentado apagar
        /// o historico de outro.
        /// </summary>
        private static void Prune(TicketFile file)
        {
            // O portao olha o MAIOR servidor, e nao o total: somando tudo, tres
            // servidores com 900 fechados cada disparavam a reconstrucao inteira
            // a cada escrita sem ter nada para podar.
            var worst = file.tickets
                .Where(t => !t.IsOpen)
                .GroupBy(t => t.guildId)
                .Select(g => g.Count())
                .DefaultIfEmpty(0)
                .Max();

            if (worst <= MaxClosedPerGuild)
                return;

            var keep = file.tickets
                .Where(t => t.IsOpen)
                .Concat(file.tickets
                    .Where(t => !t.IsOpen)
                    .GroupBy(t => t.guildId)
                    .SelectMany(g => g.OrderByDescending(t => t.closedAtUtc).Take(MaxClosedPerGuild)))
                .OrderBy(t => t.openedAtUtc)
                .ToList();

            var dropped = file.tickets.Count - keep.Count;
            file.tickets = keep;

            if (dropped > 0)
                Console.WriteLine($"[tickets] podados {dropped} ticket(s) fechado(s); teto e {MaxClosedPerGuild} por servidor.");
        }

        private async Task<TicketFile> ReadJsonAsync()
        {
            try
            {
                if (!File.Exists(_path))
                    return new TicketFile();

                var json = await File.ReadAllTextAsync(_path).ConfigureAwait(false);
                if (string.IsNullOrWhiteSpace(json))
                    return new TicketFile();

                return JsonConvert.DeserializeObject<TicketFile>(json) ?? new TicketFile();
            }
            catch (Exception ex)
            {
                // Arquivo corrompido nao pode virar "nenhum ticket": o proximo
                // write consolidaria isso como verdade e todo ticket aberto
                // perderia o dono. Melhor gritar.
                Console.WriteLine($"[tickets] falha ao ler {_path}: {ex.Message}");
                throw;
            }
        }

        private async Task WriteJsonAsync(TicketFile file)
        {
            var json = JsonConvert.SerializeObject(file, Formatting.Indented);
            var temp = _path + ".tmp";

            // O .tmp e sincronizado no disco ANTES do rename: sem isso o rename
            // pode chegar ao disco antes do conteudo, e o que sobra e um
            // tickets.json truncado - que o ReadJsonAsync trata como "nenhum
            // ticket".
            await using (var stream = new FileStream(temp, FileMode.Create, FileAccess.Write, FileShare.None))
            {
                await using var writer = new StreamWriter(stream);
                await writer.WriteAsync(json).ConfigureAwait(false);
                await writer.FlushAsync().ConfigureAwait(false);
                stream.Flush(flushToDisk: true);
            }

            File.Move(temp, _path, overwrite: true);
        }
    }
}
