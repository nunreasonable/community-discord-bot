using System;
using System.IO;
using System.Threading.Tasks;
using Newtonsoft.Json;

namespace CommunityBot.Services
{
    /// <summary>
    /// Leitura e escrita atomica de um arquivo JSON de data/.
    ///
    /// E o mesmo bloco que o WarningStore, o TicketStore, o GuildSettingsStore e
    /// o RobloxLinkStore carregam cada um a sua copia; os armazenamentos novos
    /// (niveis, economia) usam este, e os antigos podem migrar quando forem
    /// mexidos.
    /// </summary>
    internal static class JsonFile
    {
        /// <summary>
        /// Arquivo ausente ou vazio vira um objeto novo. Arquivo que nao
        /// desserializa LANCA: tratado como vazio, a proxima escrita
        /// consolidaria "nada" por cima dos dados, em silencio.
        /// </summary>
        public static async Task<T> ReadAsync<T>(string path, string tag) where T : new()
        {
            try
            {
                if (!File.Exists(path))
                    return new T();

                var json = await File.ReadAllTextAsync(path).ConfigureAwait(false);
                if (string.IsNullOrWhiteSpace(json))
                    return new T();

                return JsonConvert.DeserializeObject<T>(json) ?? new T();
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[{tag}] falha ao ler {path}: {ex.Message}");
                throw;
            }
        }

        /// <summary>
        /// Grava num .tmp, sincroniza no disco e SO ENTAO renomeia por cima. Sem
        /// o flush, uma queda de energia podia levar o rename ao disco antes do
        /// conteudo, e o que sobrava era um arquivo vazio - que a leitura acima
        /// trataria como "nenhum dado".
        /// </summary>
        public static async Task WriteAtomicAsync(string path, object value)
        {
            var json = JsonConvert.SerializeObject(value, Formatting.Indented);
            var temp = path + ".tmp";

            await using (var stream = new FileStream(temp, FileMode.Create, FileAccess.Write, FileShare.None))
            {
                await using var writer = new StreamWriter(stream);
                await writer.WriteAsync(json).ConfigureAwait(false);
                await writer.FlushAsync().ConfigureAwait(false);
                stream.Flush(flushToDisk: true);
            }

            File.Move(temp, path, overwrite: true);
        }

        /// <summary>Um .tmp que sobrou de um processo morto no meio da escrita.</summary>
        public static void DeleteLeftoverTemp(string path, string tag)
        {
            try
            {
                if (File.Exists(path + ".tmp"))
                    File.Delete(path + ".tmp");
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[{tag}] aviso: nao consegui apagar {path}.tmp: {ex.Message}");
            }
        }
    }
}
