using System;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using CommunityBot.commands;
using CommunityBot.config;
using CommunityBot.Services;
using DisCatSharp;
using DisCatSharp.ApplicationCommands;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.EventArgs;
using Newtonsoft.Json;
using DisCatSharp.Interactivity;
using DisCatSharp.Interactivity.Enums;
using DisCatSharp.Interactivity.Extensions;
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
                // hierarquia. MessageContent NAO entra: tudo aqui e slash command,
                // e pedir um intent privilegiado a toa e superficie de risco de
                // graca.
                Intents = DiscordIntents.Guilds | DiscordIntents.GuildMembers,
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

                // Uma checagem que recusou o comando nao e falha: e o
                // comportamento pretendido, e o proprio DisCatSharp ja
                // respondeu. Avisar de novo daria erro de "ja respondido".
                if (e.Exception is DisCatSharp.ApplicationCommands.Exceptions.SlashExecutionChecksFailedException)
                    return;

                try
                {
                    // O comando quase sempre ja deferiu; por isso edita a
                    // resposta original em vez de criar uma nova.
                    await e.Context!.EditResponseAsync(new DiscordWebhookBuilder()
                        .AddEmbed(Embeds.Error("Falha no comando",
                            "Algo quebrou ao executar. O erro foi registrado; avise quem administra o bot.")));
                }
                catch (Exception inner)
                {
                    Console.WriteLine($"[erro] nao consegui avisar o usuario sobre a falha de /{name}: {inner.Message}");
                }
            };

            var guildIds = config.guildIds ?? Array.Empty<ulong>();
            if (guildIds.Length == 0)
            {
                // Global propaga em ate uma hora; de guild aparece na hora. Por
                // isso guildIds existe no config - durante o desenvolvimento
                // esperar uma hora por comando nao e viavel.
                Console.WriteLine("Registrando comandos globalmente (pode levar ate uma hora para aparecer).");
                RegisterAll(slashCommands, null);
            }
            else
            {
                foreach (var gid in guildIds)
                {
                    Console.WriteLine($"Registrando comandos no servidor {gid}.");
                    RegisterAll(slashCommands, gid);
                }
            }

            StartThreadPoolCanary();

            await Client.ConnectAsync();
            await Task.Delay(-1);
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
                return;
            }

            slash.RegisterGuildCommands<Moderation>(guildId.Value);
            slash.RegisterGuildCommands<Warnings>(guildId.Value);
            slash.RegisterGuildCommands<Fun>(guildId.Value);
            slash.RegisterGuildCommands<Utility>(guildId.Value);
            slash.RegisterGuildCommands<BotLogs>(guildId.Value);
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
                            .WithContent("Você não rodou esse comando para poder fazer essa ação")
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
            var last = DateTimeOffset.UtcNow;

            s_threadPoolCanary = new Timer(_ =>
            {
                var now = DateTimeOffset.UtcNow;
                var drift = now - last - TimeSpan.FromSeconds(10);
                last = now;

                if (drift > TimeSpan.FromSeconds(2))
                    Console.WriteLine($"[canary] thread pool atrasou {drift.TotalSeconds:0.0}s");
            }, null, TimeSpan.FromSeconds(10), TimeSpan.FromSeconds(10));
        }
    }
}
