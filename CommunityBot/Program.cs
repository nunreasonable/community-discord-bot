using System;
using System.IO;
using System.Collections.Generic;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using CommunityBot.commands;
using CommunityBot.config;
using CommunityBot.Services;
using CommunityBot.Services.Economy;
using CommunityBot.Services.Fun;
using CommunityBot.Services.Levels;
using CommunityBot.Services.Music;
using CommunityBot.Services.Roblox;
using DisCatSharp;
using DisCatSharp.ApplicationCommands;
using DisCatSharp.ApplicationCommands.Attributes;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.EventArgs;
using Newtonsoft.Json;
using DisCatSharp.Interactivity;
using DisCatSharp.Interactivity.Enums;
using DisCatSharp.Interactivity.Extensions;
using DisCatSharp.Voice;
using Microsoft.Extensions.Logging;

namespace CommunityBot
{
    internal class Program
    {
        private static DiscordClient? Client { get; set; }

        static async Task Main(string[] args)
        {
            // Primeira linha do processo de proposito: o tee so captura o que for
            // escrito DEPOIS dele, e os handlers globais abaixo ja logam.
            ConsoleTee.Install();

            InstallGlobalExceptionHandlers();

            var config = new JSONReader();
            try
            {
                await config.ReadJSON();
            }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or JsonException)
            {
                // A mensagem amigavel logo abaixo era codigo morto justamente
                // para o caso que ela descreve: sem o arquivo, o ReadJSON
                // lancava FileNotFoundException antes de chegar la. E com
                // Restart=on-failure no systemd isso virava um laco de crash a
                // cada 10 segundos, sem nada explicando o motivo.
                Console.WriteLine("[fatal] Nao consegui ler config/config.jsonc: " + ex.Message);
                Console.WriteLine("        Copie config/config.example.jsonc para config/config.jsonc " +
                                  "e preencha o campo \"token\".");
                return;
            }

            if (string.IsNullOrWhiteSpace(config.token))
            {
                Console.WriteLine("[fatal] Token vazio. Copie config/config.example.jsonc para " +
                                  "config/config.jsonc e preencha o campo \"token\".");
                return;
            }

            var discordConfig = new DiscordConfiguration
            {
                // So o necessario. GuildMembers e privilegiado e precisa ser ligado
                // no portal - e o que permite ler cargos para a checagem de
                // hierarquia. GuildMessages NAO e privilegiado e nao pede nada no
                // portal; ele existe aqui so para o vigia de canal do AutoSoftban
                // receber o evento de mensagem.
                //
                // MessageContent continua de fora. O vigia olha QUEM escreveu e
                // ONDE, nunca o texto - sem esse intent o Content chega vazio, e
                // aqui isso nao faz falta nenhuma. Pedir um intent privilegiado a
                // toa e superficie de risco de graca.
                //
                // GuildVoiceStates e da musica, e tambem nao e privilegiado: sem
                // ele a biblioteca de voz nunca recebe o VOICE_STATE_UPDATE do
                // proprio bot e o ConnectAsync fica esperando para sempre - e o
                // bot nao saberia quem esta no canal para a votacao do /skip.
                Intents = DiscordIntents.Guilds | DiscordIntents.GuildMembers | DiscordIntents.GuildMessages |
                          DiscordIntents.GuildVoiceStates,
                Token = config.token,
                TokenType = TokenType.Bot,
                AutoReconnect = true,
                MinimumLogLevel = LogLevel.Information,
                DisableUpdateCheck = true,
                EnableSentry = false
            };

            Client = new DiscordClient(discordConfig);

            HookGatewayDiagnostics(Client);

            // Registrado ANTES do UseInteractivity: o despachante chama os
            // handlers na ordem de inscricao e para no primeiro que marcar
            // "Handled", e o paginador marca Handled ao confirmar o clique.
            // Registrando antes, da para barrar o clique de quem nao e dono da
            // mensagem e responder a ele.
            Client.ComponentInteractionCreated += HandleComponentInteraction;

            // Vigia de canal. Inerte enquanto autoSoftbanGuildId e
            // autoSoftbanChannelId nao estiverem no config.
            Client.MessageCreated += AutoSoftban.OnMessageCreated;

