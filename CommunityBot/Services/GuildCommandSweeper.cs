using System;
using System.Collections.Concurrent;
using System.Linq;
using System.Threading.Tasks;
using DisCatSharp;
using DisCatSharp.Entities;
using DisCatSharp.EventArgs;
using DisCatSharp.Exceptions;

namespace CommunityBot.Services
{
    /// <summary>
    /// Cuida do ESCOPO dos comandos agora que o bot deixou de viver num servidor
    /// so.
    ///
    /// O registro global e o modo normal: o Discord anexa os comandos a qualquer
    /// servidor onde a aplicacao for instalada com o escopo
    /// <c>applications.commands</c>, inclusive nos que ainda nao existem. Nao ha
    /// nada a fazer por servidor - e justamente por isso o que sobra do modo
    /// antigo precisa ser recolhido.
    ///
    /// O QUE SOBRA. Comando de guild e comando global sao dois registros
    /// separados do lado do Discord: o UpdateAsync do DisCatSharp so sobrescreve
    /// os alvos que mandaram registrar, entao um servidor que esteve em
    /// <c>guildIds</c> continua com a copia de guild de pe depois da troca. E ela
    /// nao e uma segunda copia que funciona: o despacho casa a interacao pelo ID
    /// do comando, e o id daquela copia foi criado por um processo anterior. Quem
    /// escolhe a errada no menu recebe "a aplicacao nao respondeu".
    ///
    /// Por isso a varredura nao e arrumacao - e o que faz metade do menu daquele
    /// servidor voltar a funcionar.
    /// </summary>
    internal static class GuildCommandSweeper
    {
        /// <summary>
        /// Em que modo esta subida registrou, latcheado no <c>Program.Main</c> a
        /// partir do <c>guildIds</c>.
        ///
        /// Comeca em <c>false</c> de proposito. O modo de errar aqui e varrer em
        /// modo de desenvolvimento, apagando os comandos de guild que o
        /// DisCatSharp acabou de registrar e deixando o menu vazio; o modo de
        /// errar ao contrario e so deixar de limpar uma sobra. Se algum dia a
        /// chamada de <see cref="UseGlobalMode"/> sumir do Program, o padrao
        /// seguro e nao mexer em nada.
        /// </summary>
        private static bool s_globalMode;

        public static void UseGlobalMode(bool global) => s_globalMode = global;

        /// <summary>
        /// Servidores ja varridos NESTE processo.
        ///
        /// Em memoria, e nao no guild-settings.json, porque o <see cref="AppPaths"/>
        /// resolve <c>data/</c> a partir do diretorio do EXECUTAVEL: o
        /// `dotnet run` grava em <c>bin/Debug</c> e o servico le
        /// <c>bin/Release</c>. Uma marca persistida seria escrita pela subida de
        /// desenvolvimento no arquivo ERRADO, e a subida de producao seguinte - a
        /// unica que pode varrer - continuaria vendo "ja varri". A marca ficaria
        /// inerte exatamente na travessia para a qual ela existe.
        ///
        /// Por servidor, e nao um booleano so, porque um servidor pode chegar
        /// DEPOIS: o <c>GuildCreated</c> pega quem entra com o bot no ar, mas quem
        /// foi convidado com o bot fora chega no READY ja no cache e dispara
        /// <c>GuildAvailable</c>. A varredura de subida e a unica coisa que pega
        /// esse caso, entao ela nao pode ser desligada de vez.
        ///
        /// O custo de nao persistir e uma consulta barata por servidor por
        /// processo. Reinicio de servico e raro; pular uma varredura necessaria e
        /// silencioso e permanente.
        /// </summary>
        private static readonly ConcurrentDictionary<ulong, byte> s_swept = new();

