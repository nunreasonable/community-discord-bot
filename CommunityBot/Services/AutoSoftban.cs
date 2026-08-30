using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using DisCatSharp;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.EventArgs;

namespace CommunityBot.Services
{
    /// <summary>
    /// Vigia de canal: quem escrever no canal configurado leva um softban.
    ///
    /// O canal e uma armadilha - existe para NAO ser usado. Quem escreve la
    /// perde o historico recente e sai do servidor, mas pode voltar por convite,
    /// que e a diferenca entre isto e um ban de verdade.
    ///
    /// Desligado por padrao: sem autoSoftbanGuildId E autoSoftbanChannelId no
    /// config, nao ha alvo e nenhuma mensagem passa do primeiro portao.
    ///
    /// Le apenas QUEM escreveu e ONDE - nunca o texto. E por isso que o bot
    /// continua sem o intent privilegiado MessageContent: sem ele o Content
    /// chega vazio, e aqui isso nao faz falta nenhuma.
    /// </summary>
    internal static class AutoSoftban
    {
        /// <summary>
        /// Permissoes que isentam do vigia. Teste bit a bit com QUALQUER uma
        /// delas - e nao PermissionMethods.HasPermission, que exige todas as
        /// flags passadas juntas e portanto so pegaria quem tivesse as tres.
        /// </summary>
        private const Permissions StaffPermissions =
            Permissions.BanMembers | Permissions.ManageGuild | Permissions.Administrator;

        /*
         * O ALVO VEM DO GuildSettingsStore, que ja mantem um snapshot em memoria.
         *
         * Antes havia aqui um `volatile Target?` recarregado do disco a cada dez
         * segundos, com marca de tempo e guarda de concorrencia, so porque a
         * configuracao era um arquivo editado por fora e precisava ser vigiado.
         * Com o /config escrevendo pelo proprio processo, o store republica o
         * snapshot na hora da escrita - e junto com aquela maquinaria toda foi
         * embora o defeito que vinha nela: a primeira mensagem depois de uma
         * mudanca era avaliada contra o alvo velho. Agora vale na mensagem
         * seguinte, sempre.
         */

        /// <summary>
        /// Quem ja foi pego ha pouco. Uma rajada de mensagens da mesma pessoa
        /// chega como varios eventos, e sem isto cada uma viraria um ban, um
        /// unban e um embed de log a mais para o mesmo caso.
        /// </summary>
        /// A chave e (servidor, usuario). So o id do usuario bastava enquanto o
        /// vigia enxergava um servidor so; com varios, a mesma pessoa caindo na
        /// armadilha de dois servidores em menos de 30 segundos so seria punida
        /// no primeiro.
        private static readonly ConcurrentDictionary<(ulong Guild, ulong User), DateTimeOffset> s_recent = new();

        private static readonly TimeSpan RecentWindow = TimeSpan.FromSeconds(30);

        /*
         * DISJUNTOR.
         *
         * O vigia bane sozinho, e as duas maneiras de isso dar muito errado sao
         * a mesma coisa vista de dois lados: um id de canal apontando para um
         * canal movimentado, e um raid. Nos dois casos o comportamento correto
         * nao e continuar banindo mais rapido - e parar e chamar alguem.
         *
         * Passando de BreakerLimit softbans APLICADOS em BreakerWindow, o vigia
         * se desarma e fica assim ate o processo reiniciar. Rearmar sozinho
         * significaria voltar a banir exatamente na situacao que fez o disjuntor
         * disparar. O /softban manual nao e afetado: a moderacao continua com
         * ferramenta na mao.
         *
         * CONTRAPARTIDA CONHECIDA: cinco contas descartaveis escrevendo na
         * armadilha desarmam o vigia de proposito e ele so volta no restart. E o
         * preco de nao rearmar sozinho. Por isso o desarme grita - no log, no
         * canal de moderacao, e de novo a cada TrippedNotice enquanto durar -
         * em vez de ficar mudo esperando alguem reparar.
         */
        private const int BreakerLimit = 5;
        private static readonly TimeSpan BreakerWindow = TimeSpan.FromMinutes(1);
        private static readonly TimeSpan TrippedNotice = TimeSpan.FromMinutes(5);