            // XP por mensagem, nos servidores que ligaram /config leveling. Olha
            // so autor, servidor e tipo da mensagem - o texto nem chega, sem o
            // intent de Message Content. Inerte onde o nivel esta desligado.
            Client.MessageCreated += LevelService.OnMessageCreated;

            // Diz na subida se o vigia esta ligado e, se nao estiver funcionando,
            // por que. Sem isto, canal errado, canal invisivel e canal de forum
            // produzem o mesmo silencio que "ninguem escreveu la".
            Client.GuildDownloadCompleted += AutoSoftban.ReportStatusAsync;

            // Botoes e modais dos tickets. Handler PROPRIO, e nao um ramo dentro
            // do HandleComponentInteraction: aquele volta cedo em ModalSubmit, e
            // o fluxo de ticket depende justamente de modal. Os dois convivem
            // porque nenhum marca Handled num id que nao e seu.
            Client.ComponentInteractionCreated += TicketComponents.OnComponent;
            Client.GuildDownloadCompleted += TicketService.ReportStatusAsync;

            // Botoes da verificacao Roblox (painel, "I've authorized", Unlink).
            // Mesmo esquema dos tickets: handler proprio, Handled so para
            // custom id "verify:".
            Client.ComponentInteractionCreated += VerificationFlow.OnComponent;

            // Botoes do "Now playing" da musica. Mesmo esquema: Handled so para
            // custom id "music:".
            Client.ComponentInteractionCreated += MusicComponents.OnComponent;

            // Botoes dos comandos fun: o "Return" do /roleplay e o tabuleiro do
            // /tictactoe. Handled so para "rp:" e "ttt:".
            Client.ComponentInteractionCreated += FunComponents.OnComponent;

            // Yes/No da DM de level up ("lvl:") e confirmacao do /pay ("eco:").
            Client.ComponentInteractionCreated += LevelService.OnComponent;
            Client.ComponentInteractionCreated += EconomyComponents.OnComponent;

            // Bot expulso do canal de voz e canal que esvazia. Nao ha evento de
            // desconexao publico na biblioteca de voz; e por aqui que se sabe.
            Client.VoiceStateUpdated += MusicService.OnVoiceStateUpdated;

            // Quem entra ja vinculado recebe cargos e apelido na hora; quem entra
            // sem vinculo recebe o cargo de nao verificado, se houver. Inerte em
            // servidor que nao configurou a verificacao. O intent GuildMembers,
            // que entrega este evento, ja estava ligado pela hierarquia.
            Client.GuildMemberAdded += VerificationService.OnMemberAddedAsync;

            // Traz as chaves antigas do config.jsonc para a configuracao por
            // servidor, uma vez so. Roda aqui, e nao antes de conectar, porque
            // precisa do cache de servidores para descobrir a que servidor cada
            // id solto pertence.
            Client.GuildDownloadCompleted += LegacyConfigMigration.RunAsync;

            // Diz na subida se a musica esta pronta (yt-dlp e ffmpeg achados) ou
            // o que falta. Roda os processos fora do caminho do gateway.
            Client.GuildDownloadCompleted += MusicService.ReportStatusAsync;

            // Feed de commits de um repositorio local para um canal. Liga na
            // primeira carga dos servidores (o canal precisa estar no cache) e
            // fica inerte sem a secao "gitFeed" no config.
            Client.GuildDownloadCompleted += GitFeed.StartAsync;

            // Recolhe os comandos de guild que sobraram da epoca em que o bot
            // vivia num servidor so. Sem isto, o servidor que estava em guildIds
            // mostra cada comando DUAS vezes depois da troca para global - e a
            // copia velha nem responde, porque o id dela e de outro processo.
            //
            // Inscrito por ULTIMO entre os handlers deste evento, de proposito: o
            // despachante os chama em ordem, e o relatorio do vigia e a
            // reconciliacao de tickets nao devem esperar por consulta REST. (O
            // proprio sweeper ja sai do caminho do gateway, mas a ordem e de
            // graca e nao depende disso continuar verdade.)
            Client.GuildDownloadCompleted += GuildCommandSweeper.SweepAsync;

