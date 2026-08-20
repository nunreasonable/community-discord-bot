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
        public static async Task RecordAsync(
            DiscordClient client,
            JSONReader config,
            string action,
            DiscordUser target,
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
                    .WithThumbnail(target.AvatarUrl)
                    .AddField(new DiscordEmbedField("Usuário", $"{target.Mention}\n`{target.Id}`", true))
                    .AddField(new DiscordEmbedField("Moderador", $"{moderator.Mention}\n`{moderator.Id}`", true))
                    .AddField(new DiscordEmbedField("Motivo",
                        string.IsNullOrWhiteSpace(reason) ? "*não informado*" : Embeds.Trim(reason, 1000), false));

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