        /// <summary>
        /// Handler do evento. Devolve NA HORA e faz o trabalho em segundo plano.
        ///
        /// A varredura e uma consulta REST por servidor, e o despachante do
        /// DisCatSharp espera o handler terminar - no mesmo laco que processa os
        /// ACKs de heartbeat. Segurando-o, a primeira subida ja rendia um "An
        /// event handler for GUILD_DOWNLOAD_COMPLETED took too long to execute", e
        /// com servidor suficiente o caminho seguinte e o Zombied. Mesmo desenho
        /// do AutoSoftban.OnMessageCreated e do HandleComponentInteraction.
        /// </summary>
        public static Task SweepAsync(DiscordClient client, GuildDownloadCompletedEventArgs e)
        {
            // e.Guilds, e nao client.Guilds: e o conjunto que ACABOU de baixar. O
            // cache vivo pode ganhar e perder servidor no meio da varredura.
            var guildIds = e.Guilds.Keys.ToArray();

            _ = Task.Run(() => SweepManyAsync(client, guildIds));
            return Task.CompletedTask;
        }

        private static async Task SweepManyAsync(DiscordClient client, ulong[] guildIds)
        {
            try
            {
                if (!s_globalMode)
                {
                    await ReportGlobalLeftoversAsync(client);
                    return;
                }

                var removed = 0;

                foreach (var guildId in guildIds)
                    if (await SweepOneAsync(client, guildId) == SweepResult.Removed)
                        removed++;

                if (removed > 0)
                    Console.WriteLine($"[comandos] varredura concluida em {removed} servidor(es).");
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[comandos] falha ao varrer comandos de servidor: {ex.Message}");
            }
        }

        /// <summary>
        /// Entrada em servidor novo. E aqui que "todo servidor novo que ele
        /// entrar" fica VISIVEL: sem esta linha, entrar num servidor e nao
        /// funcionar nele produzem o mesmo silencio.
        ///
        /// O <c>GuildCreated</c> cobre quem entra com o bot no ar. Quem convidou o
        /// bot enquanto ele estava fora chega pelo READY e dispara
        /// <c>GuildAvailable</c> - esse caso e da varredura de subida.
        /// </summary>
        public static Task OnGuildCreatedAsync(DiscordClient client, GuildCreateEventArgs e)
        {
            var guild = e.Guild;

            // MemberCount e `int?`, e Nullable<T>.ToString() devolve STRING VAZIA
            // quando nao ha valor - nunca null. Sem o `?.`, a linha sairia como
            // ", membro(s)". Mesma armadilha documentada no /serverinfo.
            var members = guild.MemberCount?.ToString() ?? "?";

            Console.WriteLine($"[servidor] entrei em \"{guild.Name}\" ({guild.Id}), {members} membro(s). " +
                              (s_globalMode
                                  ? "Se o convite trouxe o escopo applications.commands, os comandos " +
                                    "globais ja valem aqui, e quem tem Gerenciar Servidor liga o resto " +
                                    "com /config."
                                  : "AVISO: esta subida registrou comandos POR SERVIDOR (guildIds " +
                                    "preenchido), entao aqui nao ha comando nenhum - nem /config."));

            if (!s_globalMode)
                return Task.CompletedTask;

            // Em segundo plano pelo mesmo motivo do SweepAsync.
            //
            // O bot pode ja ter estado aqui antes, na epoca do registro por
            // servidor, e ter deixado a copia de guild para tras. Uma consulta so,
            // e so quando entra - nunca no caminho de uma mensagem.
            _ = Task.Run(() => SweepOneAsync(client, guild.Id));
            return Task.CompletedTask;
        }

        private enum SweepResult
        {
            Clean,
            Removed,
            Failed
        }

