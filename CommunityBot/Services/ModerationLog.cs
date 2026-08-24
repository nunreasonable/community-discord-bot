using System;
using System.Threading.Tasks;
using CommunityBot.config;
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
        /// MESMA resposta original - o moderador via "Falha no comando" numa
        /// punicao que tinha sido aplicada. Trazendo a leitura para dentro deste
        /// metodo, ela passa a ser coberta pelo mesmo catch do resto.
        /// </summary>
        public static async Task RecordAsync(
            DiscordClient client,
            string action,
            DiscordUser? target,
            DiscordUser moderator,
            string? reason,
            string? extra = null)
        {
            try
            {
                var config = new JSONReader();
                await config.ReadJSON();
                await RecordAsync(client, config, action, target, moderator, reason, extra);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[modlog] falha ao ler o config para registrar '{action}': {ex.Message}");
            }
        }

        public static async Task RecordAsync(
            DiscordClient client,
            JSONReader config,
            string action,
            // Nullable: ha acoes que nao tem alvo, como o /purge sem filtro de
            // usuario. Antes o chamador passava o proprio moderador nesse caso,
            // e o embed saia dizendo que ele agira contra si mesmo.
            DiscordUser? target,
            DiscordUser moderator,
            string? reason,
            string? extra = null)
        {
            if (!config.moderationLogChannelId.HasValue || config.moderationLogChannelId.Value == 0)
                return;

            try
            {
                var channel = await client.GetChannelAsync(config.moderationLogChannelId.Value);
                if (channel is null)
                    return;

                var embed = new DiscordEmbedBuilder()
                    .WithTitle($"Moderação — {action}")
                    .WithColor(DiscordColor.Orange)
                    .WithTimestamp(DateTimeOffset.UtcNow)
                    .AddField(new DiscordEmbedField("Usuário",
                        target is null ? "*toda a conversa do canal*" : $"{target.Mention}\n`{target.Id}`", true))
                    .AddField(new DiscordEmbedField("Moderador", $"{moderator.Mention}\n`{moderator.Id}`", true))
                    .AddField(new DiscordEmbedField("Motivo",
                        string.IsNullOrWhiteSpace(reason) ? "*não informado*" : Embeds.Trim(reason, 1000), false));

                if (target is not null)
                    embed.WithThumbnail(target.AvatarUrl);

                if (!string.IsNullOrWhiteSpace(extra))
                    embed.AddField(new DiscordEmbedField("Detalhes", Embeds.Trim(extra, 1000), false));

                await channel.SendMessageAsync(new DiscordMessageBuilder().AddEmbed(embed));
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[modlog] falha ao registrar '{action}': {ex.Message}");
            }
        }
    }
}
