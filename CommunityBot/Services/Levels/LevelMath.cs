using System;

namespace CommunityBot.Services.Levels
{
    /// <summary>
    /// A curva de nivel do MEE6: do nivel L para o L+1 sao 5L² + 50L + 100 XP.
    /// O nivel 1 sai com 100 XP, o 10 com 4.675, o 50 com 268.375 - e quem ja
    /// usou bot de nivel reconhece o ritmo.
    /// </summary>
    internal static class LevelMath
    {
        // Teto do laco: com XP em long o nivel nunca chega perto disto, mas um
        // valor absurdo no arquivo nao pode virar laco sem fim.
        private const int MaxLevel = 10_000;

        public static long XpToNext(int level) => 5L * level * level + 50L * level + 100;

        /// <summary>Nivel, XP ja feito dentro dele e quanto o nivel inteiro pede.</summary>
        public static (int Level, long Into, long Needed) Progress(long totalXp)
        {
            var level = 0;
            var remaining = Math.Max(0, totalXp);

            while (level < MaxLevel && remaining >= XpToNext(level))
            {
                remaining -= XpToNext(level);
                level++;
            }

            return (level, remaining, XpToNext(level));
        }

        public static int LevelOf(long totalXp) => Progress(totalXp).Level;

        /// <summary>XP total para CHEGAR ao nivel pedido.</summary>
        public static long TotalFor(int level)
        {
            long total = 0;
            for (var l = 0; l < Math.Min(level, MaxLevel); l++)
                total += XpToNext(l);
            return total;
        }
    }
}
