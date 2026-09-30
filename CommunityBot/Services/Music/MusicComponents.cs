using System;
using System.Threading.Tasks;
using DisCatSharp;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.EventArgs;

namespace CommunityBot.Services.Music
{
    /// <summary>
    /// Botoes do aviso de "Now playing". Mesmo esquema dos tickets: handler
    /// proprio, Handled marcado so para custom id "music:".
    ///
    /// Os botoes passam pelas MESMAS acoes dos comandos (MusicActions), com as
    /// mesmas regras: o aviso fica no canal a vista de todos, e um botao que
    /// pulasse a votacao seria o atalho obvio para contorna-la.
    /// </summary>
    internal static class MusicComponents
    {
        public static Task OnComponent(DiscordClient client, ComponentInteractionCreateEventArgs e)
        {
            var id = e.Interaction.Data?.CustomId ?? e.Id;
            if (string.IsNullOrEmpty(id) || !id.StartsWith(MusicFormat.Prefix, StringComparison.Ordinal))
                return Task.CompletedTask;

            // Sincrono, antes de qualquer await: o despachante so olha Handled
            // depois que este metodo retorna.
            e.Handled = true;

            var interaction = e.Interaction;
            _ = Task.Run(async () =>
            {
                try
                {
                    await RouteAsync(e, id, interaction);
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"[musica] falha ao tratar '{id}' de {e.User.Id}: {ex}");
                    await TryFailAsync(interaction, "Something broke while handling that button.");
                }
            });

            return Task.CompletedTask;
        }

        private static async Task RouteAsync(ComponentInteractionCreateEventArgs e, string id, DiscordInteraction interaction)
        {
            if (e.Guild is not { } guild || e.Member is not { } member)
            {
                await TryFailAsync(interaction, "This only works inside a server.");
                return;
            }

            // O ultimo aviso de uma faixa que ja acabou perde os botoes, mas um
            // clique pode chegar no intervalo - ou o bot pode ter reiniciado.
            var player = MusicService.Get(guild.Id);
            if (player is null)
            {
                await RespondAsync(interaction, MusicReply.Private(
                    Embeds.Info("Nothing is playing", "These buttons belong to a song that already ended.")));
                return;
            }

            MusicReply reply;
            switch (id)
            {
                case MusicFormat.PauseId:
                    reply = await MusicActions.SetPausedAsync(member, guild.Id, null);
                    break;
                case MusicFormat.SkipId:
                    reply = MusicActions.Skip(member, guild.Id);
                    break;
                case MusicFormat.StopId:
                    reply = MusicActions.Stop(member, guild.Id);
                    break;
                case MusicFormat.QueueId:
                    // Consulta: so quem clicou precisa ver.
                    reply = MusicActions.Queue(guild.Id, 1) with { Ephemeral = true };
                    break;
                default:
                    Console.WriteLine($"[musica] custom id desconhecido: '{id}'");
                    reply = MusicReply.Private(Embeds.Error("Unknown button", "This button is from an older version of the bot."));
                    break;
            }

            await RespondAsync(interaction, reply);
        }

        private static async Task RespondAsync(DiscordInteraction interaction, MusicReply reply)
        {
            var builder = new DiscordInteractionResponseBuilder()
                .AddEmbed(reply.Embed)
                .WithAllowedMentions(Array.Empty<IMention>());

            if (reply.Ephemeral)
                builder.AsEphemeral();

            await interaction.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource, builder);
        }

        private static async Task TryFailAsync(DiscordInteraction interaction, string message)
        {
            try
            {
                await interaction.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                    new DiscordInteractionResponseBuilder().AddEmbed(Embeds.Error("Error", message)).AsEphemeral());
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[musica] nao consegui avisar a falha do botao: {ex.Message}");
            }
        }
    }
}
