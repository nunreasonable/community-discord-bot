using DisCatSharp;
using DisCatSharp.Entities;

namespace CommunityBot.Services
{
    /// <summary>
    /// Onde um comando "de qualquer lugar" esta rodando.
    ///
    /// Instalado como app pessoal, o Sollarety recebe comandos de servidores em
    /// que o BOT nao esta: o Discord manda o id do servidor, mas o bot nao tem o
    /// servidor em cache, nao ve os membros e nao pode mexer em nada la. Ver um
    /// ctx.Guild nao basta para saber se da para consultar membro, cargo ou
    /// configuracao - so o cache do proprio bot diz isso.
    /// </summary>
    internal static class CommandScope
    {
        /// <summary>O servidor, se o bot estiver nele; null em DM e em servidor onde so a pessoa instalou o app.</summary>
        public static DiscordGuild? BotGuild(DiscordClient client, DiscordGuild? guild) =>
            guild is not null && client.Guilds.TryGetValue(guild.Id, out var cached) ? cached : null;
    }
}