        /// <summary>
        /// Softbans em andamento. Existe so para o encerramento poder esperar por
        /// eles: um ban ja aplicado cujo unban ainda nao rodou e um BANIMENTO DE
        /// VERDADE, e o processo morrendo no meio deixa a pessoa banida sem uma
        /// linha sequer de log - o ramo UnbanFailed nunca chega a rodar. A janela
        /// nao e teorica: sao ate tres tentativas de unban com espera entre elas,
        /// e um `systemctl restart` durante a rajada que o vigia existe para
        /// conter cai exatamente ali.
        /// </summary>
        private static int s_inFlight;

        /// <summary>
        /// Estado do disjuntor de UM servidor.
        ///
        /// Por servidor, e nao do processo: um raid no servidor A nao e evidencia
        /// nenhuma sobre o servidor B, e com o estado global o raid de um
        /// desarmaria a armadilha de todos os outros - em silencio, porque o
        /// aviso de desarme vai para o log de moderacao de um servidor so.
        /// </summary>
        private sealed class Breaker
        {
            public readonly Queue<DateTimeOffset> Applied = new();
            public bool Tripped;
            public long LastNoticeTicks;
        }

        private static readonly object s_breakerLock = new();
        private static readonly Dictionary<ulong, Breaker> s_breakers = new();

        private static Breaker BreakerFor(ulong guildId)
        {
            // Sempre sob s_breakerLock.
            if (!s_breakers.TryGetValue(guildId, out var breaker))
                s_breakers[guildId] = breaker = new Breaker();

            return breaker;
        }

        public static Task OnMessageCreated(DiscordClient client, MessageCreateEventArgs e)
        {
            // Portoes baratos, sincronos, no caminho do gateway: nada de rede nem
            // de disco. A esmagadora maioria das mensagens do servidor nao tem
            // nada a ver com o vigia e precisa sair daqui rapido.
            if (e.Guild is null)
                return Task.CompletedTask;

            // Bot e webhook nao sao gente e nao ha o que banir. Sai sem logar:
            // este portao vale para o servidor inteiro, entao uma linha aqui
            // seria ruido puro.
            if (e.Author.IsBot || e.Message.WebhookMessage)
                return Task.CompletedTask;

            // Lookup no snapshot em memoria: sincrono, sem disco, sem alocacao.
            // Este e o caminho por onde passa TODA mensagem do servidor.
            var trap = GuildSettingsStore.For(e.Guild.Id)?.autoSoftbanChannelId;
            if (trap is not > 0 || !ChannelMatches(e.Channel, trap.Value))
                return Task.CompletedTask;

            // Desarmado NESTE servidor. Avisa de vez em quando em vez de ficar
            // mudo: sem isto, um vigia morto e indistinguivel de uma armadilha que
            // ninguem pisou. Estrangulado porque durante um raid o proprio aviso
            // viraria o flood que o disjuntor existe para conter.
            if (IsTripped(e.Guild.Id))
            {
                NoticeStillTripped(e.Guild.Id, e.Author.Id);
                return Task.CompletedTask;
            }

            var guild = e.Guild;
            var author = e.Author;
            var channel = e.Channel;
            var message = e.Message;

            // So agora sai do caminho do gateway, e so para mensagem que ja
            // sabemos ser da armadilha.
            _ = Task.Run(async () =>
            {
                try
                {
                    await HandleAsync(client, guild, channel, author, message);
                }
                catch (Exception ex)
                {
                    // Nada foi aplicado neste caminho: solta a marca para a
                    // proxima mensagem tentar de novo.
                    Release(guild.Id, author.Id);
                    Console.WriteLine($"[autosoftban] falha ao processar {author.Id}: {ex}");
                }
            });

            return Task.CompletedTask;
        }

