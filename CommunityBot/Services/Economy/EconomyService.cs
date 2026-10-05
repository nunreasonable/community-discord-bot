using System;
using System.Collections.Concurrent;
using System.Globalization;
using System.Linq;
using System.Security.Cryptography;
using System.Threading.Tasks;

namespace CommunityBot.Services.Economy
{
    /// <summary>
    /// As regras da economia de SOL$, no molde da Loritta: um claim por dia com
    /// sequencia, um trabalho por hora e doacao entre pessoas.
    ///
    /// O relogio entra como parametro nas regras que dependem dele: e o que deixa
    /// testar "virou o dia" sem esperar o dia virar.
    /// </summary>
    internal static class EconomyService
    {
        public const string Currency = "SOL$";

        public const long DailyBase = 500;
        public const long DailyStreakBonus = 50;
        public const int DailyStreakCap = 10;

        public static readonly TimeSpan WorkCooldown = TimeSpan.FromHours(1);
        public const int WorkMin = 80;
        public const int WorkMax = 250;

        /// <summary>"1,250 SOL$" - separador fixo, para nao depender da cultura da maquina.</summary>
        public static string Format(long amount) =>
            $"{amount.ToString("N0", CultureInfo.InvariantCulture)} {Currency}";

        // ------------------------------------------------------------------
        // Daily
        // ------------------------------------------------------------------

        internal readonly record struct DailyResult(bool Claimed, long Reward, int Streak, DateTimeOffset NextAt, long Balance);

        /// <summary>
        /// Um claim por dia UTC. A sequencia continua se o ultimo foi ontem e
        /// volta a 1 se nao foi; cada dia seguido soma 50 ao premio, ate 10 dias
        /// (500 a 1.000 SOL$).
        /// </summary>
        public static Task<DailyResult> ClaimDailyAsync(ulong userId, DateTimeOffset now) =>
            EconomyStore.Instance.UpdateAsync(edit =>
            {
                var wallet = edit.Get(userId);
                var today = now.UtcDateTime.Date;
                var last = wallet.lastDailyUtc?.UtcDateTime.Date;
                var next = new DateTimeOffset(today.AddDays(1), TimeSpan.Zero);

                if (last == today)
                    return new DailyResult(false, 0, wallet.dailyStreak, next, wallet.balance);

                var streak = last == today.AddDays(-1) ? wallet.dailyStreak + 1 : 1;
                var reward = DailyBase + DailyStreakBonus * Math.Min(streak - 1, DailyStreakCap);

                wallet.balance += reward;
                wallet.dailyStreak = streak;
                wallet.lastDailyUtc = now;
                edit.MarkChanged();

                return new DailyResult(true, reward, streak, next, wallet.balance);
            });

        /// <summary>Quando o proximo daily libera, ou null se ja esta liberado.</summary>
        public static DateTimeOffset? NextDaily(Wallet? wallet, DateTimeOffset now)
        {
            var today = now.UtcDateTime.Date;
            return wallet?.lastDailyUtc?.UtcDateTime.Date == today
                ? new DateTimeOffset(today.AddDays(1), TimeSpan.Zero)
                : null;
        }

        /// <summary>A sequencia que ainda vale: quem pulou um dia ja a perdeu, mesmo antes do proximo claim.</summary>
        public static int LiveStreak(Wallet? wallet, DateTimeOffset now)
        {
            var last = wallet?.lastDailyUtc?.UtcDateTime.Date;
            var today = now.UtcDateTime.Date;
            return last == today || last == today.AddDays(-1) ? wallet!.dailyStreak : 0;
        }

        // ------------------------------------------------------------------
        // Work
        // ------------------------------------------------------------------

        private static readonly string[] s_jobs =
        {
            "You walked a pack of very excited dogs around the block",
            "You delivered pizzas across town (only one got slightly squished)",
            "You fixed a printer that was definitely haunted",
            "You streamed for three hours to an audience of four",
            "You debugged someone else's code with zero comments",
            "You moderated a Discord server through a meme war",
            "You worked the night shift at the space station canteen",
            "You taught a parrot to say \"git push --force\"",
            "You sorted 2,000 Lego bricks by color",
            "You photographed a very serious corgi wedding",
            "You harvested solar panels on the sunny side of the server",
            "You translated a menu written entirely in emoji",
            "You tuned the pianos at the local concert hall",
            "You beta-tested a game that crashed on the title screen",
            "You returned 47 overdue library books for the whole town"
        };

