using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Threading;
using System.Threading.Tasks;
using CommunityBot.config;
using DisCatSharp.Entities;
using DisCatSharp.Enums.Core;
using DisCatSharp.Voice;

namespace CommunityBot.Services.Music
{
    /// <summary>
    /// O tocador de UM servidor: fila, faixa atual e o laco que baixa, toca e
    /// apaga.
    ///
    /// Todo o ciclo de vida mora no laco (RunAsync). Parar, sair por ociosidade e
    /// ser expulso do canal terminam do mesmo jeito - cancelando o laco -, e e o
    /// finally dele que desconecta, apaga os arquivos e tira o player do
    /// registro. Um lugar so para limpar e o que garante que nenhum caminho deixa
    /// audio no disco.
    /// </summary>
    internal sealed class GuildPlayer
    {
        // PCM que o ffmpeg entrega: 48 kHz, 2 canais, 16 bits.
        private const double BytesPerSecond = 48000 * 2 * 2;

        private readonly object _gate = new();
        private readonly List<QueuedTrack> _queue = new();
        private readonly HashSet<ulong> _skipVotes = new();
        private readonly SemaphoreSlim _wake = new(0);
        private readonly CancellationTokenSource _lifetime = new();
        private readonly string _directory;

        private Task _loop = Task.CompletedTask;
        private QueuedTrack? _current;
        private CancellationTokenSource? _entryCts;
        private CancellationTokenSource? _aloneCts;
        private (QueuedTrack Entry, Task<string> File)? _prefetch;
        private DiscordMessage? _nowPlaying;
        private LoopMode _loopMode = LoopMode.Off;
        private long _bytesSent;
        private int _volume = 100;
        private bool _paused;

        // Entre tirar a entrada da fila e ela virar a faixa atual ha um download
        // inteiro. Sem esta marca, quem desse /play nesse intervalo ouviria
        // "tocando agora" para uma faixa que ainda vai esperar a outra.
        private bool _busy;
        private string _closeReason = "parado";

        public GuildPlayer(ulong guildId, VoiceConnection connection, DiscordChannel textChannel)
        {
            GuildId = guildId;
            Connection = connection;
            TextChannel = textChannel;
            _directory = Path.Combine(MusicService.TempRoot, guildId.ToString());
        }

        public ulong GuildId { get; }
        public VoiceConnection Connection { get; }

        /// <summary>Onde saem os avisos de "Now playing". O ultimo canal onde alguem usou /play.</summary>
        public DiscordChannel TextChannel { get; set; }

        public DiscordChannel? VoiceChannel => Connection.TargetChannel;
        public bool IsClosing => _lifetime.IsCancellationRequested;
        public Task Completion => _loop;

        public QueuedTrack? Current { get { lock (_gate) return _current; } }
        public bool IsPaused { get { lock (_gate) return _paused; } }
        public int Volume { get { lock (_gate) return _volume; } }

        public LoopMode Loop
        {
            get { lock (_gate) return _loopMode; }
            set { lock (_gate) _loopMode = value; }
        }

        /// <summary>
        /// Posicao pelo que ja foi entregue a voz. Adianta no maximo o buffer do
        /// envio (meio segundo), e congela sozinha na pausa, porque a escrita
        /// bloqueia enquanto o envio esta parado.
        /// </summary>
        public TimeSpan Position => TimeSpan.FromSeconds(Interlocked.Read(ref _bytesSent) / BytesPerSecond);

        public void Start() => _loop = Task.Run(RunAsync);

        // ------------------------------------------------------------------
        // Fila
        // ------------------------------------------------------------------

        /// <summary>Posicao na fila, a partir de 1; 0 quando vai tocar agora.</summary>
        public bool TryEnqueue(QueuedTrack entry, int max, out int position)
        {
            lock (_gate)
            {
                position = 0;
                if (IsClosing || _queue.Count >= max)
                    return false;

                _queue.Add(entry);
                position = _current is null && !_busy && _queue.Count == 1 ? 0 : _queue.Count;
            }

            _wake.Release();
            return true;
        }

        public List<QueuedTrack> SnapshotQueue()
        {
            lock (_gate)
                return _queue.ToList();
        }

