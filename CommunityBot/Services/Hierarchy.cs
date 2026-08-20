using System.Linq;
using DisCatSharp.Entities;

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
        /// <summary>Retorna null quando a acao e permitida, ou o embed de recusa.</summary>
        public static DiscordEmbed? Check(DiscordGuild guild, DiscordMember actor, DiscordMember target, DiscordMember bot)
        {
            if (target.Id == actor.Id)
                return Embeds.Error("Alvo inválido", "Você não pode aplicar isso em você mesmo.");

            if (target.Id == bot.Id)
                return Embeds.Error("Alvo inválido", "Não posso aplicar isso em mim mesmo.");

            if (target.Id == guild.OwnerId)
                return Embeds.Error("Alvo inválido", "Não é possível moderar o dono do servidor.");

            // O dono passa por cima da comparacao de cargo: o cargo mais alto dele
            // nao precisa ser o mais alto do servidor.
            if (actor.Id != guild.OwnerId && TopRole(target) >= TopRole(actor))
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
        /// Posicao do cargo mais alto. Membro sem cargo nenhum fica em -1, abaixo
        /// de @everyone, que e posicao 0.
        /// </summary>
        private static int TopRole(DiscordMember member) =>
            member.Roles.Any() ? member.Roles.Max(r => r.Position) : -1;
    }
}