            // Entrada em servidor novo. Vira uma linha no log e varre a sobra de
            // uma passagem anterior pelo mesmo servidor.
            Client.GuildCreated += GuildCommandSweeper.OnGuildCreatedAsync;

            Client.UseInteractivity(new InteractivityConfiguration
            {
                Timeout = TimeSpan.FromMinutes(3),

                // Sem o ACK a paginacao quebra: ao clicar numa seta o paginador
                // guarda a interacao do BOTAO como "ultima interacao" e edita a
                // resposta original dela, que sem o defer nao existe - o Discord
                // acaba mostrando "interacao falhou".
                AckPaginationButtons = true,

                // Ao expirar, apenas desabilita os botoes em vez de apagar a
                // mensagem: o conteudo continua legivel.
                ButtonBehavior = ButtonPaginationBehavior.Disable
            });

            // Voz para a musica. So envio: EnableIncoming fica desligado, entao o
            // bot nao decodifica - nem recebe - o audio de ninguem no canal.
            //
            // O DAVE (E2EE de voz, obrigatorio no Discord desde marco de 2026)
            // vem ligado pelo padrao da biblioteca; a libdave chega pelo pacote
            // DisCatSharp.Voice.Natives. Se ela faltar, a biblioteca loga "DAVE
            // disabled" e o Discord derruba a conexao com 4017.
            Client.UseVoice(new VoiceConfiguration
            {
                EnableIncoming = false
            });

            Client.Ready += (s, e) =>
            {
                Console.WriteLine($"Bot pronto como {s.CurrentUser.UsernameWithDiscriminator}.");
                return Task.CompletedTask;
            };

            var slashCommands = Client.UseApplicationCommands(new ApplicationCommandsConfiguration());

            // Sem este handler, toda excecao lancada DEPOIS do defer efemero
            // sumia: o DisCatSharp a entrega no SlashCommandErrored, e ninguem
            // estava inscrito. Na pratica o usuario ficava no "pensando..."
            // para sempre e nada chegava ao BotLogBuffer - entao nem o /logs
            // conseguia diagnosticar. Isso mascarava todo o resto.
            slashCommands.SlashCommandErrored += async (s, e) =>
            {
                var name = e.Context?.CommandName ?? "(desconhecido)";
                var who = e.Context?.User?.Id;
                Console.WriteLine($"[erro] /{name} de {who} falhou: {e.Exception}");

                /*
                 * Uma checagem que recusou o comando nao e falha - e o
                 * comportamento pretendido. So que apenas UMA delas responde ao
                 * usuario sozinha: o SlashCommandCooldown, que manda um
                 * "Ratelimit hit" efemero. As checagens de permissao
                 * (RequireBotPermissions, RequireUserPermissions, RequireGuild)
                 * lancam sem responder nada.
                 *
                 * Com o `return` valendo para todas, tirar Ban Members do cargo
                 * do bot fazia o /ban e o /softban morrerem em "A aplicacao nao
                 * respondeu" - a checagem falha ANTES do corpo do comando, entao
                 * nem o defer chegou a acontecer, e ninguem dizia ao moderador o
                 * que estava errado.
                 */
                if (e.Exception is DisCatSharp.ApplicationCommands.Exceptions.SlashExecutionChecksFailedException checks)
                {
                    /*
                     * O DisCatSharp roda TODAS as checagens e junta todas as
                     * falhas - nao para na primeira. E o cooldown responde a
                     * interacao por conta propria antes de reprovar.
                     *
                     * Entao quando o cooldown falha JUNTO com outra checagem, a
                     * interacao ja foi respondida, e um CreateResponseAsync aqui
                     * levava 400/40060. O usuario via so o "Ratelimit hit" da
                     * biblioteca e nunca ficava sabendo que faltava permissao ao
                     * bot - e o log ganhava um ERRO falso justamente no filtro que
                     * se usa durante um incidente.
                     */
                    var explain = checks.FailedChecks?
                        .Where(c => c is not SlashCommandCooldownAttribute)
                        .ToList() ?? new List<ApplicationCommandCheckBaseAttribute>();

                    if (explain.Count == 0)
                        return;

                    // Sobrou alguma alem do cooldown, e havia cooldown na lista:
                    // logo o cooldown ja respondeu e so cabe follow-up.
                    var alreadyAnswered = explain.Count != checks.FailedChecks!.Count;

                    try
                    {
                        var embed = Embeds.Error("Command denied", DescribeFailedChecks(explain));

                        if (alreadyAnswered)
                        {
                            await e.Context!.FollowUpAsync(
                                new DiscordFollowupMessageBuilder().AddEmbed(embed).AsEphemeral());
                        }
                        else
                        {
                            // Nada foi deferido: e preciso CRIAR a resposta.
                            await e.Context!.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                                new DiscordInteractionResponseBuilder().AddEmbed(embed).AsEphemeral());
                        }
                    }
                    catch (Exception inner)
                    {
                        Console.WriteLine($"[erro] nao consegui explicar a recusa de /{name}: {inner.Message}");
                    }

                    return;
                }

