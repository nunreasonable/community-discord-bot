using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using CommunityBot.config;
using DisCatSharp;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.EventArgs;
using DisCatSharp.Voice;

namespace CommunityBot.Services.Music
{
    /// <summary>
    /// Registro dos players por servidor e tudo o que e de fora deles: entrar no
    /// canal, reagir a mudancas de voz, o relatorio de subida e o desligamento.
    /// </summary>
    internal static class MusicService
    {
        /// <summary>
        /// Pasta dos arquivos baixados. Sob o /tmp, que na unit do systemd e
        /// PRIVADO (PrivateTmp=yes): outro usuario da maquina nao ve o que o bot
        /// esta tocando, e o systemd apaga tudo quando a unit para.
        /// </summary>
        public static readonly string TempRoot = Path.Combine(Path.GetTempPath(), "sollarety-music");

        private static readonly ConcurrentDictionary<ulong, GuildPlayer> s_players = new();

        // Um /play por servidor de cada vez entre "tem player?" e "cria player".
        // Sem isto, dois /play simultaneos num servidor sem musica abriam duas
        // conexoes de voz para o mesmo canal.
        private static readonly ConcurrentDictionary<ulong, SemaphoreSlim> s_connectLocks = new();

        private static readonly TimeSpan s_probeInterval = TimeSpan.FromMinutes(1);
        private static readonly SemaphoreSlim s_probeLock = new(1, 1);
        private static string? s_unavailableReason = "ainda nao verificado";
        private static DateTimeOffset s_lastProbe = DateTimeOffset.MinValue;
        private static string? s_lastReport;

        public static GuildPlayer? Get(ulong guildId) =>
            s_players.TryGetValue(guildId, out var player) && !player.IsClosing ? player : null;

        /// <summary>So o proprio player se tira daqui, no fim do laco dele.</summary>
        public static void Unregister(GuildPlayer player) =>
            s_players.TryRemove(new KeyValuePair<ulong, GuildPlayer>(player.GuildId, player));

        public static async Task<MusicSettings> ReadSettingsAsync()
        {
            try
            {
                var reader = new JSONReader();
                await reader.ReadJSON();
                return reader.music;
            }
            catch (Exception ex)
            {
                // Config ilegivel no meio de uma edicao: valem os padroes ate a
                // proxima leitura, como na verificacao Roblox.
                Console.WriteLine($"[musica] aviso: nao consegui ler o config: {ex.Message}");
                return new MusicSettings();
            }
        }

        /// <summary>
        /// Apaga o que sobrou de uma execucao anterior. Roda antes de conectar:
        /// um processo morto no meio de uma faixa deixa o arquivo para tras.
        /// </summary>
        public static void CleanTempRoot()
        {
            try
            {
                if (Directory.Exists(TempRoot))
                    Directory.Delete(TempRoot, recursive: true);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[musica] aviso: nao consegui limpar {TempRoot}: {ex.Message}");
            }
        }

        // ------------------------------------------------------------------
        // Disponibilidade
        // ------------------------------------------------------------------

        /// <summary>
        /// Null quando a musica esta pronta; senao, o motivo (para o log). Refaz a
        /// verificacao no maximo uma vez por minuto enquanto estiver faltando
        /// algo: instalar o yt-dlp com o bot no ar passa a valer sem reiniciar.
        /// </summary>
        public static async Task<string?> CheckAvailabilityAsync(bool force = false)
        {
            var reason = Volatile.Read(ref s_unavailableReason);
            if (!force && (reason is null || DateTimeOffset.UtcNow - s_lastProbe < s_probeInterval))
                return reason;

            await s_probeLock.WaitAsync();
            try
            {
                if (!force && DateTimeOffset.UtcNow - s_lastProbe < s_probeInterval)
                    return Volatile.Read(ref s_unavailableReason);

                var settings = await ReadSettingsAsync();
                string? problem = null;
                string report;

                try
                {
                    var ytdlp = await YtDlp.ProbeAsync(settings, CancellationToken.None);
                    var ffmpeg = await ProbeFfmpegAsync(settings);
                    report = $"[musica] pronta: yt-dlp {ytdlp}, {ffmpeg}, runtime JS '{settings.jsRuntimes}'";
                }
                catch (Exception ex)
                {
                    problem = ex.Message;
                    report = $"[musica] aviso: desligada - {problem}. Ver a secao \"music\" do config.";
                }

                s_lastProbe = DateTimeOffset.UtcNow;
                Volatile.Write(ref s_unavailableReason, problem);

                // Uma linha so por mudanca de estado, e nao uma por /play.
                if (report != s_lastReport)
                {
                    s_lastReport = report;
                    Console.WriteLine(report);
                }

                return problem;
            }
            finally
            {
                s_probeLock.Release();
            }
        }