        public int QueueCount
        {
            get { lock (_gate) return _queue.Count; }
        }

        /// <summary>Tira a entrada da posicao pedida (a partir de 1), se <paramref name="allowed"/> deixar.</summary>
        public QueuedTrack? RemoveAt(int position, Func<QueuedTrack, bool> allowed, out bool denied)
        {
            denied = false;
            lock (_gate)
            {
                if (position < 1 || position > _queue.Count)
                    return null;

                var entry = _queue[position - 1];
                if (!allowed(entry))
                {
                    denied = true;
                    return null;
                }

                _queue.RemoveAt(position - 1);
                return entry;
            }
        }

        public int Shuffle()
        {
            lock (_gate)
            {
                // Fisher-Yates com o RNG do sistema, como o resto dos comandos fun.
                for (var i = _queue.Count - 1; i > 0; i--)
                {
                    var j = RandomNumberGenerator.GetInt32(i + 1);
                    (_queue[i], _queue[j]) = (_queue[j], _queue[i]);
                }

                return _queue.Count;
            }
        }

        public void ClearQueue()
        {
            lock (_gate)
                _queue.Clear();
        }

        // ------------------------------------------------------------------
        // Controles
        // ------------------------------------------------------------------

        public bool Skip()
        {
            CancellationTokenSource? cts;
            bool paused;
            lock (_gate)
            {
                cts = _entryCts;
                paused = _paused;
            }

            if (cts is null)
                return false;

            // Pausado, o envio esta parado e a proxima faixa ficaria muda ate
            // alguem lembrar de dar /resume.
            if (paused)
                _ = ResumeAsync();

            try
            {
                cts.Cancel();
            }
            catch (ObjectDisposedException)
            {
                // A faixa acabou entre a leitura e o Cancel.
            }

            return true;
        }

        /// <summary>
        /// Voto para pular. Devolve se pulou, quantos votos ha e quantos faltam.
        /// A maioria e sobre quem esta ouvindo AGORA, entao sair do canal conta
        /// como deixar de votar.
        /// </summary>
        public (bool Skipped, int Votes, int Needed, bool Duplicate) VoteSkip(ulong userId, IReadOnlyCollection<ulong> listeners)
        {
            var needed = Math.Max(1, (int)Math.Ceiling(listeners.Count / 2.0));
            int votes;
            bool duplicate;

            lock (_gate)
            {
                if (_current is null)
                    return (false, 0, needed, false);

                duplicate = !_skipVotes.Add(userId);
                _skipVotes.IntersectWith(listeners);
                votes = _skipVotes.Count;
            }

            if (votes < needed)
                return (false, votes, needed, duplicate);

            Skip();
            return (true, votes, needed, duplicate);
        }

        public async Task<bool> TogglePauseAsync()
        {
            if (IsPaused)
            {
                await ResumeAsync();
                return false;
            }

            Pause();
            return true;
        }

        public void Pause()
        {
            lock (_gate)
            {
                if (_paused)
                    return;
                _paused = true;
            }

            Connection.Pause();
        }

        public async Task ResumeAsync()
        {
            lock (_gate)
            {
                if (!_paused)
                    return;
                _paused = false;
            }

            await Connection.ResumeAsync();
        }

        public void SetVolume(int percent)
        {
            lock (_gate)
                _volume = percent;

            try
            {
                Connection.GetTransmitSink().VolumeModifier = percent / 100.0;
            }
            catch (Exception ex)
            {
                // Vale na proxima faixa de qualquer jeito: o StreamAsync aplica o
                // volume guardado ao comecar.
                Console.WriteLine($"[musica] aviso: nao consegui aplicar o volume agora em {GuildId}: {ex.Message}");
            }
        }

        /// <summary>Encerra tudo. O finally do laco faz a limpeza.</summary>
        public void Stop(string reason)
        {
            bool paused;
            lock (_gate)
            {
                if (_lifetime.IsCancellationRequested)
                    return;
                _closeReason = reason;
                paused = _paused;
            }

            // Pausado, a escrita na voz esta bloqueada esperando o envio voltar.
            // Retomar antes de cancelar garante que o laco solte e chegue a
            // limpeza, em vez de depender de a biblioteca honrar o token ali.
            if (paused)
                _ = ResumeAsync();

            try
            {
                _lifetime.Cancel();
            }
            catch (ObjectDisposedException)
            {
            }
        }