                try
                {
                    // O comando quase sempre ja deferiu; por isso edita a
                    // resposta original em vez de criar uma nova.
                    await e.Context!.EditResponseAsync(new DiscordWebhookBuilder()
                        .AddEmbed(Embeds.Error("Command failed",
                            "Something went wrong while running this command. The error was logged; let the bot's admin know.")));
                }
                catch (Exception inner)
                {
                    Console.WriteLine($"[erro] nao consegui avisar o usuario sobre a falha de /{name}: {inner.Message}");
                }
            };

            var guildIds = config.guildIds ?? Array.Empty<ulong>();

            // Latcheado AQUI, uma vez, e nao relido dentro do sweeper. O
            // JSONReader recarrega o config quando o arquivo muda, entao ler
            // guildIds de novo mais tarde poderia ver uma edicao feita com o bot
            // no ar - e o sweeper apagaria comandos de guild que ESTE processo
            // esta servindo. O que vale para ele e o modo em que a subida
            // registrou, nao o que o arquivo diz agora.
            GuildCommandSweeper.UseGlobalMode(guildIds.Length == 0);

            if (guildIds.Length == 0)
            {
                // MODO NORMAL. Global vale em todo servidor onde a aplicacao for
                // instalada com o escopo applications.commands - inclusive nos
                // que ainda nao existem -, entao entrar num servidor novo nao
                // pede passo nenhum.
                //
                // A espera de ate uma hora que este aviso prometia e de ALTERAR a
                // definicao de um comando, nao de um servidor novo enxergar os
                // que ja existem. Escrito como estava, dava a entender que o bot
                // ficaria mudo por uma hora em cada servidor novo.
                Console.WriteLine("Registrando comandos globalmente: valem em todo servidor, " +
                                  "inclusive nos que o bot entrar depois. " +
                                  "Mudanca na definicao de um comando pode levar ate uma hora para propagar.");
                RegisterAll(slashCommands, null);
            }
            else
            {
                // MODO DE DESENVOLVIMENTO. Comando de guild aparece na hora, o
                // que torna a iteracao viavel - mas o bot passa a NAO ter comando
                // em nenhum outro servidor, os novos inclusive. O
                // GuildCommandSweeper avisa no log se ainda houver comandos
                // globais de pe, porque ai os dois conjuntos se somam.
                //
                // Distinct: um id repetido no config registrava o mesmo servidor
                // duas vezes, mandando definicoes duplicadas no mesmo payload.
                foreach (var gid in guildIds.Distinct())
                {
                    Console.WriteLine($"Registrando comandos no servidor {gid}.");
                    RegisterAll(slashCommands, gid);
                }
            }

            StartThreadPoolCanary();

            // Carrega a configuracao dos servidores ANTES de conectar. Sem isto o
            // primeiro lote de mensagens depois do connect chegaria antes de o
            // snapshot existir, e uma mensagem na armadilha nesse instante
            // passaria batido.
            await GuildSettingsStore.Instance.LoadAsync();

            // Pelo mesmo motivo: a entrada de membro consulta o vinculo pelo
            // snapshot, e uma entrada logo depois do connect nao pode achar a
            // lista vazia.
            await RobloxLinkStore.Instance.LoadAsync();

