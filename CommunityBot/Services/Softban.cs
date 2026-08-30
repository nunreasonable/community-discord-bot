using System;
using System.Threading.Tasks;
using DisCatSharp.Entities;

namespace CommunityBot.Services
{
    /// <summary>
    /// Softban: banir e desbanir na sequencia.
    ///
    /// O efeito util e o do MEIO. O ban do Discord aceita apagar o historico
    /// recente do alvo em todos os canais de uma vez - coisa que nem o /kick nem
    /// o /purge fazem - e o unban logo depois devolve a pessoa a condicao de quem
    /// so foi expulso: ela pode voltar por convite. Feito a mao sao dois passos,
    /// e esquecer o segundo transforma uma limpeza num banimento permanente.
    ///
    /// Mora num servico, e nao dentro do comando, porque o vigia de canal
    /// (<see cref="AutoSoftban"/>) precisa exatamente do mesmo caminho e nao tem
    /// interacao nenhuma nas maos. O Hierarchy ja documenta o preco de ter
    /// deixado duas copias byte a byte de um metodo de moderacao por ai.
    /// </summary>
    internal static class Softban
    {
        /// <summary>
        /// Sete dias em SEGUNDOS, que e a unidade que a API usa - o /ban daqui
        /// carrega o comentario do dia em que passar "7" cru apagou 7 segundos de
        /// mensagens. E o teto que o Discord permite, e apagar o historico e o
        /// proposito inteiro do softban: nao ha valor menor que faca sentido
        /// oferecer como opcao.
        /// </summary>
        public const int DeleteMessageSeconds = 7 * 86400;

        private const int UnbanAttempts = 3;
        private static readonly TimeSpan UnbanRetryDelay = TimeSpan.FromSeconds(1);

        public enum SoftbanOutcome
        {
            /// <summary>Banido e desbanido. Fim.</summary>
            Ok,

            /// <summary>O ban nao passou; nada aconteceu com o alvo.</summary>
            BanFailed,

            /// <summary>O ban passou e o unban NAO. O alvo ficou banido.</summary>
            UnbanFailed
        }

        public sealed record SoftbanResult(SoftbanOutcome Outcome, string? Error);

        /// <summary>
        /// Aplica o softban. Nunca lanca: o resultado diz o que sobrou de pe.
        ///
        /// Os dois modos de falha PRECISAM ser distinguiveis. Se o ban falha, o
        /// servidor ficou como estava. Se o unban falha, a pessoa esta banida de
        /// verdade - o oposto do que o comando prometeu - e quem chamou tem de
        /// dizer isso em voz alta, para alguem desfazer a mao. Um catch so, com
        /// uma mensagem generica de "falhou", mentiria sobre o estado do servidor
        /// justamente no caso em que o estado mudou.
        /// </summary>
        public static async Task<SoftbanResult> ApplyAsync(DiscordGuild guild, ulong userId, string auditReason)
        {
            try
            {
                await guild.BanMemberAsync(userId, DeleteMessageSeconds, auditReason);
            }
            catch (Exception ex)
            {
                return new SoftbanResult(SoftbanOutcome.BanFailed, ex.Message);
            }

            /*
             * O unban INSISTE, e o ban nao.
             *
             * Os dois lados falham de formas diferentes. Se o ban nao passa, o
             * servidor ficou como estava e tentar de novo e opcional. Se o unban
             * nao passa, existe agora um banimento permanente que ninguem pediu -
             * e o custo de nao insistir e uma pessoa banida de verdade ate alguem
             * reparar a mao.
             *
             * Tres tentativas com espera curta cobrem o 5xx passageiro e o
             * intervalo em que o proprio DisCatSharp esta segurando um 429. Nao e
             * garantia: se todas falharem, o resultado diz isso em voz alta, que e
             * a unica coisa honesta a fazer.
             */
            Exception? last = null;

            for (var attempt = 1; attempt <= UnbanAttempts; attempt++)
            {
                try
                {
                    await guild.UnbanMemberAsync(userId, auditReason);
                    return new SoftbanResult(SoftbanOutcome.Ok, null);
                }
                catch (Exception ex)
                {
                    last = ex;

                    if (attempt < UnbanAttempts)
                        await Task.Delay(UnbanRetryDelay * attempt);
                }
            }

            return new SoftbanResult(SoftbanOutcome.UnbanFailed, last?.Message);
        }
    }
}
