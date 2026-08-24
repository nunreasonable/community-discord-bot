using System;
using System.IO;
using System.Threading;
using System.Threading.Tasks;
using CommunityBot.Services;
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

        private static readonly string ConfigPath = AppPaths.Config("config.jsonc");

        // Cache do arquivo lido, invalidado pela data de modificacao.
        //
        // Cada comando cria um leitor novo, entao sem isto o config inteiro seria
        // lido e desserializado do disco a cada interacao - dentro do caminho que
        // precisa responder ao Discord em 3 segundos. Editar o config continua
        // valendo na hora: o carimbo de modificacao muda e a proxima leitura
        // recarrega, sem precisar reiniciar o bot.
        private static readonly SemaphoreSlim s_cacheLock = new(1, 1);

        /// <summary>
        /// Dado e carimbo num objeto so, publicado de uma vez.
        ///
        /// Antes eram dois estaticos separados, escritos sob o semaforo mas lidos
        /// FORA dele no caminho rapido. Sem barreira de memoria, um leitor podia
        /// ver o cache novo junto do carimbo velho (ou o contrario) e ficar preso
        /// a um config desatualizado ate o arquivo mudar de novo.
        /// </summary>
        private sealed record CachedConfig(JSONStructure? Data, DateTime Stamp);

        private static CachedConfig? s_cache;

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

            // Volatile.Read/Write: e o par (dado, carimbo) que precisa ser visto
            // inteiro, e como ele viaja num objeto so, basta publicar a
            // referencia com barreira.
            var cached = Volatile.Read(ref s_cache);
            if (cached is not null && stamp == cached.Stamp)
                return cached.Data;

            await s_cacheLock.WaitAsync().ConfigureAwait(false);
            try
            {
                // Confere de novo: outra chamada pode ter recarregado enquanto
                // esta esperava o lock.
                stamp = File.GetLastWriteTimeUtc(ConfigPath);
                cached = Volatile.Read(ref s_cache);
                if (cached is not null && stamp == cached.Stamp)
                    return cached.Data;

                var json = await File.ReadAllTextAsync(ConfigPath).ConfigureAwait(false);
                var data = JsonConvert.DeserializeObject<JSONStructure>(json);

                // Rele o carimbo DEPOIS do conteudo. Se o arquivo mudou entre o
                // GetLastWriteTimeUtc de cima e o ReadAllText, cachear com o
                // carimbo de antes guardava o conteudo NOVO sob o timestamp VELHO,
                // e o cache ficava preso nessa versao ate o arquivo mudar de novo.
                // Com o carimbo de depois, o pior caso e recarregar uma vez a mais.
                var postStamp = File.GetLastWriteTimeUtc(ConfigPath);
                Volatile.Write(ref s_cache, new CachedConfig(data, postStamp));
                return data;
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