            // XP e carteiras. Antes de conectar pelo mesmo motivo: a primeira
            // mensagem depois do connect ja soma XP. Arquivo corrompido derruba a
            // subida de proposito, como nos outros armazenamentos - subir com
            // "ninguem tem nada" e o proximo flush apagariam tudo.
            await LevelStore.Instance.LoadAsync();
            await EconomyStore.Instance.LoadAsync();

            // Audio que sobrou de uma execucao que morreu no meio de uma faixa.
            MusicService.CleanTempRoot();

            // Shutdown gracioso. A unit do systemd usa KillSignal=SIGINT, e antes
            // o Task.Delay(-1) so era interrompido pela morte do processo: sem
            // DisconnectAsync (o gateway ficava pendurado do lado do Discord) e
            // sem flush do ConsoleTee (a ultima linha sem \n sumia do buffer).
            using var shutdown = new CancellationTokenSource();
            // O PRIMEIRO sinal e capturado para o encerramento limpo; o segundo
            // passa direto. Cancelando os dois, um DisconnectAsync travado deixava
            // o operador sem saida a nao ser SIGKILL - no systemd o
            // TimeoutStopSec=30 resolve, mas num `dotnet run` interativo o Ctrl-C
            // simplesmente parava de responder.
            var signalled = 0;
            void OnSignal(System.Runtime.InteropServices.PosixSignalContext ctx)
            {
                if (Interlocked.Exchange(ref signalled, 1) == 1)
                    return;

                ctx.Cancel = true;
                shutdown.Cancel();
            }

            using var sigint = System.Runtime.InteropServices.PosixSignalRegistration.Create(
                System.Runtime.InteropServices.PosixSignal.SIGINT, OnSignal);
            using var sigterm = System.Runtime.InteropServices.PosixSignalRegistration.Create(
                System.Runtime.InteropServices.PosixSignal.SIGTERM, OnSignal);

            try
            {
                await Client.ConnectAsync();
            }
            catch (Exception ex)
            {
                // Sem isto, um token revogado sobe pelo Main, cai no
                // UnhandledException e o processo sai com erro - e o
                // Restart=on-failure da unit reergue tudo a cada 10 segundos, num
                // laco de IDENTIFY falho que o Discord acaba limitando. E o mesmo
                // laco que a leitura de config ja aprendeu a evitar.
                Console.WriteLine("[fatal] Nao consegui conectar ao gateway: " + ex.Message);
                Console.WriteLine("        Se for 401, o token em config/config.jsonc foi revogado ou esta errado.");
                Console.Out.Flush();
                return;
            }

            try
            {
                await Task.Delay(Timeout.Infinite, shutdown.Token);
            }
            catch (OperationCanceledException)
            {
                // Sinal recebido: segue para o encerramento limpo abaixo.
            }

            // Espera o que estiver em voo ANTES de largar o gateway. Um softban
            // com o ban aplicado e o unban ainda pendente vira banimento
            // permanente se o processo morrer no meio, e sem nenhuma linha de log
            // dizendo isso. O TimeoutStopSec=30 da unit cobre esta espera.
            await AutoSoftban.DrainAsync(TimeSpan.FromSeconds(10));

            // Sai dos canais de voz e apaga o audio baixado antes de largar o
            // gateway. Cabe no TimeoutStopSec=30 junto com a espera de cima.
            await MusicService.DrainAsync(TimeSpan.FromSeconds(8));

            // Para a varredura do feed de commits. Um post pela metade so faz o
            // mesmo commit sair de novo na proxima subida - o ultimo postado so
            // e gravado depois do envio.
            GitFeed.Stop();

            Console.WriteLine("[shutdown] sinal recebido; desconectando do gateway...");
            try
            {
                await Client.DisconnectAsync();
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[shutdown] falha ao desconectar: {ex.Message}");
            }

            // O XP vai para o disco a cada 30s; aqui sai o que faltava. Depois do
            // DisconnectAsync, quando nenhuma mensagem nova chega para mudar o
            // XP no meio da gravacao. (A economia grava a cada mudanca.)
            LevelStore.Instance.StopTimer();
            try
            {
                await LevelStore.Instance.FlushAsync();
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[shutdown] falha ao gravar o XP: {ex.Message}");
            }

