using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using Newtonsoft.Json;

namespace CommunityBot.Services
{
    /// <summary>
    /// O que cada servidor configurou pelo /config.
    ///
    /// Nomes de propriedade em minuscula: sao as chaves do JSON direto, como no
    /// WarningStore e no TicketStore.
    /// </summary>
    internal sealed class GuildSettings
    {
        public ulong guildId { get; set; }

        public ulong? moderationLogChannelId { get; set; }
        public ulong? autoSoftbanChannelId { get; set; }
        public ulong? ticketCategoryId { get; set; }
        public ulong? ticketStaffRoleId { get; set; }
        public ulong? ticketLogChannelId { get; set; }

        // Verificacao Roblox. O vinculo em si e global (RobloxLinkStore); aqui
        // mora so o que ESTE servidor faz com ele.
        public ulong? verifiedRoleId { get; set; }
        public ulong? unverifiedRoleId { get; set; }
        /// <summary>Uma das chaves de NicknameFormats. Nulo e "nenhum": o bot nao mexe no apelido.</summary>
        public string? nicknameFormat { get; set; }
        /// <summary>Idade minima da conta Roblox, em dias. Zero desliga o filtro.</summary>
        public int minAccountAgeDays { get; set; }
        public List<GroupBind> groupBinds { get; set; } = new();

        // Niveis e economia: desligados ate quem administra o servidor ligar.
        // A ausencia da chave no arquivo vira false, entao servidores antigos
        // nao ganham nada sem pedir.
        public bool levelingEnabled { get; set; }
        public bool economyEnabled { get; set; }

        public DateTimeOffset? updatedAtUtc { get; set; }
        public ulong? updatedById { get; set; }

        public GuildSettings Copy() => new()
        {
            guildId = guildId,
            moderationLogChannelId = moderationLogChannelId,
            autoSoftbanChannelId = autoSoftbanChannelId,
            ticketCategoryId = ticketCategoryId,
            ticketStaffRoleId = ticketStaffRoleId,
            ticketLogChannelId = ticketLogChannelId,
            verifiedRoleId = verifiedRoleId,
            unverifiedRoleId = unverifiedRoleId,
            nicknameFormat = nicknameFormat,
            minAccountAgeDays = minAccountAgeDays,
            // Copia PROFUNDA: o snapshot e lido fora do lock, e uma lista
            // compartilhada com o mutador poderia ser vista pela metade.
            groupBinds = (groupBinds ?? new()).Select(b => b.Copy()).ToList(),
            levelingEnabled = levelingEnabled,
            economyEnabled = economyEnabled,
            updatedAtUtc = updatedAtUtc,
            updatedById = updatedById
        };
    }

    /// <summary>
    /// Regra "quem esta no grupo X com rank entre A e B ganha o cargo Y".
    ///
    /// Rank 0 e "fora do grupo", como o proprio Roblox conta. Um bind com
    /// minRank 0 casa tambem com quem nao esta no grupo.
    /// </summary>
    internal sealed class GroupBind
    {
        public string id { get; set; } = string.Empty;
        public long groupId { get; set; }
        public string groupName { get; set; } = string.Empty;
        public int minRank { get; set; }
        public int maxRank { get; set; }
        public ulong roleId { get; set; }
        public ulong createdById { get; set; }
        public DateTimeOffset createdAtUtc { get; set; }

        public bool Matches(int rank) => rank >= minRank && rank <= maxRank;

        public GroupBind Copy() => new()
        {
            id = id,
            groupId = groupId,
            groupName = groupName,
            minRank = minRank,
            maxRank = maxRank,
            roleId = roleId,
            createdById = createdById,
            createdAtUtc = createdAtUtc
        };
    }

    internal sealed class GuildSettingsFile
    {
        public List<GuildSettings> guilds { get; set; } = new();

        /// <summary>
        /// Marca que as chaves antigas do config.jsonc ja foram trazidas para ca.
        /// Fica no arquivo, e nao numa flag em memoria, porque a migracao precisa
        /// rodar uma vez na VIDA da instalacao e nao uma vez por processo.
        /// </summary>
        public bool migratedFromConfig { get; set; }

        public DateTimeOffset? lastUpdatedUtc { get; set; }
    }

    /// <summary>
    /// Visao mutavel entregue ao <see cref="GuildSettingsStore.UpdateAsync"/>.
    /// Quem muta precisa dizer que mutou.
    /// </summary>
    internal sealed class GuildSettingsEdit
    {
        public GuildSettingsEdit(GuildSettingsFile file) => File = file;

        public GuildSettingsFile File { get; }

        public bool Changed { get; private set; }

        public void MarkChanged() => Changed = true;

        /// <summary>
        /// O registro daquele servidor, criando um vazio se ainda nao existir.
        /// Nao marca alterado sozinho - quem chama decide.
        /// </summary>
        public GuildSettings Get(ulong guildId)
        {
            var existing = File.guilds.FirstOrDefault(g => g.guildId == guildId);
            if (existing is not null)
                return existing;

            var created = new GuildSettings { guildId = guildId };
            File.guilds.Add(created);
            return created;
        }
    }

