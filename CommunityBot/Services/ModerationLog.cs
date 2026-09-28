using System;
using System.Threading.Tasks;
using DisCatSharp;
using DisCatSharp.Entities;

namespace CommunityBot.Services
{
    /// <summary>
    /// Registro das acoes de moderacao num canal do servidor.
    ///
    /// Moderacao sem registro nao e auditavel, e "quem baniu fulano?" e a
    /// primeira pergunta que aparece. O Audit Log nativo do Discord guarda a
    /// acao, mas nao o motivo digitado no comando nem o id da advertencia.
    ///
    /// Nunca lanca: falhar em registrar nao pode desfazer nem mascarar uma
    /// punicao que ja foi aplicada. A falha vai para o log do bot, que o /logs le.
    /// </summary>
    internal static class ModerationLog
    {
        /// <summary>
        /// Le o config e registra, sem nunca lancar.
        ///
        /// Os comandos chamavam `new JSONReader()` + `ReadJSON()` por conta
        /// propria DEPOIS de ja terem respondido "sucesso" ao moderador, e fora
        /// de qualquer try. Se essa leitura falhasse (arquivo sendo reescrito,
        /// JSON invalido), a excecao chegava ao SlashCommandErrored, que edita a
        /// MESMA resposta original - o moderador via "Command failed" numa
        /// punicao que tinha sido aplicada. Trazendo a leitura para dentro deste
        /// metodo, ela passa a ser coberta pelo mesmo catch do resto.
        /// </summary>
        /// <summary>
        /// O canal de log e UM so para o processo inteiro, e o
        /// client.GetChannelAsync resolve qualquer id em qualquer servidor onde o
        /// bot esteja. Sem esta conferencia, com o bot em mais de um servidor -
        /// que e o caso desde que o vigia de canal passou a apontar para um
        /// servidor diferente do de registro dos comandos - o /ban de um servidor
        /// ia parar no canal de log de OUTRO, entregando motivo, alvo e moderador
        /// a gente que nao tem nada com aquilo.
        ///
        /// Na duvida nao publica: e o lado certo para errar quando o assunto e
        /// vazar registro de moderacao.
        /// </summary>
        private static bool BelongsToGuild(DiscordChannel? channel, ulong guildId, string what)
        {
            if (channel is null)
                return false;

            if (channel.GuildId == guildId)
                return true;

            Console.WriteLine($"[modlog] '{what}' NAO registrado: o canal de log configurado esta no servidor " +
                              $"{channel.GuildId?.ToString() ?? "(nenhum)"}, e a acao aconteceu no {guildId}. " +
                              "Configure um moderationLogChannelId do proprio servidor.");
            return false;
        }

        public static async Task RecordAsync(
            DiscordClient client,
            ulong guildId,
            string action,
            DiscordUser? target,
            DiscordUser moderator,
            string? reason,
            string? extra = null,
            string noTargetLabel = "*entire channel*")
        {
            try
            {
                await WriteAsync(client, guildId, action, target, moderator, reason, extra, noTargetLabel);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[modlog] falha ao registrar '{action}': {ex.Message}");
            }
        }

        /// <summary>
        /// Aviso do proprio bot no canal de log - nao e o registro de uma
        /// punicao, e por isso nao passa pelo RecordAsync: la os campos sao
        /// "User" e "Moderator", e um alerta como o do disjuntor do vigia nao
        /// tem nem um nem outro. Forcar isso no molde de punicao produzia
        /// "User: *entire channel*", que nao quer dizer nada aqui.
        ///
        /// Vermelho pela mesma regra do resto do bot: e uma condicao que exige
        /// alguem olhar.
        ///
        /// Nunca lanca, como o RecordAsync.
        /// </summary>
        public static async Task AlertAsync(DiscordClient client, ulong guildId, string title, string description)
        {
            try
            {
                var configured = GuildSettingsStore.For(guildId)?.moderationLogChannelId;
                if (configured is not > 0)
                    return;

                var channel = await client.GetChannelAsync(configured.Value);
                if (!BelongsToGuild(channel, guildId, title))
                    return;

                var embed = new DiscordEmbedBuilder()
                    .WithTitle($"⚠️ {Embeds.Trim(title, 200)}")
                    .WithDescription(Embeds.Trim(description, 3500))
                    .WithColor(DiscordColor.IndianRed)
                    .WithTimestamp(DateTimeOffset.UtcNow);

                await channel.SendMessageAsync(new DiscordMessageBuilder().AddEmbed(embed));
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[modlog] falha ao avisar '{title}': {ex.Message}");
            }
        }

        private static async Task WriteAsync(
            DiscordClient client,
            ulong guildId,
            string action,
            // Nullable: ha acoes que nao tem alvo, como o /purge sem filtro de
            // usuario. Antes o chamador passava o proprio moderador nesse caso,
            // e o embed saia dizendo que ele agira contra si mesmo.
            DiscordUser? target,
            DiscordUser moderator,
            string? reason,
            string? extra = null,
            string noTargetLabel = "*entire channel*")
        {
            // O canal e o DAQUELE servidor. Antes era um id unico do processo, e
            // como o GetChannelAsync resolve id em qualquer servidor onde o bot
            // esteja, o /ban de um servidor ia parar no log de outro. A guarda
            // BelongsToGuild abaixo continua, agora como defesa em profundidade:
            // o dono ainda pode digitar um id de fora no /config.
            var target2 = GuildSettingsStore.For(guildId)?.moderationLogChannelId;
            if (target2 is not > 0)
                return;

            try
            {
                var channel = await client.GetChannelAsync(target2.Value);
                if (!BelongsToGuild(channel, guildId, action))
                    return;

                var embed = new DiscordEmbedBuilder()
                    .WithTitle($"Moderation — {action}")
                    .WithColor(DiscordColor.Orange)
                    .WithTimestamp(DateTimeOffset.UtcNow)
                    .AddField(new DiscordEmbedField("User",
                        target is null ? noTargetLabel : $"{target.Mention}\n`{target.Id}`", true))
                    .AddField(new DiscordEmbedField("Moderator", $"{moderator.Mention}\n`{moderator.Id}`", true))
                    .AddField(new DiscordEmbedField("Reason",
                        string.IsNullOrWhiteSpace(reason) ? "*no reason given*" : Embeds.SafeTrim(reason, 1000), false));

                if (target is not null)
                    embed.WithThumbnail(target.AvatarUrl);

                if (!string.IsNullOrWhiteSpace(extra))
                    embed.AddField(new DiscordEmbedField("Details", Embeds.SafeTrim(extra, 1000), false));

                await channel.SendMessageAsync(new DiscordMessageBuilder().AddEmbed(embed));
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[modlog] falha ao registrar '{action}': {ex.Message}");
            }
        }
    }
}
