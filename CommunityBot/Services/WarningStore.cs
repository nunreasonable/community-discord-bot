using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using Newtonsoft.Json;

namespace CommunityBot.Services
{
    internal sealed class Warning
    {
        /// <summary>Id curto, o que a pessoa digita no /delwarn.</summary>
        public string id { get; set; } = string.Empty;

        public ulong guildId { get; set; }
        public ulong userId { get; set; }
        public ulong moderatorId { get; set; }
        public string moderatorTag { get; set; } = string.Empty;
        public string reason { get; set; } = string.Empty;
        public DateTimeOffset createdAtUtc { get; set; }
    }

    internal sealed class WarningFile
    {
        public List<Warning> warnings { get; set; } = new();
        public DateTimeOffset? lastUpdatedUtc { get; set; }
    }

    /// <summary>
    /// Visao mutavel do arquivo entregue a <see cref="WarningStore.UpdateAsync"/>.
    ///
    /// Quem muta precisa dizer que mutou, chamando <see cref="MarkChanged"/> -
    /// e o unico jeito de o store saber se vale gravar sem reserializar tudo
    /// para comparar.
    /// </summary>
    internal sealed class WarningEdit
    {
        public WarningEdit(WarningFile file) => File = file;

        public WarningFile File { get; }

        public bool Changed { get; private set; }

        public void MarkChanged() => Changed = true;
    }

    /// <summary>
    /// Persistencia das advertencias (data/warnings.json).
    ///
    /// Segue o AuditStore do ccore em duas escolhas que nao sao obvias:
    ///
    /// 1. O semaforo e ESTATICO. Cada comando cria a sua instancia de leitor; um
    ///    semaforo de instancia nao protegeria nada entre dois /warn simultaneos.
    ///    Por isso os comandos usam sempre WarningStore.Instance.
    /// 2. A escrita e atomica (.tmp + File.Move). Uma queda no meio de um
    ///    WriteAllText trunca o arquivo, e aqui esta o historico inteiro de
    ///    moderacao do servidor.
    /// </summary>
    internal sealed class WarningStore
    {
        public static WarningStore Instance { get; } = new();

        private static readonly SemaphoreSlim s_lock = new(1, 1);

        private readonly string _path = AppPaths.Data("warnings.json");

        private WarningStore()
        {
            // Remove um .tmp orfao de um processo que morreu entre o FileStream e
            // o File.Move: ele fica no disco indefinidamente com o historico
            // completo de moderacao. Best-effort - se nao der para apagar, o
            // proximo WriteJsonAsync o sobrescreve de qualquer forma.
            try
            {
                var orphan = _path + ".tmp";
                if (File.Exists(orphan))
                    File.Delete(orphan);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[warnings] nao foi possivel limpar o .tmp orfao: {ex.Message}");
            }
        }

        public async Task<WarningFile> ReadAsync()
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

        /// <summary>Advertencias de um usuario num servidor, da mais nova para a mais velha.</summary>
        public async Task<List<Warning>> ListAsync(ulong guildId, ulong userId)
        {
            var file = await ReadAsync().ConfigureAwait(false);
            return file.warnings
                .Where(w => w.guildId == guildId && w.userId == userId)
                .OrderByDescending(w => w.createdAtUtc)
                .ToList();
        }

