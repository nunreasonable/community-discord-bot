using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using Newtonsoft.Json;

namespace CommunityBot.Services.Roblox
{
    /// <summary>
    /// Um vinculo Discord -> Roblox, provado pelo OAuth dos dois lados.
    ///
    /// O que identifica a conta e o robloxId. Nome e nome de exibicao mudam no
    /// Roblox sem aviso, entao ficam aqui so como a ultima leitura - o /update
    /// os renova.
    /// </summary>
    internal sealed class RobloxLink
    {
        public ulong discordId { get; set; }
        public long robloxId { get; set; }
        public string robloxName { get; set; } = string.Empty;
        public string displayName { get; set; } = string.Empty;
        public DateTimeOffset? robloxCreatedUtc { get; set; }
        public DateTimeOffset verifiedAtUtc { get; set; }
        public DateTimeOffset? refreshedAtUtc { get; set; }

        public RobloxLink Copy() => new()
        {
            discordId = discordId,
            robloxId = robloxId,
            robloxName = robloxName,
            displayName = displayName,
            robloxCreatedUtc = robloxCreatedUtc,
            verifiedAtUtc = verifiedAtUtc,
            refreshedAtUtc = refreshedAtUtc
        };
    }

    internal sealed class RobloxLinkFile
    {
        public List<RobloxLink> links { get; set; } = new();
        public DateTimeOffset? lastUpdatedUtc { get; set; }
    }

    /// <summary>
    /// Os vinculos, GLOBAIS: quem verifica uma vez fica verificado em todo
    /// servidor onde o bot esta, como no BloxLink.
    ///
    /// Mesmo desenho do GuildSettingsStore: semaforo estatico, escrita atomica
    /// com .tmp sincronizado antes do rename, e snapshot imutavel em memoria. O
    /// snapshot e o que deixa a entrada de membro consultar o vinculo sem disco
    /// no caminho do gateway.
    ///
    /// Sem poda, pelo mesmo motivo do GuildSettingsStore: cada registro e um
    /// vinculo vivo, e o jeito de apagar e o /unverify de quem e dono dele.
    /// </summary>
    internal sealed class RobloxLinkStore
    {
        public static RobloxLinkStore Instance { get; } = new();

        private static readonly SemaphoreSlim s_lock = new(1, 1);

        private readonly string _path = AppPaths.Data("roblox-links.json");

        private static volatile IReadOnlyDictionary<ulong, RobloxLink> s_snapshot =
            new Dictionary<ulong, RobloxLink>();

        private RobloxLinkStore()
        {
            try
            {
                var orphan = _path + ".tmp";
                if (File.Exists(orphan))
                    File.Delete(orphan);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[roblox] nao foi possivel limpar o .tmp orfao: {ex.Message}");
            }
        }

        /// <summary>O vinculo de alguem, ou null. Sincrono e sem I/O.</summary>
        public static RobloxLink? For(ulong discordId) =>
            s_snapshot.TryGetValue(discordId, out var link) ? link : null;

        public static int Count => s_snapshot.Count;

        public async Task LoadAsync()
        {
            await s_lock.WaitAsync().ConfigureAwait(false);
            try
            {
                Publish(await ReadJsonAsync().ConfigureAwait(false));
            }
            finally
            {
                s_lock.Release();
            }
        }

        /// <summary>
        /// Grava (ou troca) o vinculo de uma conta do Discord. Trocar de conta
        /// Roblox e so verificar de novo: o registro anterior e substituido.
        /// </summary>
        public Task<RobloxLink> SetAsync(RobloxLink link) =>
            UpdateAsync(file =>
            {
                file.links.RemoveAll(l => l.discordId == link.discordId);
                file.links.Add(link.Copy());
                return (true, link.Copy());
            });

        /// <summary>Apaga o vinculo. Devolve o que foi apagado, ou null se nao havia.</summary>
        public Task<RobloxLink?> RemoveAsync(ulong discordId) =>
            UpdateAsync(file =>
            {
                var existing = file.links.FirstOrDefault(l => l.discordId == discordId);
                if (existing is null)
                    return (false, (RobloxLink?)null);

                file.links.Remove(existing);
                return (true, (RobloxLink?)existing.Copy());
            });

        /// <summary>
        /// Renova nome, exibicao e data de criacao lidos da API publica.
        ///
        /// So se o vinculo ainda aponta para a MESMA conta: entre a leitura e a
        /// escrita a pessoa pode ter verificado outra, e gravar os nomes antigos
        /// por cima dela misturaria as duas.
        /// </summary>
        public Task<RobloxLink?> RefreshAsync(ulong discordId, RobloxUser user) =>
            UpdateAsync(file =>
            {
                var existing = file.links.FirstOrDefault(l => l.discordId == discordId);
                if (existing is null || existing.robloxId != user.Id)
                    return (false, existing?.Copy());

                existing.robloxName = user.Name;
                existing.displayName = user.DisplayName;
                existing.robloxCreatedUtc = user.Created ?? existing.robloxCreatedUtc;
                existing.refreshedAtUtc = DateTimeOffset.UtcNow;
                return (true, existing.Copy());
            });

        /// <summary>Mutador SINCRONO: nada de chamada de rede com o arquivo travado.</summary>
        private async Task<T> UpdateAsync<T>(Func<RobloxLinkFile, (bool Changed, T Result)> mutate)
        {
            await s_lock.WaitAsync().ConfigureAwait(false);
            try
            {
                var file = await ReadJsonAsync().ConfigureAwait(false);
                var (changed, result) = mutate(file);

                if (changed)
                {
                    file.lastUpdatedUtc = DateTimeOffset.UtcNow;
                    await WriteJsonAsync(file).ConfigureAwait(false);
                    Publish(file);
                }

                return result;
            }
            finally
            {
                s_lock.Release();
            }
        }

        private static void Publish(RobloxLinkFile file) =>
            s_snapshot = file.links
                .GroupBy(l => l.discordId)
                .ToDictionary(g => g.Key, g => g.Last().Copy());

        private async Task<RobloxLinkFile> ReadJsonAsync()
        {
            try
            {
                if (!File.Exists(_path))
                    return new RobloxLinkFile();

                var json = await File.ReadAllTextAsync(_path).ConfigureAwait(false);
                if (string.IsNullOrWhiteSpace(json))
                    return new RobloxLinkFile();

                return JsonConvert.DeserializeObject<RobloxLinkFile>(json) ?? new RobloxLinkFile();
            }
            catch (Exception ex)
            {
                // Arquivo corrompido nao pode virar "ninguem verificou": a proxima
                // escrita consolidaria isso e todo mundo perderia o vinculo.
                Console.WriteLine($"[roblox] falha ao ler {_path}: {ex.Message}");
                throw;
            }
        }

        private async Task WriteJsonAsync(RobloxLinkFile file)
        {
            var json = JsonConvert.SerializeObject(file, Formatting.Indented);
            var temp = _path + ".tmp";

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