        /// <summary>
        /// Decide se a mensagem caiu na armadilha.
        ///
        /// Thread filha do canal vigiado CONTA. Sem isso, qualquer um com
        /// permissao de criar thread abria uma no canal-armadilha e conversava
        /// la a vontade: para quem le, esta escrevendo na armadilha; para o
        /// vigia, o id do canal e outro.
        ///
        /// O teste de TIPO e o que torna isso seguro. ParentId de um canal comum
        /// e a CATEGORIA dele, entao casar por ParentId sem olhar o tipo faria o
        /// id de uma categoria pegar todo canal dentro dela - que era a razao de
        /// so aceitar id exato antes.
        /// </summary>
        private static bool ChannelMatches(DiscordChannel channel, ulong targetId)
        {
            if (channel.Id == targetId)
                return true;

            return channel.Type is ChannelType.PublicThread or ChannelType.PrivateThread or ChannelType.NewsThread
                   && channel.ParentId == targetId;
        }

        private static async Task HandleAsync(DiscordClient client, DiscordGuild guild, DiscordChannel channel,
            DiscordUser author, DiscordMessage message)
        {
            /*
             * Aviso de entrada no servidor, pin, boost e afins aparecem no canal
             * com o usuario como autor sem que ele tenha escrito coisa alguma.
             *
             * MessageType e `MessageType?` - anulavel. Um null, ou um tipo que o
             * enum desta versao ainda nao conhece, cai neste `is not` e a
             * mensagem seria descartada. Como aqui ela ja e comprovadamente da
             * armadilha, o descarte deixa rastro: da para investigar por que a
             * armadilha nao disparou, em vez de ficar sem nada.
             *
             * Continua falhando FECHADO - na duvida nao bane -, so que visivel.
             */
            var type = message.MessageType;
            if (type is not (MessageType.Default or MessageType.Reply))
            {
                Console.WriteLine($"[autosoftban] ignorado {author.Id}: tipo de mensagem " +
                                  $"{(type?.ToString() ?? "null")} nao e conversa");
                return;
            }

            if (!TryClaim(guild.Id, author.Id))
                return;

            await ApplyAsync(client, guild, channel, author);
        }

