using System;
using System.IO;
using System.Threading;
using System.Threading.Tasks;
using Newtonsoft.Json;

namespace CommunityBot.config
{
    /// <summary>
    /// Leitor do config, no mesmo molde do ccore.
    /// </summary>
    internal class JSONReader
    {
        public string? token { get; private set; }
        public ulong[]? guildIds { get; private set; }
        public ulong? moderationLogChannelId { get; private set; }

        private const string ConfigPath = "config/config.jsonc";

        // Cache do arquivo lido, invalidado pela data de modificacao.
        //
        // Cada comando cria um leitor novo, entao sem isto o config inteiro seria
        // lido e desserializado do disco a cada interacao - dentro do caminho que
        // precisa responder ao Discord em 3 segundos. Editar o config continua
        // valendo na hora: o carimbo de modificacao muda e a proxima leitura
        // recarrega, sem precisar reiniciar o bot.
        private static readonly SemaphoreSlim s_cacheLock = new(1, 1);
        private static JSONStructure? s_cache;
        private static DateTime s_cacheStamp;

        public async Task ReadJSON()
        {
            var data = await LoadAsync();

            token = data?.token;
            guildIds = data?.guildIds;
            moderationLogChannelId = data?.moderationLogChannelId;
        }

        /// <summary>
        /// Devolve o config desserializado, relendo do disco apenas quando o
        /// arquivo mudou. O objeto e compartilhado entre os leitores - ninguem
        /// escreve nele, e por isso os campos aqui sao todos `private set`.
        /// </summary>
        private static async Task<JSONStructure?> LoadAsync()
        {
            var stamp = File.GetLastWriteTimeUtc(ConfigPath);
            if (s_cache is not null && stamp == s_cacheStamp)
                return s_cache;

            await s_cacheLock.WaitAsync().ConfigureAwait(false);
            try
            {
                // Confere de novo: outra chamada pode ter recarregado enquanto
                // esta esperava o lock.
                stamp = File.GetLastWriteTimeUtc(ConfigPath);
                if (s_cache is not null && stamp == s_cacheStamp)
                    return s_cache;

                var json = await File.ReadAllTextAsync(ConfigPath).ConfigureAwait(false);
                s_cache = JsonConvert.DeserializeObject<JSONStructure>(json);
                s_cacheStamp = stamp;
                return s_cache;
            }
            finally
            {
                s_cacheLock.Release();
            }
        }
    }

    internal sealed class JSONStructure
    {
        public string? token { get; set; }

        /// <summary>Vazio ou ausente registra os comandos globalmente.</summary>
        public ulong[]? guildIds { get; set; }

        /// <summary>Canal que recebe um embed por acao de moderacao.</summary>
        public ulong? moderationLogChannelId { get; set; }
    }
}