        /// <summary>
        /// Chamado a cada mudanca de voz no servidor. Sozinho no canal, arma a
        /// saida; alguem voltou, desarma.
        /// </summary>
        public void UpdateAlone(bool alone, TimeSpan grace)
        {
            CancellationTokenSource? previous = null;
            CancellationTokenSource? armed = null;

            lock (_gate)
            {
                if (alone && _aloneCts is null && !IsClosing)
                {
                    armed = _aloneCts = CancellationTokenSource.CreateLinkedTokenSource(_lifetime.Token);
                }
                else if (!alone && _aloneCts is not null)
                {
                    previous = _aloneCts;
                    _aloneCts = null;
                }
            }

            previous?.Cancel();
            previous?.Dispose();

            if (armed is null)
                return;

            _ = Task.Run(async () =>
            {
                try
                {
                    await Task.Delay(grace, armed.Token);
                }
                catch (OperationCanceledException)
                {
                    return;
                }

                await SayAsync(Embeds.Info("Everyone left",
                    "Nobody was listening anymore, so I left the voice channel and cleared the queue."));
                Stop("canal vazio");
            });
        }

        // ------------------------------------------------------------------
        // Laco
        // ------------------------------------------------------------------

        private async Task RunAsync()
        {
            var ct = _lifetime.Token;

            try
            {
                while (!ct.IsCancellationRequested)
                {
                    QueuedTrack? entry;
                    lock (_gate)
                    {
                        entry = _queue.Count > 0 ? _queue[0] : null;
                        if (entry is not null)
                        {
                            _queue.RemoveAt(0);
                            _busy = true;
                        }
                    }

                    if (entry is null)
                    {
                        var settings = await MusicService.ReadSettingsAsync();
                        if (await _wake.WaitAsync(settings.IdleLeave, ct))
                            continue;

                        if (QueueCount > 0)
                            continue;

                        await SayAsync(Embeds.Info("Queue finished",
                            "Nothing left to play, so I left the voice channel."));
                        lock (_gate)
                            _closeReason = "fila vazia";
                        break;
                    }

                    await PlayAsync(entry, ct);
                }
            }
            catch (OperationCanceledException) when (ct.IsCancellationRequested)
            {
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[musica] falha no player de {GuildId}: {ex}");
            }
            finally
            {
                await ShutdownAsync();
            }
        }

        private async Task PlayAsync(QueuedTrack entry, CancellationToken ct)
        {
            string? file = null;
            var entryCts = CancellationTokenSource.CreateLinkedTokenSource(ct);

            try
            {
                var settings = await MusicService.ReadSettingsAsync();

                try
                {
                    file = await TakeFileAsync(entry, settings, ct);
                }
                catch (MusicException ex)
                {
                    await SayAsync(Embeds.Error("Skipped a track", $"{MusicFormat.Link(entry.Track)}\n{ex.Message}"));
                    return;
                }
                catch (Exception ex) when (!ct.IsCancellationRequested)
                {
                    Console.WriteLine($"[musica] falha ao baixar {entry.Track.Id} em {GuildId}: {ex.Message}");
                    await SayAsync(Embeds.Error("Skipped a track",
                        $"{MusicFormat.Link(entry.Track)}\nSomething went wrong while downloading it."));
                    return;
                }

                lock (_gate)
                {
                    _current = entry;
                    _entryCts = entryCts;
                    _skipVotes.Clear();
                }

                StartPrefetch(settings);
                await AnnounceAsync(entry);

                bool interrupted;
                do
                {
                    try
                    {
                        interrupted = await StreamAsync(file, settings, entryCts.Token);
                    }
                    catch (Exception ex) when (ex is not OperationCanceledException)
                    {
                        // ffmpeg que nao abre, conexao de voz que caiu no meio:
                        // perde-se esta faixa, nao o player inteiro.
                        Console.WriteLine($"[musica] falha ao tocar {entry.Track.Id} em {GuildId}: {ex.Message}");
                        await SayAsync(Embeds.Error("Playback failed",
                            $"Something went wrong while playing {MusicFormat.Link(entry.Track)}, so I skipped it."));
                        return;
                    }
                }
                while (!interrupted && Loop == LoopMode.Track && !ct.IsCancellationRequested);

                // Loop de fila: a faixa volta ao fim, so como metadado. O audio
                // sera baixado de novo quando chegar a vez dela.
                if (Loop == LoopMode.Queue && !ct.IsCancellationRequested)
                {
                    lock (_gate)
                        _queue.Add(entry);
                }
            }
            finally
            {
                lock (_gate)
                {
                    _current = null;
                    _entryCts = null;
                    _busy = false;
                }

                entryCts.Dispose();
                await RetireNowPlayingAsync();
                DeleteFile(file);
            }
        }