        /// <summary>
        /// A mensagem ja e da armadilha quando se chega aqui, entao toda recusa
        /// daqui para baixo vira uma linha de log: "por que fulano nao foi
        /// banido?" e a primeira pergunta que aparece, e o /logs le exatamente
        /// estas linhas.
        ///
        /// REGRA DA MARCA: todo caminho cujo motivo pode MUDAR nos proximos 30
        /// segundos tem de chamar Release. A marca do dedup existe para nao punir duas vezes a
        /// mesma rajada - se ela sobrevivesse a uma falha passageira, viraria
        /// uma janela de 30 segundos de impunidade concedida por um 500 da API.
        /// </summary>
        private static async Task ApplyAsync(DiscordClient client, DiscordGuild guild, DiscordChannel channel, DiscordUser author)
        {
            var member = await Hierarchy.TryGetMemberAsync(guild, author.Id);
            if (member is null)
            {
                Console.WriteLine($"[autosoftban] ignorado {author.Id}: nao esta mais no servidor");
                Release(guild.Id, author.Id);
                return;
            }

            /*
             * Uniao dos dois calculos de permissao, e nao um ou outro.
             *
             * `member.Permissions` e a soma dos cargos - permissao de SERVIDOR,
             * que ignora os overwrites do canal. `channel.PermissionsFor` e o
             * calculo efetivo naquele canal, que aplica os overwrites daquele
             * canal (e so dele: numa thread, que nao tem overwrites proprios,
             * ele nao consulta o canal pai e a uniao recai sobre a permissao de
             * servidor).
             *
             * A uniao so ALARGA a isencao. Qualquer que seja o dos dois que
             * esteja errado num arranjo especifico de cargos e overwrites, o erro
             * cai para o lado de nao banir um moderador - que e o lado certo para
             * errar num recurso que age sozinho.
             */
            var effective = member.Permissions | channel.PermissionsFor(member);
            if ((effective & StaffPermissions) != 0)
            {
                Console.WriteLine($"[autosoftban] ignorado {author.Id}: tem permissao de moderacao");
                return;
            }

            var botMember = guild.CurrentMember;
            if (botMember is null)
            {
                // Cache frio logo apos reconectar. Nao da para conferir
                // hierarquia nenhuma, entao recusa - e solta a marca, porque
                // isto nao e culpa de quem escreveu.
                Console.WriteLine($"[autosoftban] adiado {author.Id}: nao consegui ler o meu proprio membro ainda");
                Release(guild.Id, author.Id);
                return;
            }

            // Conferido a cada acao, e nao so na subida: alguem pode tirar a
            // permissao do cargo do bot com o processo ja rodando, e sem esta
            // checagem cada mensagem viraria um 403 - queimando uma vaga do
            // disjuntor por vez ate desarmar o vigia por um motivo que nao e
            // raid nenhum.
            if ((botMember.Permissions & Permissions.BanMembers) == 0)
            {
                Console.WriteLine($"[autosoftban] nao posso punir {author.Id}: estou sem Ban Members neste servidor");
                Release(guild.Id, author.Id);
                return;
            }

            // O bot e o proprio ator: nao ha moderador humano por tras. Isso
            // reaproveita a checagem inteira - dono do servidor, o proprio bot e
            // quem esta acima do cargo dele ficam de fora.
            var blocked = Hierarchy.Check(guild, botMember, member, botMember);
            if (blocked is not null)
            {
                Console.WriteLine($"[autosoftban] ignorado {author.Id}: {blocked.Title}");
                return;
            }

            // O disjuntor e um portao de ADMISSAO, conferido imediatamente antes
            // de agir: a vaga e reservada de forma atomica, sob o mesmo lock que
            // conta. Contar depois do ban nao funcionaria - numa rajada, dezenas
            // de tarefas passariam pela checagem antes de a primeira terminar e
            // disparar, e o disjuntor estouraria o proprio limite em muito.
            if (!TryAdmit(guild.Id, out var justTripped))
            {
                if (justTripped)
                    await AnnounceTripAsync(client, guild, channel);

                Release(guild.Id, author.Id);
                return;
            }

            Interlocked.Increment(ref s_inFlight);
            Softban.SoftbanResult result;
            try
            {
                result = await Softban.ApplyAsync(guild, author.Id, AuditReason(channel));
            }
            finally
            {
                Interlocked.Decrement(ref s_inFlight);
            }

            switch (result.Outcome)
            {
                case Softban.SoftbanOutcome.BanFailed:
                    // Devolve a vaga: ninguem foi punido. Contar tentativa aqui
                    // fazia cinco 429 ou 5xx passageiros desarmarem o vigia ate o
                    // restart, com um aviso culpando raid ou canal errado - nem um
                    // nem outro verdade. O caso "bot sem Ban Members", que era a
                    // justificativa para contar tentativa, ja e barrado antes do
                    // TryAdmit.
                    ReleaseAdmission(guild.Id);
                    Release(guild.Id, author.Id);
                    Console.WriteLine($"[autosoftban] falha ao banir {author.Id}: {result.Error}");
                    return;

                case Softban.SoftbanOutcome.UnbanFailed:
                    Console.WriteLine($"[autosoftban] erro: {author.Id} banido, mas o unban falhou: {result.Error}");
                    await ModerationLog.RecordAsync(client, guild.Id, "Softban automático", author, client.CurrentUser,
                        $"Mensagem no canal vigiado {channel.Mention}",
                        "⚠️ O desbanimento falhou — o usuário continua BANIDO e precisa ser desbanido à mão.");
                    return;

                default:
                    Console.WriteLine($"[autosoftban] {author.Id} levou softban por escrever em {channel.Id}");
                    await ModerationLog.RecordAsync(client, guild.Id, "Softban automático", author, client.CurrentUser,
                        $"Mensagem no canal vigiado {channel.Mention}",
                        "Mensagens dos últimos 7 dias apagadas. O usuário pode voltar por convite.");
                    return;
            }
        }