        private static async Task<string> ProbeFfmpegAsync(MusicSettings settings)
        {
            var result = await YtDlp.RunAsync(settings.ffmpegPath, new[] { "-hide_banner", "-version" },
                TimeSpan.FromSeconds(15), CancellationToken.None);
            if (result.ExitCode != 0)
                throw new InvalidOperationException($"ffmpeg -version saiu com {result.ExitCode}");

            // "ffmpeg version 8.1.2 Copyright ..." -> "ffmpeg 8.1.2"
            var parts = result.StdOut.Split(' ', 4, StringSplitOptions.RemoveEmptyEntries);
            return parts.Length >= 3 ? $"ffmpeg {parts[2]}" : "ffmpeg";
        }

        /// <summary>
        /// Relatorio de subida. Fora do caminho do gateway: roda dois processos.
        /// </summary>
        public static Task ReportStatusAsync(DiscordClient client, GuildDownloadCompletedEventArgs e)
        {
            _ = Task.Run(async () =>
            {
                try
                {
                    await CheckAvailabilityAsync(force: true);
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"[musica] falha no relatorio de subida: {ex.Message}");
                }
            });

            return Task.CompletedTask;
        }

        // ------------------------------------------------------------------
        // Conexao
        // ------------------------------------------------------------------

        /// <summary>
        /// Devolve o player do servidor, entrando no canal de voz se preciso.
        /// Lanca MusicException com a frase para o usuario quando nao da.
        /// </summary>
        public static async Task<GuildPlayer> GetOrJoinAsync(DiscordClient client, DiscordGuild guild,
            DiscordChannel voice, DiscordChannel text)
        {
            var gate = s_connectLocks.GetOrAdd(guild.Id, _ => new SemaphoreSlim(1, 1));
            await gate.WaitAsync();
            try
            {
                if (s_players.TryGetValue(guild.Id, out var existing))
                {
                    if (!existing.IsClosing)
                    {
                        if (existing.VoiceChannel?.Id != voice.Id)
                            throw new MusicException(
                                $"I'm already playing in {existing.VoiceChannel?.Mention ?? "another channel"}. Join me there to add songs.");

                        existing.TextChannel = text;
                        return existing;
                    }

                    // Saindo agora (fila vazia, /stop): espera o fim para nao
                    // abrir a conexao nova por cima da velha.
                    await existing.Completion.WaitAsync(TimeSpan.FromSeconds(10));
                }

                // Sobra de uma conexao que nao passou por aqui (um crash do
                // laco, por exemplo): a biblioteca recusaria conectar de novo.
                if (client.GetVoice()?.GetConnection(guild) is { } stale)
                {
                    try { stale.Disconnect(); }
                    catch (Exception ex) { Console.WriteLine($"[musica] aviso: conexao velha em {guild.Id}: {ex.Message}"); }
                }

                VoiceConnection connection;
                try
                {
                    connection = await voice.ConnectAsync();
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"[musica] falha ao entrar em {voice.Id} ({guild.Id}): {ex}");
                    throw new MusicException("I couldn't join your voice channel. Check that I can see it and that it isn't full.");
                }

                // DAVE: desde marco de 2026 o Discord so aceita audio cifrado
                // ponta a ponta. A negociacao corre depois do connect; tocar antes
                // dela terminar manda audio que ninguem consegue ouvir.
                if (!connection.IsE2eeUsableForSend)
                {
                    var ready = await connection.WaitForDaveActiveAsync(TimeSpan.FromSeconds(10), CancellationToken.None);
                    if (!ready && !connection.IsE2eeUsableForSend)
                        Console.WriteLine($"[musica] aviso: DAVE nao ficou pronto em 10s em {guild.Id} " +
                                          $"(estado {connection.DaveState}); tocando assim mesmo");
                }

                var player = new GuildPlayer(guild.Id, connection, text);
                s_players[guild.Id] = player;
                player.Start();

                Console.WriteLine($"[musica] entrou em {voice.Id} ({guild.Id}), DAVE {connection.DaveState} v{connection.DaveProtocolVersion}");
                return player;
            }
            finally
            {
                gate.Release();
            }
        }

        // ------------------------------------------------------------------
        // Quem esta ouvindo
        // ------------------------------------------------------------------

        /// <summary>Pessoas (nao bots) no canal de voz do player.</summary>
        public static List<DiscordMember> Listeners(GuildPlayer player)
        {
            var channel = player.VoiceChannel;
            if (channel is null)
                return new List<DiscordMember>();

            try
            {
                return channel.Users.Where(u => !u.IsBot).ToList();
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[musica] aviso: nao consegui listar o canal {channel.Id}: {ex.Message}");
                return new List<DiscordMember>();
            }
        }

        /// <summary>
        /// Quem manda sem votacao: quem tem Move Members (a permissao de
        /// arrastar gente entre canais de voz), quem pediu a faixa atual e quem
        /// esta sozinho com o bot - nesse caso nao ha maioria a consultar.
        /// </summary>
        public static bool CanControl(DiscordMember member, GuildPlayer player)
        {
            // No canal de voz, e nao no servidor: um overwrite pode dar (ou
            // tirar) Move Members so naquele canal. O PermissionsIn ja resolve
            // Administrator como tudo.
            var perms = player.VoiceChannel is { } channel ? member.PermissionsIn(channel) : member.Permissions;
            if ((perms & Permissions.MoveMembers) != 0)
                return true;

            if (player.Current?.RequesterId == member.Id)
                return true;

            var listeners = Listeners(player);
            return listeners.Count == 1 && listeners[0].Id == member.Id;
        }

        public static bool IsInPlayerChannel(DiscordMember member, GuildPlayer player) =>
            member.VoiceState?.Channel?.Id is { } channelId && channelId == player.VoiceChannel?.Id;

        // ------------------------------------------------------------------
        // Eventos de voz
        // ------------------------------------------------------------------

        /// <summary>
        /// Bot expulso ou movido, e o canal que esvazia. Nao ha evento publico de
        /// desconexao na biblioteca de voz, entao e o VoiceStateUpdated do proprio
        /// bot que diz que a conexao acabou.
        /// </summary>
        public static Task OnVoiceStateUpdated(DiscordClient client, VoiceStateUpdateEventArgs e)
        {
            if (e.Guild is null || !s_players.TryGetValue(e.Guild.Id, out var player) || player.IsClosing)
                return Task.CompletedTask;

            _ = Task.Run(async () =>
            {
                try
                {
                    if (e.User?.Id == client.CurrentUser.Id)
                    {
                        if (e.After?.Channel is null)
                        {
                            player.Stop("desconectado do canal");
                            return;
                        }
                    }

                    var settings = await ReadSettingsAsync();
                    player.UpdateAlone(Listeners(player).Count == 0, settings.IdleLeave);
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"[musica] falha ao tratar mudanca de voz em {e.Guild.Id}: {ex.Message}");
                }
            });

            return Task.CompletedTask;
        }

        // ------------------------------------------------------------------
        // Desligamento
        // ------------------------------------------------------------------

        /// <summary>
        /// Para todo player e espera a limpeza deles (desconectar, apagar
        /// arquivos) dentro do prazo. Chamado antes de largar o gateway.
        /// </summary>
        public static async Task DrainAsync(TimeSpan budget)
        {
            var players = s_players.Values.ToList();
            if (players.Count == 0)
                return;

            foreach (var player in players)
                player.Stop("desligamento");

            try
            {
                await Task.WhenAll(players.Select(p => p.Completion)).WaitAsync(budget);
            }
            catch (TimeoutException)
            {
                Console.WriteLine($"[musica] aviso: {players.Count(p => !p.Completion.IsCompleted)} player(s) nao terminaram a tempo");
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[musica] aviso: encerramento dos players: {ex.Message}");
            }

            CleanTempRoot();
        }
    }
}
