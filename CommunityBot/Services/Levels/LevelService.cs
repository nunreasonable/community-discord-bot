using System;
using System.Collections.Concurrent;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using DisCatSharp;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.Enums.Core;
using DisCatSharp.EventArgs;
using DisCatSharp.Exceptions;

namespace CommunityBot.Services.Levels
{
    /// <summary>
    /// Ganho de XP por mensagem e o aviso de level up por DM.
    ///
    /// O bot continua sem o intent de Message Content: o XP conta QUE a pessoa
    /// escreveu, quando e onde, e nunca le o que ela escreveu. Por isso nao ha
    /// "XP por tamanho de mensagem" - nao ha tamanho nenhum para medir.
    /// </summary>
    internal static class LevelService
    {
        public const string Prefix = "lvl:";
        public const string DmYesId = Prefix + "dm:yes";
        public const string DmNoId = Prefix + "dm:no";

        // Um ganho por minuto por pessoa por servidor, como no MEE6: e o que faz
        // floodar o chat nao valer a pena.
        private static readonly long s_cooldownMs = (long)TimeSpan.FromSeconds(60).TotalMilliseconds;
        private const int MinGain = 15;
        private const int MaxGain = 25;

        private static readonly ConcurrentDictionary<(ulong Guild, ulong User), long> s_lastGain = new();

        // Limpa as entradas vencidas do cooldown de tempos em tempos, em vez de a
        // cada mensagem - varrer o dicionario inteiro no caminho do gateway seria
        // pior que a memoria que ele ocupa.
        private static readonly Timer s_prune = new(_ => PruneCooldowns(), null,
            TimeSpan.FromMinutes(5), TimeSpan.FromMinutes(5));

        public static Task OnMessageCreated(DiscordClient client, MessageCreateEventArgs e)
        {
            // Tudo sincrono e sem I/O: roda no caminho do gateway.
            if (e.Guild is not { } guild || e.Author is not { } author || author.IsBot || e.Message.WebhookMessage)
                return Task.CompletedTask;

            // Entrada no servidor, pin, boost: mensagem de sistema nao e conversa.
            if (e.Message.MessageType is not (MessageType.Default or MessageType.Reply))
                return Task.CompletedTask;

            if (GuildSettingsStore.For(guild.Id)?.levelingEnabled != true)
                return Task.CompletedTask;

            var key = (guild.Id, author.Id);
            var now = Environment.TickCount64;
            if (s_lastGain.TryGetValue(key, out var last) && now - last < s_cooldownMs)
                return Task.CompletedTask;
            s_lastGain[key] = now;

            var (oldXp, newXp) = LevelStore.Instance.AddXp(guild.Id, author.Id, Random.Shared.Next(MinGain, MaxGain + 1));
            var newLevel = LevelMath.LevelOf(newXp);

            if (newLevel > LevelMath.LevelOf(oldXp))
                _ = Task.Run(() => NotifyLevelUpAsync(guild, author, newLevel, byModerator: false));

            return Task.CompletedTask;
        }

        /// <summary>
        /// A DM de level up, com a pergunta de se a pessoa quer continuar
        /// recebendo. Quem respondeu No nao recebe mais - nem por XP dado por
        /// moderador.
        /// </summary>
        public static async Task NotifyLevelUpAsync(DiscordGuild guild, DiscordUser user, int level, bool byModerator)
        {
            if (!LevelStore.Instance.DmEnabled(user.Id))
                return;

            var embed = new DiscordEmbedBuilder()
                .WithTitle("🎉 Level up!")
                .WithDescription($"You reached **level {level}** in **{Embeds.SafeTrim(guild.Name, 100)}**" +
                                 (byModerator ? " (XP given by a moderator)." : ".") +
                                 "\n\nWould you like to keep receiving these DMs?")
                .WithColor(DiscordColor.Gold);

            if (guild.IconUrl is { } icon)
                embed.WithThumbnail(icon);

            try
            {
                await user.SendMessageAsync(new DiscordMessageBuilder()
                    .AddEmbed(embed)
                    .AddComponents(
                        new DiscordButtonComponent(ButtonStyle.Success, DmYesId, "Yes"),
                        new DiscordButtonComponent(ButtonStyle.Secondary, DmNoId, "No")));
            }
            catch (Exception ex) when (ex is UnauthorizedException or BadRequestException)
            {
                // DM fechada ou sem servidor em comum: comum demais para virar
                // linha de log a cada level up.
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[niveis] aviso: nao consegui mandar a DM de level up para {user.Id}: {ex.Message}");
            }
        }

        private static void PruneCooldowns()
        {
            try
            {
                var cutoff = Environment.TickCount64 - s_cooldownMs;
                foreach (var key in s_lastGain.Where(kv => kv.Value < cutoff).Select(kv => kv.Key).ToList())
                    s_lastGain.TryRemove(key, out _);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[niveis] aviso: limpeza do cooldown falhou: {ex.Message}");
            }
        }

        /// <summary>Botoes Yes/No da DM. Handler proprio, Handled so para "lvl:".</summary>
        public static Task OnComponent(DiscordClient client, ComponentInteractionCreateEventArgs e)
        {
            var id = e.Interaction.Data?.CustomId ?? e.Id;
            if (string.IsNullOrEmpty(id) || !id.StartsWith(Prefix, StringComparison.Ordinal))
                return Task.CompletedTask;

            e.Handled = true;

            var interaction = e.Interaction;
            var message = e.Message;
            var userId = e.User.Id;
            _ = Task.Run(async () =>
            {
                try
                {
                    var keep = id == DmYesId;
                    if (!keep && id != DmNoId)
                    {
                        Console.WriteLine($"[niveis] custom id desconhecido: '{id}'");
                        return;
                    }

                    LevelStore.Instance.SetDm(userId, keep);

                    // Edita a propria DM: tira os botoes e diz o que ficou valendo.
                    var original = message?.Embeds.FirstOrDefault();
                    var note = keep
                        ? "👍 Got it — you'll keep getting a DM when you level up."
                        : "🔕 Got it — no more level-up DMs. Turn them back on anytime with `/level-dms`.";

                    // Replace, e nao Update: a mensagem nova sai sem os botoes, e
                    // ninguem clica de novo numa pergunta ja respondida.
                    await interaction.CreateResponseAsync(InteractionResponseType.DeferredMessageUpdate);

                    var builder = new DiscordWebhookBuilder();
                    if (original is not null)
                        builder.AddEmbed(new DiscordEmbedBuilder(original)
                            .WithDescription(Embeds.Trim((original.Description ?? string.Empty)
                                .Replace("Would you like to keep receiving these DMs?", note), 4000)));
                    else
                        builder.WithContent(note);

                    await interaction.EditOriginalResponseAsync(builder, ModifyMode.Replace);
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"[niveis] falha ao tratar '{id}' de {userId}: {ex.Message}");
                }
            });

            return Task.CompletedTask;
        }
    }
}