        /// <summary>
        /// Texto do Audit Log. Sem quebra de linha e sem caractere de controle: o
        /// valor viaja num CABECALHO HTTP, e um \n ali quebra a requisicao inteira
        /// - o softban falharia antes de comecar, por causa do nome de um canal.
        /// </summary>
        private static string AuditReason(DiscordChannel channel) =>
            // Services.AuditReason, e nao uma copia: esta versao REMOVIA os
            // caracteres de controle enquanto a de la os TROCA POR ESPACO, e a
            // classe de la existe justamente com um comentario dizendo que essa
            // limpeza nao pode existir em copias.
            Services.AuditReason.Automatic($"auto-softban: escreveu no canal vigiado #{channel.Name}");

        /// <summary>
        /// Reserva uma vaga no disjuntor. Devolve false quando o vigia nao pode
        /// agir - e <paramref name="justTripped"/> diz se foi ESTA chamada que o
        /// desarmou, para o aviso sair uma vez so.
        ///
        /// A vaga e reservada antes de agir e DEVOLVIDA quando o ban falha, entao
        /// o que este disjuntor conta e punicao APLICADA - que e o que a mensagem
        /// de desarme afirma e o que o README promete.
        /// </summary>
        private static bool TryAdmit(ulong guildId, out bool justTripped)
        {
            justTripped = false;

            lock (s_breakerLock)
            {
                var breaker = BreakerFor(guildId);
                if (breaker.Tripped)
                    return false;

                var now = DateTimeOffset.UtcNow;

                while (breaker.Applied.Count > 0 && now - breaker.Applied.Peek() > BreakerWindow)
                    breaker.Applied.Dequeue();

                if (breaker.Applied.Count >= BreakerLimit)
                {
                    breaker.Tripped = true;
                    justTripped = true;
                    return false;
                }

                breaker.Applied.Enqueue(now);
                return true;
            }
        }

        private static bool IsTripped(ulong guildId)
        {
            lock (s_breakerLock)
                return s_breakers.TryGetValue(guildId, out var breaker) && breaker.Tripped;
        }

        /// <summary>Devolve ao disjuntor uma vaga reservada que nao virou punicao.</summary>
        private static void ReleaseAdmission(ulong guildId)
        {
            lock (s_breakerLock)
            {
                var breaker = BreakerFor(guildId);
                if (breaker.Applied.Count > 0)
                    breaker.Applied.Dequeue();
            }
        }

        /// <summary>
        /// Espera os softbans em voo terminarem, ate o teto dado. Chamado no
        /// encerramento, antes de largar o gateway.
        /// </summary>
        public static async Task DrainAsync(TimeSpan budget)
        {
            var deadline = DateTimeOffset.UtcNow + budget;

            while (Volatile.Read(ref s_inFlight) > 0 && DateTimeOffset.UtcNow < deadline)
                await Task.Delay(100);

            var left = Volatile.Read(ref s_inFlight);
            if (left > 0)
                Console.WriteLine($"[autosoftban] erro: encerrando com {left} softban(s) em voo. " +
                                  "Confira a lista de banidos - pode ter ficado alguem banido sem o desbanimento.");
        }

