using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;

namespace CommunityBot.Services.Economy
{
    /// <summary>A carteira de uma pessoa. Global: vale em todo servidor com a economia ligada.</summary>
    internal sealed class Wallet
    {
        public long balance { get; set; }
        public DateTimeOffset? lastDailyUtc { get; set; }
        public int dailyStreak { get; set; }
        public DateTimeOffset? lastWorkUtc { get; set; }

        public Wallet Copy() => new()
        {
            balance = balance,
            lastDailyUtc = lastDailyUtc,
            dailyStreak = dailyStreak,
            lastWorkUtc = lastWorkUtc
        };
    }

    /// <summary>O que um mutador do EconomyStore enxerga. Ver UpdateAsync.</summary>
    internal sealed class EconomyEdit
    {
        private readonly Dictionary<ulong, Wallet> _users;

        public EconomyEdit(Dictionary<ulong, Wallet> users) => _users = users;

        public bool Changed { get; private set; }

        public void MarkChanged() => Changed = true;

        /// <summary>A carteira da pessoa, criada vazia se nao existir (sem marcar mudanca).</summary>
        public Wallet Get(ulong userId)
        {
            if (!_users.TryGetValue(userId, out var wallet))
                _users[userId] = wallet = new Wallet();
            return wallet;
        }
    }

    /// <summary>
    /// As carteiras de SOL$.
    ///
    /// Diferente do XP, cada mudanca vai para o disco NA HORA: dinheiro muda
    /// pouco (um daily, um work, um pay) e vale mais que um minuto de conversa.
    /// O mutador roda numa COPIA; so se a gravacao der certo a copia vira o
    /// estado publicado. Assim um disco cheio nao deixa a memoria a frente do
    /// arquivo - nem um /pay que "saiu" e some no proximo reinicio.
    /// </summary>
    internal sealed class EconomyStore
    {
        private const string Tag = "economia";

        public static EconomyStore Instance { get; } = new();

        internal sealed class EconomyFile
        {
            public Dictionary<ulong, Wallet> users { get; set; } = new();
            public DateTimeOffset? lastUpdatedUtc { get; set; }
        }

        private readonly string _path = AppPaths.Data("economy.json");
        private readonly SemaphoreSlim _lock = new(1, 1);

        // Publicado inteiro de uma vez e nunca mais alterado: quem le nao precisa
        // de lock, so da referencia atual.
        private volatile IReadOnlyDictionary<ulong, Wallet> _snapshot = new Dictionary<ulong, Wallet>();

        public async Task LoadAsync()
        {
            JsonFile.DeleteLeftoverTemp(_path, Tag);
            var file = await JsonFile.ReadAsync<EconomyFile>(_path, Tag);
            _snapshot = file.users ?? new Dictionary<ulong, Wallet>();
            Console.WriteLine($"[economia] carregado: {_snapshot.Count} carteira(s)");
        }

        /// <summary>Copia da carteira, ou null para quem nunca teve uma.</summary>
        public Wallet? For(ulong userId) => _snapshot.TryGetValue(userId, out var w) ? w.Copy() : null;

        /// <summary>Todas as carteiras com saldo, da maior para a menor.</summary>
        public List<(ulong User, long Balance)> Board() =>
            _snapshot.Where(kv => kv.Value.balance > 0)
                .Select(kv => (kv.Key, kv.Value.balance))
                .OrderByDescending(x => x.balance).ThenBy(x => x.Key)
                .ToList();

        /// <summary>
        /// Muda uma ou mais carteiras numa gravacao so. O mutador e sincrono e
        /// precisa chamar MarkChanged; sem isso nada e gravado.
        /// </summary>
        public async Task<T> UpdateAsync<T>(Func<EconomyEdit, T> mutate)
        {
            await _lock.WaitAsync();
            try
            {
                var working = _snapshot.ToDictionary(kv => kv.Key, kv => kv.Value.Copy());
                var edit = new EconomyEdit(working);
                var result = mutate(edit);

                if (edit.Changed)
                {
                    await JsonFile.WriteAtomicAsync(_path, new EconomyFile
                    {
                        users = working,
                        lastUpdatedUtc = DateTimeOffset.UtcNow
                    });
                    _snapshot = working;
                }

                return result;
            }
            finally
            {
                _lock.Release();
            }
        }
    }
}