        internal readonly record struct WorkResult(bool Worked, long Reward, string Job, DateTimeOffset NextAt, long Balance);

        public static Task<WorkResult> WorkAsync(ulong userId, DateTimeOffset now) =>
            EconomyStore.Instance.UpdateAsync(edit =>
            {
                var wallet = edit.Get(userId);

                if (wallet.lastWorkUtc is { } last && now - last < WorkCooldown)
                    return new WorkResult(false, 0, string.Empty, last + WorkCooldown, wallet.balance);

                var reward = (long)RandomNumberGenerator.GetInt32(WorkMin, WorkMax + 1);
                var job = s_jobs[RandomNumberGenerator.GetInt32(s_jobs.Length)];

                wallet.balance += reward;
                wallet.lastWorkUtc = now;
                edit.MarkChanged();

                return new WorkResult(true, reward, job, now + WorkCooldown, wallet.balance);
            });

        // ------------------------------------------------------------------
        // Pay
        // ------------------------------------------------------------------

        internal readonly record struct TransferResult(bool Done, string? Refusal, long PayerBalance);

        /// <summary>Debita e credita na MESMA gravacao: nao existe meio pagamento.</summary>
        public static Task<TransferResult> TransferAsync(ulong payerId, ulong targetId, long amount) =>
            EconomyStore.Instance.UpdateAsync(edit =>
            {
                if (amount <= 0)
                    return new TransferResult(false, "The amount has to be more than zero.", 0);
                if (payerId == targetId)
                    return new TransferResult(false, "You can't pay yourself.", 0);

                var payer = edit.Get(payerId);
                if (payer.balance < amount)
                    return new TransferResult(false,
                        $"You only have {Format(payer.balance)}.", payer.balance);

                var target = edit.Get(targetId);
                payer.balance -= amount;
                target.balance = checked(target.balance + amount);
                edit.MarkChanged();

                return new TransferResult(true, null, payer.balance);
            });

        /// <summary>Um /pay esperando o clique de confirmacao de quem paga.</summary>
        internal sealed record PendingPay(ulong PayerId, ulong TargetId, long Amount, DateTimeOffset ExpiresAt);

        public static readonly TimeSpan PayConfirmWindow = TimeSpan.FromMinutes(2);

        private static readonly ConcurrentDictionary<string, PendingPay> s_pending = new();

        /// <summary>
        /// Guarda o pedido e devolve a chave que vai no botao. O valor fica AQUI,
        /// e nao no custom id: o botao so carrega uma chave aleatoria, que nao da
        /// para forjar nem reaproveitar.
        /// </summary>
        public static string RegisterPending(ulong payerId, ulong targetId, long amount, DateTimeOffset now)
        {
            foreach (var stale in s_pending.Where(kv => kv.Value.ExpiresAt < now).Select(kv => kv.Key).ToList())
                s_pending.TryRemove(stale, out _);

            var nonce = Convert.ToHexString(RandomNumberGenerator.GetBytes(8)).ToLowerInvariant();
            s_pending[nonce] = new PendingPay(payerId, targetId, amount, now + PayConfirmWindow);
            return nonce;
        }

        /// <summary>
        /// Tira o pedido da fila (uso unico) se quem clicou e quem paga. Devolve
        /// null quando ja foi usado, expirou ou nunca existiu - um reinicio do bot
        /// tambem zera os pendentes.
        /// </summary>
        public static PendingPay? TakePending(string nonce, ulong clickerId, DateTimeOffset now, out bool notYours)
        {
            notYours = false;
            if (!s_pending.TryGetValue(nonce, out var pending))
                return null;

            if (pending.PayerId != clickerId)
            {
                notYours = true;
                return null;
            }

            if (!s_pending.TryRemove(nonce, out pending) || pending.ExpiresAt < now)
                return null;

            return pending;
        }

        public static void CancelPending(string nonce) => s_pending.TryRemove(nonce, out _);
    }
}