        /// <summary>
        /// Unica forma de escrever: le, aplica a mutacao e grava - tudo sob o
        /// mesmo lock.
        /// </summary>
        /// <summary>
        /// Unica forma de escrever: le, aplica a mutacao e grava - tudo sob o
        /// mesmo lock.
        ///
        /// A mutacao recebe um <see cref="WarningEdit"/> e diz explicitamente se
        /// mudou alguma coisa. Antes isso era descoberto serializando o arquivo
        /// INTEIRO duas vezes (antes e depois) e comparando as strings: dois
        /// passes sobre todo o historico do servidor, dentro do lock global e do
        /// orcamento de 3 segundos da interacao, so para saber se valia gravar.
        /// </summary>
        public async Task<T> UpdateAsync<T>(Func<WarningEdit, T> mutate)
        {
            await s_lock.WaitAsync().ConfigureAwait(false);
            try
            {
                var file = await ReadJsonAsync().ConfigureAwait(false);
                var edit = new WarningEdit(file);
                var result = mutate(edit);

                // Uma mutacao que nao alterou nada (um /delwarn com id
                // inexistente, por exemplo) nao reescreve o arquivo nem mexe no
                // lastUpdatedUtc.
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
        /// Teto de advertencias guardadas. Sem isto o arquivo so crescia, e cada
        /// /warn relia, reserializava e regravava o historico inteiro.
        /// </summary>
        private const int MaxStoredWarnings = 5000;

        private static void Prune(WarningFile file)
        {
            if (file.warnings.Count <= MaxStoredWarnings)
                return;

            // Descarta as mais antigas: advertencia recente e a que ainda importa
            // para decidir uma punicao.
            var keep = file.warnings
                .OrderByDescending(w => w.createdAtUtc)
                .Take(MaxStoredWarnings)
                .OrderBy(w => w.createdAtUtc)
                .ToList();

            var dropped = file.warnings.Count - keep.Count;
            file.warnings = keep;
            Console.WriteLine($"[warnings] podadas {dropped} advertencia(s) antiga(s); teto e {MaxStoredWarnings}.");
        }

        /// <summary>
        /// Id curto e legivel para o /delwarn.
        ///
        /// Oito caracteres de um Guid sao ~32 bits: no teto de 5000 advertencias a
        /// chance de colisao ja passa de 0,3% (paradoxo do aniversario), e uma
        /// colisao faz o /delwarn (que usa FirstOrDefault) apagar a advertencia
        /// errada em silencio. Use NewUniqueId dentro da mutacao sempre que houver
        /// o arquivo em maos.
        /// </summary>
        public static string NewId() => Guid.NewGuid().ToString("N")[..8];

        /// <summary>
        /// Id curto garantidamente unico dentro do arquivo. Chamado de dentro da
        /// mutacao do UpdateAsync, onde o conjunto atual de advertencias esta
        /// visivel e a unicidade pode ser conferida sem corrida.
        /// </summary>
        public static string NewUniqueId(WarningFile file)
        {
            string id;
            do
            {
                id = Guid.NewGuid().ToString("N")[..8];
            }
            while (file.warnings.Any(w => w.id == id));

            return id;
        }

        private async Task<WarningFile> ReadJsonAsync()
        {
            try
            {
                if (!File.Exists(_path))
                    return new WarningFile();

                var json = await File.ReadAllTextAsync(_path).ConfigureAwait(false);
                if (string.IsNullOrWhiteSpace(json))
                    return new WarningFile();

                return JsonConvert.DeserializeObject<WarningFile>(json) ?? new WarningFile();
            }
            catch (Exception ex)
            {
                // Arquivo corrompido nao pode virar "nenhuma advertencia", que o
                // proximo write consolidaria como verdade: melhor gritar.
                Console.WriteLine($"[warnings] falha ao ler {_path}: {ex.Message}");
                throw;
            }
        }

        private async Task WriteJsonAsync(WarningFile file)
        {
            var json = JsonConvert.SerializeObject(file, Formatting.Indented);
            var temp = _path + ".tmp";

            // O .tmp e sincronizado no disco ANTES do rename.
            //
            // WriteAllTextAsync + File.Move ja protegia contra escrita rasgada,
            // mas nao contra perda de energia: o rename podia chegar ao disco
            // antes do conteudo, e o que sobrava era um warnings.json truncado ou
            // vazio. Como o ReadJsonAsync trata vazio como "nenhuma
            // advertencia", isso apagaria o historico inteiro em silencio - o
            // oposto do que a escrita atomica existe para garantir.
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