        /// <summary>
        /// Toca um arquivo ate o fim. Devolve true se foi interrompido (skip ou
        /// stop), false se terminou sozinho.
        /// </summary>
        private async Task<bool> StreamAsync(string file, MusicSettings settings, CancellationToken token)
        {
            var psi = new ProcessStartInfo(settings.ffmpegPath)
            {
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false,
                CreateNoWindow = true
            };
            foreach (var arg in new[]
                     {
                         "-nostdin", "-hide_banner", "-loglevel", "error",
                         "-i", file, "-vn", "-ac", "2", "-ar", "48000", "-f", "s16le", "pipe:1"
                     })
                psi.ArgumentList.Add(arg);

            using var ffmpeg = new Process { StartInfo = psi };
            ffmpeg.Start();
            var stderr = ffmpeg.StandardError.ReadToEndAsync(CancellationToken.None);

            Interlocked.Exchange(ref _bytesSent, 0);

            var sink = Connection.GetTransmitSink();
            sink.VolumeModifier = Volume / 100.0;

            // Laco proprio em vez do CopyToAsync da biblioteca: e o que deixa
            // contar os bytes para o /nowplaying.
            var buffer = new byte[sink.SampleLength * 4];
            var stdout = ffmpeg.StandardOutput.BaseStream;
            var interrupted = false;

            try
            {
                int read;
                while ((read = await stdout.ReadAsync(buffer, token)) > 0)
                {
                    await sink.WriteAsync(buffer.AsMemory(0, read), token);
                    Interlocked.Add(ref _bytesSent, read);
                }

                await sink.FlushAsync(token);

                // So espera o fim do que ja esta na fila de envio (meio segundo).
                // Com prazo: se o sinal de "parou de falar" nao vier, a musica
                // seguinte nao pode ficar esperando para sempre.
                try
                {
                    await Connection.WaitForPlaybackFinishAsync().WaitAsync(TimeSpan.FromSeconds(15), token);
                }
                catch (TimeoutException)
                {
                }
            }
            catch (OperationCanceledException) when (token.IsCancellationRequested)
            {
                interrupted = true;
            }
            finally
            {
                YtDlp.Kill(ffmpeg);
            }

            string errors;
            try
            {
                errors = await stderr.WaitAsync(TimeSpan.FromSeconds(5));
            }
            catch
            {
                errors = string.Empty;
            }

            if (!interrupted && !string.IsNullOrWhiteSpace(errors))
                Console.WriteLine($"[musica] aviso: ffmpeg em {GuildId}: {Embeds.Trim(errors.Trim(), 500)}");

            return interrupted;
        }

        // ------------------------------------------------------------------
        // Arquivos
        // ------------------------------------------------------------------

        private async Task<string> TakeFileAsync(QueuedTrack entry, MusicSettings settings, CancellationToken ct)
        {
            if (_prefetch is { } p && ReferenceEquals(p.Entry, entry))
            {
                _prefetch = null;
                return await p.File;
            }

            // A fila mudou (/remove, /shuffle) depois do prefetch: aquele arquivo
            // nao serve mais.
            DiscardPrefetch();
            return await YtDlp.DownloadAsync(entry.Track, _directory, settings, ct);
        }

        /// <summary>
        /// Baixa a proxima enquanto a atual toca, para a troca de faixa nao
        /// esperar o download. So UMA a frente: e no maximo dois arquivos no
        /// disco por servidor.
        /// </summary>
        private void StartPrefetch(MusicSettings settings)
        {
            QueuedTrack? next;
            lock (_gate)
                next = _queue.Count > 0 ? _queue[0] : null;

            if (next is null || (_prefetch is { } p && ReferenceEquals(p.Entry, next)))
                return;

            DiscardPrefetch();
            _prefetch = (next, YtDlp.DownloadAsync(next.Track, _directory, settings, _lifetime.Token));
        }