        private static async Task<SweepResult> SweepOneAsync(DiscordClient client, ulong guildId)
        {
            // Uma varredura por servidor por processo. O GuildDownloadCompleted
            // re-dispara a cada reconexao; sem isto, cada queda de socket custaria
            // uma consulta por servidor.
            if (!s_swept.TryAdd(guildId, 0))
                return SweepResult.Clean;

            // Este metodo APAGA todos os comandos de guild do servidor. Isso so e
            // seguro porque, em modo global, o RegisterAll do Program nao registra
            // nenhum comando de guild - logo tudo o que estiver la e sobra. Se
            // alguem um dia misturar os dois registros, esta linha passa a apagar
            // o que a biblioteca acabou de criar.
            if (!s_globalMode)
                return SweepResult.Clean;

            try
            {
                // Consulta antes de apagar: o DELETE e barato, mas um por servidor
                // a cada processo numa instalacao ja limpa gastaria rate limit a
                // toa e sujaria o log com uma linha que nao aconteceu.
                var existing = await client.GetGuildApplicationCommandsAsync(guildId, false);
                if (existing is null || existing.Count == 0)
                    return SweepResult.Clean;

                await client.RemoveGuildApplicationCommandsAsync(guildId);

                // Nomeia o que foi apagado: e uma remocao irreversivel e de uma vez
                // so, e um numero solto nao deixa ninguem conferir depois pelo /logs.
                var names = string.Join(" ", existing.Select(c => "/" + c.Name));
                Console.WriteLine($"[comandos] removi {existing.Count} comando(s) de servidor que sobravam " +
                                  $"em {NameOf(client, guildId)}; os globais assumem no lugar: {names}");
                return SweepResult.Removed;
            }
            catch (UnauthorizedException)
            {
                // Sem o escopo applications.commands neste servidor nao existe
                // comando de guild para apagar. Nao e falha: e a resposta "nao ha
                // nada aqui" dita de outro jeito. Tratar como falha deixaria este
                // servidor pedindo varredura para sempre, com um AVISO por subida.
                return SweepResult.Clean;
            }
            catch (Exception ex)
            {
                // Solta a marca: a varredura so vale como feita quando terminou. O
                // servidor que falhou tenta de novo na proxima RECONEXAO, sem
                // esperar por um reinicio.
                s_swept.TryRemove(guildId, out _);

                Console.WriteLine($"[comandos] AVISO: nao consegui varrer {NameOf(client, guildId)}: " +
                                  ex.Message);
                return SweepResult.Failed;
            }
        }

        private static string NameOf(DiscordClient client, ulong guildId) =>
            client.Guilds.TryGetValue(guildId, out var guild)
                ? $"\"{guild.Name}\" ({guildId})"
                : guildId.ToString();

        /// <summary>
        /// O risco espelhado: subir em modo de desenvolvimento com os comandos
        /// globais ainda de pe tambem duplica tudo nos servidores da lista.
        ///
        /// Aqui e AVISO, nao remocao - e nao deve virar remocao nem atras de uma
        /// flag. Ha um token so e uma aplicacao so: um atalho daqui para "todo
        /// servidor de producao fica sem comando" nao pode existir no binario.
        /// </summary>
        private static async Task ReportGlobalLeftoversAsync(DiscordClient client)
        {
            try
            {
                var globals = await client.GetGlobalApplicationCommandsAsync(false);
                if (globals is null || globals.Count == 0)
                    return;

                Console.WriteLine($"[comandos] AVISO: {globals.Count} comando(s) GLOBAIS continuam " +
                                  "registrados, e esta subida registrou por servidor. Nos servidores de " +
                                  "guildIds cada comando aparece DUAS vezes, e a copia global responde " +
                                  "\"a aplicacao nao respondeu\" - este processo so conhece os ids dos de " +
                                  "guild. Nao apago os globais daqui: isso tiraria os comandos de todos " +
                                  "os outros servidores. Para voltar ao normal, esvazie guildIds e suba " +
                                  "uma vez. Para nao viver com isso, use uma aplicacao e um token " +
                                  "separados no desenvolvimento.");
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[comandos] nao consegui conferir os comandos globais: {ex.Message}");
            }
        }
    }
}