        private static async Task AnnounceTripAsync(DiscordClient client, DiscordGuild guild, DiscordChannel channel)
        {
            // "disjuntor" e palavra reconhecida pelo BotLogBuffer.Classify como
            // ERRO. Sem isso esta linha - a mais importante que o vigia produz -
            // seria arquivada como INFO e sumiria de `/logs nivel:erro`.
            Console.WriteLine($"[autosoftban] DISJUNTOR disparado: {BreakerLimit} softbans em menos de " +
                              $"{BreakerWindow.TotalSeconds:0}s no canal {channel.Id}. " +
                              "Vigia desarmado ate o bot reiniciar.");

            lock (s_breakerLock) BreakerFor(guild.Id).LastNoticeTicks = DateTimeOffset.UtcNow.Ticks;

            await ModerationLog.AlertAsync(client, guild.Id, "Vigia de canal desarmado",
                $"O softban automático chegou a {BreakerLimit} punições em menos de " +
                $"{BreakerWindow.TotalSeconds:0} segundos em {channel.Mention} e **se desarmou**.\n\n" +
                "Isso costuma significar um raid, um `autoSoftbanChannelId` apontando para o canal errado, " +
                "ou o bot sem permissão de banir. Confira antes de religar.\n\n" +
                "O vigia só volta quando o bot reiniciar. O `/softban` manual continua funcionando.");
        }

        /// <summary>
        /// Lembra, de tempos em tempos, que a armadilha esta desarmada. Um vigia
        /// morto calado e indistinguivel de um canal em que ninguem escreveu.
        /// </summary>
        private static void NoticeStillTripped(ulong guildId, ulong authorId)
        {
            var now = DateTimeOffset.UtcNow.Ticks;

            lock (s_breakerLock)
            {
                var breaker = BreakerFor(guildId);
                if (TimeSpan.FromTicks(now - breaker.LastNoticeTicks) < TrippedNotice)
                    return;

                breaker.LastNoticeTicks = now;
            }

            Console.WriteLine($"[autosoftban] disjuntor do servidor {guildId} continua desarmado: mensagem de " +
                              $"{authorId} na armadilha foi ignorada. Reinicie o bot para religar o vigia.");
        }

        /*
         * DIAGNOSTICO DE SUBIDA.
         *
         * Sem isto, as maneiras de errar a configuracao produzem o MESMO sintoma
         * que "ninguem escreveu no canal": silencio absoluto. O canal pode nao
         * existir, pode ser invisivel para o bot - e ai o gateway nem entrega a
         * mensagem -, ou pode ser um forum, onde as mensagens moram em threads.
         *
         * Junta TODOS os problemas numa lista em vez de parar no primeiro: um
         * canal invisivel escondia a falta de Ban Members, e o operador arrumava
         * um, reiniciava, e descobria o outro.
         *
         * Roda no GuildDownloadCompleted, e nao no Ready: o Ready dispara antes
         * dos servidores entrarem no cache, e ali guild e canal ainda nao existem
         * para consultar. Ele re-dispara a cada reconexao, entao o relatorio so
         * e escrito quando MUDA.
         */
        // Sincrono agora: a leitura de config saiu daqui e virou um lookup em
        // memoria. A assinatura continua devolvendo Task porque e um handler de
        // evento do DisCatSharp.
        public static Task ReportStatusAsync(DiscordClient client, GuildDownloadCompletedEventArgs e)
        {
            try
            {
                var report = BuildStatusReport(client);
                if (report == Volatile.Read(ref s_lastReport))
                    return Task.CompletedTask;

                Volatile.Write(ref s_lastReport, report);
                Console.WriteLine(report);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[autosoftban] falha ao conferir a configuracao do vigia: {ex.Message}");
            }

            return Task.CompletedTask;
        }

        private static string? s_lastReport;

        private static string BuildStatusReport(DiscordClient client)
        {
            // Um relatorio por servidor com armadilha configurada. Antes havia um
            // alvo unico no processo, entao havia uma linha unica.
            var lines = new List<string>();

            foreach (var guild in client.Guilds.Values)
            {
                var trap = GuildSettingsStore.For(guild.Id)?.autoSoftbanChannelId;
                if (trap is not > 0)
                    continue;

                lines.Add(DescribeGuild(guild, trap.Value));
            }

            if (lines.Count == 0)
                return "[autosoftban] vigia de canal desligado em todos os servidores (use /config auto-softban).";

            return string.Join("\n", lines);
        }

