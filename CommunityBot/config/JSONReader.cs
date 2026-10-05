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

        /// <summary>Nunca nulo: sem a secao no arquivo, valem os padroes, que deixam o /verify desligado.</summary>
        public RobloxVerifySettings robloxVerify { get; private set; } = new();

        /// <summary>Nunca nulo: sem a secao no arquivo, valem os padroes de MusicSettings.</summary>
        public MusicSettings music { get; private set; } = new();

        /// <summary>Nunca nulo: sem a secao, o feed de commits fica desligado.</summary>
        public GitFeedSettings gitFeed { get; private set; } = new();

        // LEGADO. Estas seis chaves saíram do arquivo e viraram configuracao por
        // servidor, escrita pelo /config. Continuam sendo lidas para a migracao
        // unica do LegacyConfigMigration, e nada mais as consulta.
        public ulong? moderationLogChannelId { get; private set; }
        public ulong? autoSoftbanGuildId { get; private set; }
        public ulong? autoSoftbanChannelId { get; private set; }
        public ulong? ticketCategoryId { get; private set; }
        public ulong? ticketStaffRoleId { get; private set; }
        public ulong? ticketLogChannelId { get; private set; }

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
            robloxVerify = data?.robloxVerify ?? new RobloxVerifySettings();
            music = data?.music ?? new MusicSettings();
            gitFeed = data?.gitFeed ?? new GitFeedSettings();
            moderationLogChannelId = data?.moderationLogChannelId;
            autoSoftbanGuildId = data?.autoSoftbanGuildId;
            autoSoftbanChannelId = data?.autoSoftbanChannelId;
            ticketCategoryId = data?.ticketCategoryId;
            ticketStaffRoleId = data?.ticketStaffRoleId;
            ticketLogChannelId = data?.ticketLogChannelId;
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

                // Arquivo vazio (ou com o literal `null`) desserializa para NULL
                // SEM LANCAR. Aceitando isso, uma janela de truncamento no meio de
                // uma edicao - todo editor que escreve por truncate-then-write tem
                // uma - fazia todas as chaves virarem nulas: o vigia de canal se
                // desarmava sozinho e anunciava no log "desligado pelo config",
                // como se alguem tivesse pedido. Tratar como leitura falha mantem
                // o valor anterior de pe, que e o que o AutoSoftban documenta.
                var data = JsonConvert.DeserializeObject<JSONStructure>(json)
                    ?? throw new JsonException("config/config.jsonc esta vazio ou contem apenas 'null'.");

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

        /// <summary>Ligacao com o Worker de verificacao Roblox. Ver RobloxVerifySettings.</summary>
        public RobloxVerifySettings? robloxVerify { get; set; }

        /// <summary>Caminho do yt-dlp e limites da musica. Ver MusicSettings.</summary>
        public MusicSettings? music { get; set; }

        /// <summary>Repositorio git local cujos commits vao para um canal. Ver GitFeedSettings.</summary>
        public GitFeedSettings? gitFeed { get; set; }

        /// <summary>
        /// LEGADO, so para a migracao. Ver LegacyConfigMigration: estas seis
        /// chaves viraram configuracao por servidor e hoje se definem pelo
        /// /config. Manter aqui e o que permite trazer a configuracao antiga sem
        /// o operador redigitar nada.
        /// </summary>
        public ulong? moderationLogChannelId { get; set; }

        /// <summary>
        /// Servidor e canal vigiados pelo auto-softban. Os DOIS precisam estar
        /// preenchidos; com qualquer um deles nulo ou zero o vigia fica inerte.
        /// Sao chaves proprias de proposito: guildIds e registro de comando, e
        /// moderationLogChannelId e o destino do log - nenhum dos dois diz nada
        /// sobre onde vigiar.
        /// </summary>
        public ulong? autoSoftbanGuildId { get; set; }

        /// <inheritdoc cref="autoSoftbanGuildId"/>
        public ulong? autoSoftbanChannelId { get; set; }

        /// <summary>
        /// Categoria onde os canais de ticket nascem, e cargo que enxerga todo
        /// ticket. Os DOIS precisam estar preenchidos: sem categoria nao ha onde
        /// criar, e sem cargo nao ha como dar acesso a equipe - o acesso e um
        /// permission overwrite, e overwrite aponta para um cargo ou um membro,
        /// nunca para "quem tem tal permissao".
        /// </summary>
        public ulong? ticketCategoryId { get; set; }

        /// <inheritdoc cref="ticketCategoryId"/>
        public ulong? ticketStaffRoleId { get; set; }

        /// <summary>
        /// Canal que recebe o transcript e o registro de abertura/fechamento dos
        /// tickets. Chave propria, e nao o moderationLogChannelId: transcript e
        /// arquivo grande e frequente, e afogaria o registro de ban e
        /// advertencia. Sem ele nao ha como arquivar, e o fechamento passa a
        /// trancar o canal em vez de apagar.
        /// </summary>
        public ulong? ticketLogChannelId { get; set; }
    }

    /// <summary>
    /// Onde fica o Worker de verificacao Roblox e o segredo que o bot usa para
    /// buscar resultados nele.
    ///
    /// O segredo mora aqui, e nao por servidor, porque e da INSTALACAO: e o
    /// mesmo valor do BOT_API_SECRET do Worker, e quem o tem pode ler o
    /// resultado pendente de qualquer pessoa. Por isso fica neste arquivo, que
    /// ja esta no .gitignore por causa do token.
    /// </summary>
    internal sealed class RobloxVerifySettings
    {
        /// <summary>Pagina que a pessoa abre. Estatica: nao carrega identidade nenhuma.</summary>
        public string startUrl { get; set; } = "https://daeese.me/oauth/roblox/start";

        /// <summary>Endpoint que o bot consulta, com o segredo no Authorization.</summary>
        public string resultUrl { get; set; } = "https://daeese.me/oauth/roblox/result";

        public string? apiSecret { get; set; }

        public bool IsConfigured =>
            !string.IsNullOrWhiteSpace(apiSecret) &&
            Uri.TryCreate(startUrl, UriKind.Absolute, out var start) && start.Scheme == Uri.UriSchemeHttps &&
            Uri.TryCreate(resultUrl, UriKind.Absolute, out var result) && result.Scheme == Uri.UriSchemeHttps;
    }

    /// <summary>
    /// Musica: onde esta o yt-dlp e ate onde o bot aceita ir.
    ///
    /// Fica na INSTALACAO, e nao por servidor, porque todos os limites aqui sao
    /// do disco e da banda DESTA maquina - cada faixa e baixada inteira antes de
    /// tocar. Um servidor nao tem como saber quanto a maquina aguenta.
    /// </summary>
    internal sealed class MusicSettings
    {
        /// <summary>
        /// Caminho do executavel. Absoluto na pratica: o PATH do systemd de
        /// usuario e so /usr/local/bin:/usr/bin, e o binario standalone mora em
        /// ~/.local/bin.
        /// </summary>
        public string ytDlpPath { get; set; } = "yt-dlp";

        public string ffmpegPath { get; set; } = "ffmpeg";

        /// <summary>
        /// Valor do --js-runtimes do yt-dlp. O YouTube exige um runtime JS desde
        /// o fim de 2025; o yt-dlp so liga o Deno sozinho, e o que existe nesta
        /// maquina e o node.
        /// </summary>
        public string jsRuntimes { get; set; } = "node";

        /// <summary>Teto de um video do YouTube - o unico que e BAIXADO antes de tocar.</summary>
        public int maxTrackMinutes { get; set; } = 15;

        /// <summary>
        /// Teto do que toca por streaming (SoundCloud). Maior porque nao ocupa
        /// disco: mix de uma hora e comum la.
        /// </summary>
        public int maxStreamMinutes { get; set; } = 180;

        public int maxQueueLength { get; set; } = 50;

        /// <summary>Quantas faixas um link de playlist, album ou set pode enfileirar de uma vez.</summary>
        public int maxPlaylistTracks { get; set; } = 50;

        public int maxFileSizeMB { get; set; } = 60;

        /// <summary>Quanto o bot espera com a fila vazia, ou sozinho no canal, antes de sair.</summary>
        public int idleLeaveMinutes { get; set; } = 2;

        // Os limites passam por um piso e um teto aqui, e nao onde sao usados:
        // um zero ou negativo digitado no config viraria "nenhuma faixa cabe" ou
        // "sai do canal na hora", sem nada no log dizendo por que.
        public TimeSpan MaxTrackLength => TimeSpan.FromMinutes(Math.Clamp(maxTrackMinutes, 1, 180));
        public TimeSpan MaxStreamLength => TimeSpan.FromMinutes(Math.Clamp(maxStreamMinutes, 1, 720));
        public int MaxQueue => Math.Clamp(maxQueueLength, 1, 500);
        public int MaxPlaylist => Math.Clamp(maxPlaylistTracks, 1, 200);
        public int MaxFileSizeMegabytes => Math.Clamp(maxFileSizeMB, 5, 1024);
        public TimeSpan IdleLeave => TimeSpan.FromMinutes(Math.Clamp(idleLeaveMinutes, 1, 60));
    }

    /// <summary>
    /// Feed de commits: um repositorio git DESTA maquina e o canal que recebe o
    /// resumo de cada commit novo.
    ///
    /// Mora aqui, e nao no /config, de proposito: o repositorio so existe no
    /// disco de quem hospeda o bot, e nenhum outro servidor onde o Sollarety
    /// esta pode ligar - nem descobrir - isto.
    /// </summary>
    internal sealed class GitFeedSettings
    {
        /// <summary>Caminho absoluto do repositorio (a pasta que contem o .git).</summary>
        public string? repoPath { get; set; }

        public string branch { get; set; } = "main";
        public ulong? guildId { get; set; }
        public ulong? channelId { get; set; }

        /// <summary>Nome que aparece no topo de cada resumo.</summary>
        public string title { get; set; } = "Commits";

        public int pollSeconds { get; set; } = 30;
        public string gitPath { get; set; } = "git";

        public bool IsConfigured =>
            !string.IsNullOrWhiteSpace(repoPath) && guildId is > 0 && channelId is > 0 &&
            !string.IsNullOrWhiteSpace(branch);

        // Piso de 10s: cada volta roda um git, e menos que isso nao muda nada
        // para quem le o canal.
        public TimeSpan PollInterval => TimeSpan.FromSeconds(Math.Clamp(pollSeconds, 10, 3600));
    }
}