        private void DiscardPrefetch()
        {
            if (_prefetch is not { } p)
                return;

            _prefetch = null;
            _ = p.File.ContinueWith(t =>
            {
                if (t.IsCompletedSuccessfully)
                    DeleteFile(t.Result);
                else
                    _ = t.Exception; // observada: o erro dela nao interessa mais a ninguem
            }, TaskScheduler.Default);
        }

        private void DeleteFile(string? file)
        {
            if (file is null)
                return;

            try
            {
                File.Delete(file);
                Console.WriteLine($"[musica] apagado {Path.GetFileName(file)} ({GuildId})");
            }
            catch (Exception ex)
            {
                // Sobra no /tmp privado da unit, que o systemd limpa no stop - e
                // a pasta inteira e apagada no fim do player e na proxima subida.
                Console.WriteLine($"[musica] aviso: nao consegui apagar {Path.GetFileName(file)}: {ex.Message}");
            }
        }

        // ------------------------------------------------------------------
        // Mensagens
        // ------------------------------------------------------------------

        private async Task AnnounceAsync(QueuedTrack entry)
        {
            try
            {
                var message = await TextChannel.SendMessageAsync(new DiscordMessageBuilder()
                    .AddEmbed(MusicFormat.NowPlaying(entry, Loop, QueueCount))
                    .AddComponents(MusicFormat.Controls())
                    .WithAllowedMentions(Array.Empty<IMention>()));

                Interlocked.Exchange(ref _nowPlaying, message);
            }
            catch (Exception ex)
            {
                // Sem permissao no canal de texto a musica toca igual; so o aviso
                // se perde.
                Console.WriteLine($"[musica] aviso: nao consegui anunciar a faixa em {TextChannel.Id}: {ex.Message}");
            }
        }

        /// <summary>Tira os botoes do aviso da faixa que acabou, para ninguem pular a musica seguinte por engano.</summary>
        private async Task RetireNowPlayingAsync()
        {
            var message = Interlocked.Exchange(ref _nowPlaying, null);
            if (message is null)
                return;

            try
            {
                await message.ModifyAsync(new DiscordMessageBuilder().AddEmbeds(message.Embeds), ModifyMode.Replace);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[musica] aviso: nao consegui tirar os botoes da mensagem {message.Id}: {ex.Message}");
            }
        }

        public async Task SayAsync(DiscordEmbed embed)
        {
            try
            {
                await TextChannel.SendMessageAsync(new DiscordMessageBuilder()
                    .AddEmbed(embed)
                    .WithAllowedMentions(Array.Empty<IMention>()));
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[musica] aviso: nao consegui avisar em {TextChannel.Id}: {ex.Message}");
            }
        }

        // ------------------------------------------------------------------
        // Encerramento
        // ------------------------------------------------------------------

        private async Task ShutdownAsync()
        {
            string reason;
            lock (_gate)
            {
                reason = _closeReason;
                _queue.Clear();
            }

            try
            {
                _lifetime.Cancel();
            }
            catch (ObjectDisposedException)
            {
            }

            DiscardPrefetch();
            await RetireNowPlayingAsync();

            try
            {
                Connection.Disconnect();
            }
            catch (Exception ex)
            {
                // Ja desconectado (expulso do canal, por exemplo): a biblioteca
                // pode ter descartado a conexao antes.
                Console.WriteLine($"[musica] aviso: desconexao em {GuildId}: {ex.Message}");
            }

            MusicService.Unregister(this);

            // Um prefetch cancelado pode terminar depois daqui; a pasta vai junto
            // de qualquer forma na proxima subida.
            try
            {
                if (Directory.Exists(_directory))
                    Directory.Delete(_directory, recursive: true);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[musica] aviso: nao consegui apagar a pasta de {GuildId}: {ex.Message}");
            }

            Console.WriteLine($"[musica] saiu do canal de voz em {GuildId} ({reason})");
        }
    }
}