        private static string DescribeGuild(DiscordGuild guild, ulong channelId)
        {
            // Thread configurada direto como armadilha e arranjo valido.
            // Guild.Threads lista apenas as ATIVAS, entao uma thread arquivada nao
            // aparece aqui - por isso o texto do canal ausente admite essa
            // possibilidade em vez de afirmar que o vigia esta morto.
            if (!guild.Channels.TryGetValue(channelId, out var channel) &&
                guild.Threads.TryGetValue(channelId, out var thread))
                channel = thread;

            if (channel is null)
                return $"[autosoftban] AVISO: canal {channelId} nao esta no cache de \"{guild.Name}\". " +
                       "Ou ele nao existe, ou esta invisivel para o bot, ou e uma thread arquivada " +
                       "(nesse caso o vigia volta a valer quando ela for reaberta).";

            var problems = new List<string>();
            var notes = new List<string>();

            // Forum e midia FUNCIONAM: uma publicacao de forum e uma PublicThread
            // cujo pai e o canal do forum, e o ChannelMatches casa thread pelo pai.
            if (channel.Type is ChannelType.Forum or ChannelType.GuildMedia)
                notes.Add($"canal de {channel.Type}: o vigia pega as publicacoes, que sao threads filhas");

            if (channel.Type is ChannelType.Category)
                problems.Add("e uma CATEGORIA, que nao recebe mensagem");

            var bot = guild.CurrentMember;
            if (bot is null)
            {
                problems.Add("nao consegui ler o meu proprio membro neste servidor para conferir permissoes");
            }
            else
            {
                if ((channel.PermissionsFor(bot) & Permissions.AccessChannels) == 0)
                    problems.Add("nao tenho permissao de ver o canal, e o gateway nao entrega mensagem de canal " +
                                 "que o bot nao enxerga");

                if ((bot.Permissions & Permissions.BanMembers) == 0)
                    problems.Add("estou sem Ban Members neste servidor, entao todo softban vai dar erro");
            }

            // O disjuntor grita no canal de log de moderacao DESTE servidor.
            if (GuildSettingsStore.For(guild.Id)?.moderationLogChannelId is not > 0)
                problems.Add("nao ha canal de log de moderacao (/config log-moderacao), entao o aviso de " +
                             "disjuntor desarmado nao chega a canal nenhum");

            var extra = notes.Count > 0 ? $" ({string.Join("; ", notes)})" : string.Empty;

            if (problems.Count == 0)
                return $"[autosoftban] vigia ligado em \"{channel.Name}\" ({channel.Id}) do servidor \"{guild.Name}\"{extra}.";

            return $"[autosoftban] AVISO: vigia apontado para \"{channel.Name}\" ({channel.Id}) em \"{guild.Name}\"{extra}, " +
                   $"mas: {string.Join("; ", problems)}.";
        }

        /// <summary>
        /// Marca o usuario como ja tratado e devolve false se ele ja estava
        /// marcado. Poda os vencidos na passagem - a janela e curta e o dicionario
        /// so cresce com quem cai na armadilha.
        /// </summary>
        private static bool TryClaim(ulong guildId, ulong userId)
        {
            var now = DateTimeOffset.UtcNow;

            // Remocao por COMPARACAO: entre montar a lista de vencidos e remover,
            // a janela de alguem pode expirar e uma outra thread pode registrar
            // uma marca nova para o mesmo id. Um TryRemove(key) cru apagaria essa
            // marca nova, e a mesma rajada renderia dois bans.
            foreach (var stale in s_recent.Where(kv => now - kv.Value > RecentWindow).ToList())
                s_recent.TryRemove(stale);

            return s_recent.TryAdd((guildId, userId), now);
        }

        /// <summary>
        /// Desfaz a marca. Para todo caminho em que NADA aconteceu com o alvo:
        /// depois de um ban aplicado, repetir seria punir a mesma pessoa duas
        /// vezes pela mesma rajada.
        /// </summary>
        private static void Release(ulong guildId, ulong userId) => s_recent.TryRemove((guildId, userId), out _);
    }
}