            // Drena a ultima linha parcial do tee antes de sair.
            Console.Out.Flush();
        }

        /// <summary>
        /// Traduz a checagem que barrou o comando para uma frase util. Sem isso a
        /// recusa chegaria como um "algo deu errado" generico, que nao diz a
        /// quem usou nem se o problema e dele ou do bot.
        /// </summary>
        private static string DescribeFailedChecks(IEnumerable<ApplicationCommandCheckBaseAttribute> failed)
        {
            var reasons = new List<string>();

            foreach (var check in failed)
            {
                switch (check)
                {
                    case ApplicationCommandRequireBotPermissionsAttribute:
                        reasons.Add("**I** don't have the permission this needs in this server — check the bot's role");
                        break;
                    case ApplicationCommandRequireUserPermissionsAttribute:
                        reasons.Add("you don't have the permission this command requires");
                        break;
                    case ApplicationCommandRequireGuildAttribute:
                        reasons.Add("this command only works inside a server");
                        break;
                }
            }

            if (reasons.Count == 0)
                reasons.Add("a permission check blocked this command");

            return string.Join("\n", reasons.Distinct().Select(r => $"• {r}"));
        }

        /// <summary>
        /// Um lugar so para a lista de modulos: esquecer de registrar um modulo e
        /// o jeito mais facil de um comando novo simplesmente nao aparecer.
        /// </summary>
        private static void RegisterAll(ApplicationCommandsExtension slash, ulong? guildId)
        {
            if (guildId is null)
            {
                slash.RegisterGlobalCommands<Moderation>();
                slash.RegisterGlobalCommands<Warnings>();
                slash.RegisterGlobalCommands<Fun>();
                slash.RegisterGlobalCommands<Utility>();
                slash.RegisterGlobalCommands<BotLogs>();
                slash.RegisterGlobalCommands<commands.Tickets>();
                slash.RegisterGlobalCommands<commands.Config>();
                slash.RegisterGlobalCommands<Verification>();
                slash.RegisterGlobalCommands<Binds>();
                slash.RegisterGlobalCommands<Music>();
                slash.RegisterGlobalCommands<Roleplay>();
                slash.RegisterGlobalCommands<TextCommands>();
                slash.RegisterGlobalCommands<MorseCommands>();
                slash.RegisterGlobalCommands<Games>();
                slash.RegisterGlobalCommands<Levels>();
                slash.RegisterGlobalCommands<XpCommands>();
                slash.RegisterGlobalCommands<Economy>();
                slash.RegisterGlobalCommands<Profile>();
                return;
            }

            slash.RegisterGuildCommands<Moderation>(guildId.Value);
            slash.RegisterGuildCommands<Warnings>(guildId.Value);
            slash.RegisterGuildCommands<Fun>(guildId.Value);
            slash.RegisterGuildCommands<Utility>(guildId.Value);
            slash.RegisterGuildCommands<BotLogs>(guildId.Value);
            slash.RegisterGuildCommands<commands.Tickets>(guildId.Value);
            slash.RegisterGuildCommands<commands.Config>(guildId.Value);
            slash.RegisterGuildCommands<Verification>(guildId.Value);
            slash.RegisterGuildCommands<Binds>(guildId.Value);
            slash.RegisterGuildCommands<Music>(guildId.Value);
            slash.RegisterGuildCommands<Roleplay>(guildId.Value);
            slash.RegisterGuildCommands<TextCommands>(guildId.Value);
            slash.RegisterGuildCommands<MorseCommands>(guildId.Value);
            slash.RegisterGuildCommands<Games>(guildId.Value);
            slash.RegisterGuildCommands<Levels>(guildId.Value);
            slash.RegisterGuildCommands<XpCommands>(guildId.Value);
            slash.RegisterGuildCommands<Economy>(guildId.Value);
            slash.RegisterGuildCommands<Profile>(guildId.Value);
        }

        /// <summary>
        /// Redes de seguranca globais: sem isso, uma excecao em `async void` ou em
        /// uma Task esquecida derruba o processo sem deixar rastro no log.
        /// </summary>
        private static void InstallGlobalExceptionHandlers()
        {
            TaskScheduler.UnobservedTaskException += (s, e) =>
            {
                Console.WriteLine($"[unobserved] {e.Exception}");
                e.SetObserved();
            };

            AppDomain.CurrentDomain.UnhandledException += (s, e) =>
                Console.WriteLine($"[fatal] {e.ExceptionObject}");
        }

        /// <summary>
        /// Eventos de diagnostico do gateway. Sao baratos e transformam "o bot
        /// morreu" em uma linha de log com hora e motivo.
        /// </summary>
        private static void HookGatewayDiagnostics(DiscordClient client)
        {
            client.SocketOpened += (s, e) =>
            {
                Console.WriteLine("[gateway] socket aberto");
                return Task.CompletedTask;
            };

            client.SocketClosed += (s, e) =>
            {
                Console.WriteLine($"[gateway] socket fechado: {e.CloseCode} {e.CloseMessage}");
                return Task.CompletedTask;
            };

            client.SocketErrored += (s, e) =>
            {
                Console.WriteLine($"[gateway] erro de socket: {e.Exception.Message}");
                return Task.CompletedTask;
            };

            client.Resumed += (s, e) =>
            {
                Console.WriteLine("[gateway] sessao retomada");
                return Task.CompletedTask;
            };

            client.Zombied += (s, e) =>
            {
                Console.WriteLine("[gateway] conexao zumbi detectada");
                return Task.CompletedTask;
            };
        }

        /// <summary>
        /// Clique de quem NAO rodou o comando numa mensagem paginada: a
        /// interatividade descartaria o clique em silencio e o Discord mostraria
        /// "interacao falhou". Aqui ele recebe um aviso efemero.
        /// </summary>
        private static Task HandleComponentInteraction(DiscordClient sender, ComponentInteractionCreateEventArgs e)
        {
            if (e.Interaction.Type == InteractionType.ModalSubmit || e.Message is null)
                return Task.CompletedTask;

            if (!PaginationOwnership.TryGetOwner(e.Message.Id, out var ownerId) || e.User.Id == ownerId)
                return Task.CompletedTask;

            // Marcado de forma sincrona: o despachante so olha Handled depois que
            // este handler retorna, e a resposta REST vai em segundo plano para
            // nao segurar o caminho do gateway.
            e.Handled = true;

            var interaction = e.Interaction;
            _ = Task.Run(async () =>
            {
                try
                {
                    await interaction.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                        new DiscordInteractionResponseBuilder()
                            .WithContent("Only the person who ran this command can use these buttons.")
                            .AsEphemeral());
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"[component] falha ao avisar clique de terceiro: {ex.Message}");
                }
            });

            return Task.CompletedTask;
        }

        private static Timer? s_threadPoolCanary;

        /// <summary>
        /// Se este timer atrasar, o thread pool esta faminto - e um bot que parece
        /// "lento sem motivo" costuma ser exatamente isso. Barato o bastante para
        /// ficar ligado sempre.
        /// </summary>
        private static void StartThreadPoolCanary()
        {
            // TickCount64, e nao o relogio de parede: um ajuste de NTP de +3s
            // produzia um "[pool] thread pool atrasou 3.0s" que nunca aconteceu -
            // classificado como AVISO e servido justamente no `/logs level:aviso`
            // que se usa quando o bot parece lento. Tempo decorrido pede relogio
            // monotonico.
            var last = Environment.TickCount64;

            s_threadPoolCanary = new Timer(_ =>
            {
                var now = Environment.TickCount64;
                var drift = TimeSpan.FromMilliseconds(now - last) - TimeSpan.FromSeconds(10);
                last = now;

                if (drift > TimeSpan.FromSeconds(2))
                    // Tag [pool] e nao [canary]: e "[pool]" que o
                    // BotLogBuffer.Classify reconhece como AVISO. Com a tag
                    // antiga, inanicao de thread pool era arquivada como INFO e
                    // ficava invisivel em `/logs level:aviso` - justamente o
                    // filtro que se usa quando o bot esta "lento sem motivo".
                    Console.WriteLine($"[pool] thread pool atrasou {drift.TotalSeconds:0.0}s");
            }, null, TimeSpan.FromSeconds(10), TimeSpan.FromSeconds(10));
        }
    }
}
