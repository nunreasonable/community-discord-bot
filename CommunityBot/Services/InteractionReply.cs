using System;
using System.Threading.Tasks;
using DisCatSharp.ApplicationCommands.Context;
using DisCatSharp.Entities;
using DisCatSharp.Exceptions;

namespace CommunityBot.Services
{
    /// <summary>
    /// Resposta a uma interacao que pode demorar mais do que o token dela vive.
    ///
    /// O token de uma interacao vale 15 minutos. Comandos que mandam DM para um
    /// cargo inteiro (3s de intervalo por pessoa) ou que conversam por DM antes
    /// de concluir passam disso com facilidade, e ai o EditResponse final morre
    /// com NotFound - o usuario fica sem resposta nenhuma, exatamente como
    /// acontecia com os botoes de paginacao.
    ///
    /// Aqui a resposta cai para uma mensagem no canal, marcando quem rodou o
    /// comando, em vez de sumir.
    /// </summary>
    internal static class InteractionReply
    {
        public static Task SafeEditAsync(InteractionContext ctx, DiscordEmbed embed) =>
            SafeEditAsync(ctx, new DiscordWebhookBuilder().AddEmbed(embed), embed, null);

        public static Task SafeEditAsync(InteractionContext ctx, string content) =>
            SafeEditAsync(ctx, new DiscordWebhookBuilder().WithContent(content), null, content);

        private static async Task SafeEditAsync(InteractionContext ctx, DiscordWebhookBuilder builder, DiscordEmbed? embed, string? content)
        {
            try
            {
                await ctx.EditResponseAsync(builder);
            }
            catch (Exception ex) when (ex is NotFoundException or UnauthorizedException)
            {
                // Token vencido ou resposta original apagada: o canal e o unico
                // caminho que sobra.
                try
                {
                    var message = new DiscordMessageBuilder()
                        .WithContent(content is null ? ctx.User.Mention : $"{ctx.User.Mention} {content}");

                    if (embed is not null)
                        message.AddEmbed(embed);

                    await ctx.Channel.SendMessageAsync(message);
                }
                catch (Exception fallbackEx)
                {
                    Console.WriteLine($"[interacao] resposta perdida (token vencido e canal indisponivel): {fallbackEx.Message}");
                }
            }
        }
    }
}