    /// <summary>
    /// Configuracao POR SERVIDOR, escrita pelo /config e lida por todo o resto.
    ///
    /// Mesmo desenho do TicketStore: semaforo ESTATICO, escrita atomica com .tmp
    /// sincronizado antes do rename, mutador SINCRONO no UpdateAsync.
    ///
    /// DUAS diferencas deliberadas em relacao aos outros stores:
    ///
    /// 1. NAO TEM PODA. Os outros aparam historico antigo; aqui cada registro e a
    ///    configuracao viva de um servidor, e "podar o mais antigo" seria apagar
    ///    a configuracao de quem nao mexe nela ha tempo - exatamente o servidor
    ///    onde tudo ja esta funcionando. O arquivo cresce com o numero de
    ///    servidores, que e o numero certo.
    ///
    /// 2. TEM SNAPSHOT EM MEMORIA. O vigia de canal consulta isto a CADA mensagem
    ///    do gateway, e o caminho tem de ser sincrono e sem disco. O snapshot e
    ///    republicado inteiro a cada escrita, entao uma mudanca pelo /config vale
    ///    na mensagem seguinte - sem a janela de dez segundos que a leitura de
    ///    arquivo obrigava, e sem o defeito que vinha junto dela, de a primeira
    ///    mensagem depois de uma mudanca ser avaliada contra o alvo velho.
    /// </summary>
    internal sealed class GuildSettingsStore
    {
        public static GuildSettingsStore Instance { get; } = new();

        private static readonly SemaphoreSlim s_lock = new(1, 1);

        private readonly string _path = AppPaths.Data("guild-settings.json");

        /// <summary>
        /// Publicado por inteiro a cada escrita, nunca mutado no lugar. Os objetos
        /// dentro dele sao COPIAS: se fossem os mesmos que o mutador mexe, um
        /// leitor poderia ver um registro pela metade.
        /// </summary>
        private static volatile IReadOnlyDictionary<ulong, GuildSettings> s_snapshot =
            new Dictionary<ulong, GuildSettings>();

        private GuildSettingsStore()
        {
            try
            {
                var orphan = _path + ".tmp";
                if (File.Exists(orphan))
                    File.Delete(orphan);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[config] nao foi possivel limpar o .tmp orfao: {ex.Message}");
            }
        }

        /// <summary>
        /// A configuracao de um servidor, ou null se ele nunca configurou nada.
        /// Sincrono e sem I/O: e chamado no caminho do gateway.
        /// </summary>
        public static GuildSettings? For(ulong guildId) =>
            s_snapshot.TryGetValue(guildId, out var settings) ? settings : null;

        /// <summary>Todos os servidores com alguma configuracao.</summary>
        public static IReadOnlyDictionary<ulong, GuildSettings> All => s_snapshot;

        /// <summary>Carrega do disco e publica o snapshot. Chamado uma vez na subida.</summary>
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
        /// Unico caminho de escrita. Mutador SINCRONO: nao da para chamar a API do
        /// Discord com o arquivo travado.
        /// </summary>
        public async Task<T> UpdateAsync<T>(Func<GuildSettingsEdit, T> mutate)
        {
            await s_lock.WaitAsync().ConfigureAwait(false);
            try
            {
                var file = await ReadJsonAsync().ConfigureAwait(false);
                var edit = new GuildSettingsEdit(file);
                var result = mutate(edit);

                if (edit.Changed)
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

        /// <summary>
        /// Aplica uma mudanca na configuracao de um servidor e devolve como ela
        /// ficou. Guarda quem mexeu e quando, para o /config view poder dizer.
        /// </summary>
        public Task<GuildSettings> SetAsync(ulong guildId, ulong actorId, Action<GuildSettings> apply) =>
            UpdateAsync(edit =>
            {
                var settings = edit.Get(guildId);
                apply(settings);
                settings.updatedAtUtc = DateTimeOffset.UtcNow;
                settings.updatedById = actorId;
                edit.MarkChanged();
                return settings.Copy();
            });

        private static void Publish(GuildSettingsFile file) =>
            s_snapshot = file.guilds.ToDictionary(g => g.guildId, g => g.Copy());

        private async Task<GuildSettingsFile> ReadJsonAsync()
        {
            try
            {
                if (!File.Exists(_path))
                    return new GuildSettingsFile();

                var json = await File.ReadAllTextAsync(_path).ConfigureAwait(false);
                if (string.IsNullOrWhiteSpace(json))
                    return new GuildSettingsFile();

                return JsonConvert.DeserializeObject<GuildSettingsFile>(json) ?? new GuildSettingsFile();
            }
            catch (Exception ex)
            {
                // Arquivo corrompido nao pode virar "ninguem configurou nada": a
                // proxima escrita consolidaria isso e todo servidor perderia a
                // configuracao de uma vez. Melhor gritar.
                Console.WriteLine($"[config] falha ao ler {_path}: {ex.Message}");
                throw;
            }
        }

        private async Task WriteJsonAsync(GuildSettingsFile file)
        {
            var json = JsonConvert.SerializeObject(file, Formatting.Indented);
            var temp = _path + ".tmp";

            // .tmp sincronizado no disco ANTES do rename: sem isso o rename pode
            // chegar antes do conteudo e o que sobra e um arquivo truncado, que o
            // ReadJsonAsync trata como "ninguem configurou nada".
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
