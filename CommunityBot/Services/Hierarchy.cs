using System.Linq;
using System.Threading.Tasks;
using DisCatSharp.ApplicationCommands.Context;
using DisCatSharp.Entities;
using DisCatSharp.Exceptions;

namespace CommunityBot.Services
{
    /// <summary>
    /// Checagem de hierarquia antes de qualquer punicao.
    ///
    /// Sem isto o comando so falha la na frente, no meio da chamada REST, e o que
    /// chega para quem usou e um erro cru da API. Aqui a recusa vem antes e diz o
    /// motivo.
    ///
    /// As permissoes do Discord ja barram quem nao e moderador; o que elas NAO
    /// fazem e impedir um moderador de agir sobre alguem do mesmo nivel ou acima.
    /// </summary>
    internal static class Hierarchy
    {
        /// <summary>
        /// Retorna null quando a acao e permitida, ou o embed de recusa.
        ///
        /// <paramref name="bot"/> e nullable porque a origem dele -
        /// <c>guild.CurrentMember</c> - pode vir nula com o cache frio, logo
        /// depois do connect. Antes o parametro era nao-nulo e os chamadores
        /// passavam esse valor mesmo assim (quatro CS8604): o `bot.Id` da linha
        /// seguinte lancava NullReferenceException e o moderador via um generico
        /// "Algo quebrou" - falha ABERTA, porque a checagem nao chegava a rodar.
        /// Agora, sem saber onde o bot esta na hierarquia, a resposta e recusar.
        /// </summary>
        public static DiscordEmbed? Check(DiscordGuild guild, DiscordMember actor, DiscordMember target, DiscordMember? bot)
        {
            if (bot is null)
            {
                return Embeds.Error("Hierarquia indisponível",
                    "Não consegui ler o meu próprio cargo neste servidor agora, então não dá para conferir a hierarquia. Tente de novo em alguns segundos.");
            }

            if (target.Id == actor.Id)
                return Embeds.Error("Alvo inválido", "Você não pode aplicar isso em você mesmo.");

            if (target.Id == bot.Id)
                return Embeds.Error("Alvo inválido", "Não posso aplicar isso em mim mesmo.");

            // guild.OwnerId e `ulong?`, e a comparacao com um `ulong` e LIFTED:
            // com OwnerId nulo, `target.Id == guild.OwnerId` da FALSE e a
            // protecao do dono sumia em silencio, sem nenhum aviso do compilador.
            // Era o unico ponto desta classe que falhava ABERTO - e num arquivo
            // cujo docstring promete o contrario. Nao saber quem e o dono e o
            // mesmo caso de nao conseguir ler o proprio cargo: recusa.
            if (guild.OwnerId is not { } ownerId)
            {
                return Embeds.Error("Hierarquia indisponível",
                    "Não consegui ler quem é o dono deste servidor agora, então não dá para conferir a hierarquia. Tente de novo em alguns segundos.");
            }

            if (target.Id == ownerId)
                return Embeds.Error("Alvo inválido", "Não é possível moderar o dono do servidor.");

            // O dono passa por cima da comparacao de cargo: o cargo mais alto dele
            // nao precisa ser o mais alto do servidor.
            if (actor.Id != ownerId && TopRole(target) >= TopRole(actor))
            {
                return Embeds.Error("Hierarquia",
                    $"{target.Mention} tem um cargo igual ou mais alto que o seu, então você não pode moderá-lo.");
            }

            if (TopRole(target) >= TopRole(bot))
            {
                return Embeds.Error("Hierarquia",
                    $"{target.Mention} está acima do meu cargo mais alto. Mova o meu cargo para cima na lista de cargos do servidor.");
            }

            return null;
        }

        /// <summary>
        /// Busca o membro. Devolve null SO quando ele de fato nao esta no
        /// servidor.
        ///
        /// O catch aberto de antes engolia tambem rate limit, 5xx e falha de
        /// rede - e o chamador trata null como "saiu do servidor" e PULA a
        /// checagem de hierarquia. Ou seja, a checagem falhava aberta: numa
        /// instabilidade qualquer, um moderador com apenas Ban Members conseguia
        /// punir alguem acima dele. Agora so o NotFound vira null; o resto sobe
        /// e o comando falha de forma visivel, que e o lado certo para errar.
        ///
        /// Mora aqui, e nao em cada modulo de comando: Moderation e Warnings
        /// tinham copias byte a byte deste metodo, entao a proxima correcao
        /// entraria em so uma das duas.
        /// </summary>
        public static Task<DiscordMember?> TryGetMemberAsync(InteractionContext ctx, ulong userId) =>
            TryGetMemberAsync(ctx.Guild!, userId);

        /// <summary>
        /// A mesma busca, para quem nao veio de uma interacao - o vigia de canal
        /// reage a uma mensagem e nao tem InteractionContext nenhum nas maos.
        /// A sobrecarga acima delega para ca justamente para o catch estreito
        /// existir uma vez so.
        /// </summary>
        public static async Task<DiscordMember?> TryGetMemberAsync(DiscordGuild guild, ulong userId)
        {
            try
            {
                return await guild.GetMemberAsync(userId);
            }
            catch (NotFoundException)
            {
                return null;
            }
        }

        /// <summary>
        /// Diz se <paramref name="actor"/> esta ESTRITAMENTE acima de
        /// <paramref name="other"/> na hierarquia.
        ///
        /// Separado do Check porque nem toda decisao e "posso punir esta pessoa?".
        /// O /delwarn precisa de "posso mexer no registro que AQUELE moderador
        /// escreveu?", que compara dois moderadores e nao tem alvo de punicao.
        ///
        /// Falha fechado: sem saber quem e o dono, ninguem ganha o atalho de dono
        /// e a decisao recai na comparacao de cargo.
        /// </summary>
        public static bool Outranks(DiscordGuild guild, DiscordMember actor, DiscordMember other)
        {
            if (guild.OwnerId is { } ownerId)
            {
                if (actor.Id == ownerId)
                    return true;

                if (other.Id == ownerId)
                    return false;
            }

            return TopRole(actor) > TopRole(other);
        }

        /// <summary>
        /// Posicao do cargo mais alto. Membro sem cargo nenhum fica em -1, abaixo
        /// de @everyone, que e posicao 0.
        /// </summary>
        private static int TopRole(DiscordMember member) =>
            member.Roles.Any() ? member.Roles.Max(r => r.Position) : -1;
    }
}
