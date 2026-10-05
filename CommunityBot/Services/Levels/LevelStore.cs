using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;

namespace CommunityBot.Services.Levels
{
    /// <summary>
    /// O XP de cada pessoa em cada servidor, e a preferencia de DM de level up.
    ///
    /// Diferente dos outros armazenamentos, a MEMORIA e a fonte da verdade e o
    /// disco recebe uma copia a cada 30 segundos. Os outros releem e regravam o
    /// arquivo inteiro a cada mudanca, o que e certo para /warn e /config, mas
    /// nao para algo que muda a cada mensagem de cada servidor. O preco e
    /// conhecido: um crash perde no maximo 30 segundos de XP. O desligamento
    /// normal grava tudo (FlushAsync no fim do Program.Main).
    /// </summary>
    internal sealed class LevelStore
    {
        private const string Tag = "niveis";
        private static readonly TimeSpan s_flushInterval = TimeSpan.FromSeconds(30);

        public static LevelStore Instance { get; } = new();

        internal sealed class LevelFile
        {
            /// <summary>servidor -> pessoa -> XP total naquele servidor.</summary>
            public Dictionary<ulong, Dictionary<ulong, long>> guilds { get; set; } = new();

            /// <summary>So quem mudou algo do padrao aparece aqui.</summary>
            public Dictionary<ulong, UserPrefs> users { get; set; } = new();

            public DateTimeOffset? lastUpdatedUtc { get; set; }
        }

        internal sealed class UserPrefs
        {
            public bool levelUpDms { get; set; } = true;
        }

        private readonly string _path = AppPaths.Data("levels.json");
        private readonly object _gate = new();
        private readonly SemaphoreSlim _writeLock = new(1, 1);
        private LevelFile _file = new();
        private bool _dirty;
        private Timer? _timer;

        public async Task LoadAsync()
        {
            JsonFile.DeleteLeftoverTemp(_path, Tag);
            var file = await JsonFile.ReadAsync<LevelFile>(_path, Tag);
            file.guilds ??= new();
            file.users ??= new();

            lock (_gate)
                _file = file;

            _timer ??= new Timer(_ => _ = FlushSafeAsync(), null, s_flushInterval, s_flushInterval);

            Console.WriteLine($"[niveis] carregado: XP de {file.guilds.Values.Sum(g => g.Count)} membro(s) " +
                              $"em {file.guilds.Count} servidor(es)");
        }

        // ------------------------------------------------------------------
        // XP
        // ------------------------------------------------------------------

        /// <summary>Soma (ou tira, com delta negativo) e devolve o XP antes e depois. Nunca fica negativo.</summary>
        public (long Old, long New) AddXp(ulong guildId, ulong userId, long delta)
        {
            lock (_gate)
            {
                if (!_file.guilds.TryGetValue(guildId, out var members))
                    _file.guilds[guildId] = members = new Dictionary<ulong, long>();

                members.TryGetValue(userId, out var old);
                var updated = Math.Max(0, old + delta);

                if (updated == 0)
                    members.Remove(userId);
                else
                    members[userId] = updated;

                if (updated != old)
                    _dirty = true;

                return (old, updated);
            }
        }

        public long XpOf(ulong guildId, ulong userId)
        {
            lock (_gate)
                return _file.guilds.TryGetValue(guildId, out var members) && members.TryGetValue(userId, out var xp) ? xp : 0;
        }

        /// <summary>O ranking de um servidor, do maior XP para o menor.</summary>
        public List<(ulong User, long Xp)> GuildBoard(ulong guildId)
        {
            lock (_gate)
            {
                return _file.guilds.TryGetValue(guildId, out var members)
                    ? members.Select(kv => (kv.Key, kv.Value)).OrderByDescending(x => x.Value).ThenBy(x => x.Key).ToList()
                    : new List<(ulong, long)>();
            }
        }

        /// <summary>
        /// O ranking global: a soma do XP de cada pessoa nos servidores em que o
        /// nivel esta LIGADO. Um servidor que desliga sai das contas globais na
        /// hora; os dados dele continuam guardados para quando religar.
        /// </summary>
        public List<(ulong User, long Xp)> GlobalBoard(Func<ulong, bool> guildEnabled)
        {
            lock (_gate)
            {
                var totals = new Dictionary<ulong, long>();
                foreach (var (guildId, members) in _file.guilds)
                {
                    if (!guildEnabled(guildId))
                        continue;

                    foreach (var (userId, xp) in members)
                        totals[userId] = totals.GetValueOrDefault(userId) + xp;
                }

                return totals.Select(kv => (kv.Key, kv.Value)).OrderByDescending(x => x.Value).ThenBy(x => x.Key).ToList();
            }
        }

        /// <summary>Em quantos servidores com nivel ligado a pessoa tem XP.</summary>
        public int ActiveGuildCount(ulong userId, Func<ulong, bool> guildEnabled)
        {
            lock (_gate)
                return _file.guilds.Count(g => guildEnabled(g.Key) && g.Value.ContainsKey(userId));
        }

        // ------------------------------------------------------------------
        // DM de level up
        // ------------------------------------------------------------------

        public bool DmEnabled(ulong userId)
        {
            lock (_gate)
                return !_file.users.TryGetValue(userId, out var prefs) || prefs.levelUpDms;
        }

        public void SetDm(ulong userId, bool enabled)
        {
            lock (_gate)
            {
                // O padrao (ligado) nao ocupa espaco no arquivo.
                var changed = enabled ? _file.users.Remove(userId) : !_file.users.ContainsKey(userId);
                if (!enabled)
                    _file.users[userId] = new UserPrefs { levelUpDms = false };

                if (changed)
                    _dirty = true;
            }
        }

        // ------------------------------------------------------------------
        // Disco
        // ------------------------------------------------------------------

        /// <summary>Grava se houve mudanca desde a ultima vez. Chamado pelo timer e no desligamento.</summary>
        public async Task FlushAsync()
        {
            LevelFile snapshot;
            lock (_gate)
            {
                if (!_dirty)
                    return;

                // Copia funda sob o lock: a gravacao corre fora dele, enquanto as
                // mensagens continuam somando XP no original.
                snapshot = new LevelFile
                {
                    guilds = _file.guilds.ToDictionary(g => g.Key, g => new Dictionary<ulong, long>(g.Value)),
                    users = _file.users.ToDictionary(u => u.Key, u => new UserPrefs { levelUpDms = u.Value.levelUpDms }),
                    lastUpdatedUtc = DateTimeOffset.UtcNow
                };
                _dirty = false;
            }

            await _writeLock.WaitAsync();
            try
            {
                await JsonFile.WriteAtomicAsync(_path, snapshot);
            }
            catch
            {
                // Volta a marcar como sujo: a proxima volta do timer tenta de novo.
                lock (_gate)
                    _dirty = true;
                throw;
            }
            finally
            {
                _writeLock.Release();
            }
        }

        private async Task FlushSafeAsync()
        {
            try
            {
                await FlushAsync();
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[niveis] falha ao gravar {_path}: {ex.Message}");
            }
        }

        public void StopTimer()
        {
            _timer?.Dispose();
            _timer = null;
        }
    }
}
