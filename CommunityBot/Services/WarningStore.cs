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

        private readonly string _path = Path.Combine("data", "warnings.json");

        private WarningStore()
        {
            Directory.CreateDirectory("data");
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
        public async Task<T> UpdateAsync<T>(Func<WarningFile, T> mutate)
        {
            await s_lock.WaitAsync().ConfigureAwait(false);
            try
            {
                var file = await ReadJsonAsync().ConfigureAwait(false);

                // Retrato de antes, para so gravar o que de fato mudou: uma
                // mutacao que nao alterou nada (um /delwarn com id inexistente,
                // por exemplo) nao deve reescrever o arquivo nem mexer no
                // lastUpdatedUtc.
                var before = JsonConvert.SerializeObject(file);
                var result = mutate(file);

                if (JsonConvert.SerializeObject(file) != before)
                {
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
        /// Id curto e legivel para o /delwarn.
        ///
        /// Oito caracteres de um Guid dao colisao desprezivel no volume de um
        /// servidor, e ninguem digita um Guid inteiro a mao.
        /// </summary>
        public static string NewId() => Guid.NewGuid().ToString("N")[..8];

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

            await File.WriteAllTextAsync(temp, json).ConfigureAwait(false);
            File.Move(temp, _path, overwrite: true);
        }
    }
}
