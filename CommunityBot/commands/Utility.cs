using System;
using System.Linq;
using System.Threading.Tasks;
using CommunityBot.Services;
using DisCatSharp;
using DisCatSharp.ApplicationCommands;
using DisCatSharp.ApplicationCommands.Attributes;
using DisCatSharp.ApplicationCommands.Context;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.Enums.Core;

namespace CommunityBot.commands
{
    internal class Utility : ApplicationCommandsModule
    {
        [SlashCommand("ping", "Mostra a latência do bot")]
        public async Task PingCommand(InteractionContext ctx)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(new DiscordEmbedBuilder()
                    .WithTitle("🏓 Pong")
                    .AddField(new DiscordEmbedField("Gateway", $"{ctx.Client.Ping} ms", true))
                    .WithColor(DiscordColor.Blurple)));
        }

        [SlashCommand("userinfo", "Mostra informações de um usuário")]
        [ApplicationCommandRequireGuild]
        public async Task UserInfoCommand(
            InteractionContext ctx,
            [Option("usuario", "De quem ver as informações")] DiscordUser? usuario = null)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource);

            var target = usuario ?? ctx.User;

            var embed = new DiscordEmbedBuilder()
                .WithTitle(target.UsernameWithDiscriminator)
                .WithThumbnail(target.GetAvatarUrl(MediaFormat.Auto, 256))
                .WithColor(DiscordColor.Blurple)
                .AddField(new DiscordEmbedField("ID", $"`{target.Id}`", true))
                .AddField(new DiscordEmbedField("Conta criada",
                    $"<t:{target.CreationTimestamp.ToUnixTimeSeconds()}:D>", true))
                .AddField(new DiscordEmbedField("Bot", target.IsBot ? "Sim" : "Não", true));

            DiscordMember? member = null;
            try
            {
                member = await ctx.Guild!.GetMemberAsync(target.Id);
            }
            catch
            {
                // Usuario fora do servidor: os campos de membro simplesmente nao
                // aparecem, em vez de o comando falhar.
            }

            if (member is not null)
            {
                embed.AddField(new DiscordEmbedField("Entrou no servidor",
                    member.JoinedAt == default ? "—" : $"<t:{member.JoinedAt.ToUnixTimeSeconds()}:D>", true));

                // Ordem decrescente e sem o @everyone, que todo mundo tem e nao
                // informa nada.
                var everyoneId = ctx.Guild!.EveryoneRole?.Id ?? 0;
                var roles = member.Roles
                    .Where(r => r.Id != everyoneId)
                    .OrderByDescending(r => r.Position)
                    .Select(r => r.Mention)
                    .ToList();

                embed.AddField(new DiscordEmbedField($"Cargos ({roles.Count})",
                    roles.Count == 0 ? "*nenhum*" : Embeds.Trim(string.Join(" ", roles), 1000), false));

                if (member.CommunicationDisabledUntil is { } mutedUntil && mutedUntil > DateTime.UtcNow)
                {
                    var until = new DateTimeOffset(DateTime.SpecifyKind(mutedUntil, DateTimeKind.Utc));
                    embed.AddField(new DiscordEmbedField("Silenciado até",
                        $"<t:{until.ToUnixTimeSeconds()}:f>", true));
                }
            }
            else
            {
                embed.WithFooter("Este usuário não está no servidor.");
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(embed));
        }

        [SlashCommand("serverinfo", "Mostra informações do servidor")]
        [ApplicationCommandRequireGuild]
        public async Task ServerInfoCommand(InteractionContext ctx)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource);

            var guild = ctx.Guild!;

            // Em locais tipados: as coleções e o enum são nullable no contrato do
            // DisCatSharp, e o embed não aceita valor nulo.
            var members = guild.MemberCount.ToString() ?? "—";
            var roleCount = (guild.Roles?.Count ?? 0).ToString();
            var channelCount = (guild.Channels?.Count ?? 0).ToString();
            var boost = guild.PremiumTier.ToString() ?? "—";

            var embed = new DiscordEmbedBuilder()
                .WithTitle(guild.Name)
                .WithColor(DiscordColor.Blurple)
                .AddField(new DiscordEmbedField("ID", $"`{guild.Id}`", true))
                .AddField(new DiscordEmbedField("Criado em",
                    $"<t:{guild.CreationTimestamp.ToUnixTimeSeconds()}:D>", true))
                .AddField(new DiscordEmbedField("Dono", $"<@{guild.OwnerId}>", true))
                .AddField(new DiscordEmbedField("Membros", members, true))
                .AddField(new DiscordEmbedField("Cargos", roleCount, true))
                .AddField(new DiscordEmbedField("Canais", channelCount, true))
                .AddField(new DiscordEmbedField("Nível de boost", boost, true));

            if (!string.IsNullOrWhiteSpace(guild.IconUrl))
                embed.WithThumbnail(guild.IconUrl);

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(embed));
        }

        [SlashCommand("poll", "Cria uma enquete de sim/não com botões")]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(2, 30, CooldownBucketType.User)]
        public async Task PollCommand(
            InteractionContext ctx,
            [Option("pergunta", "O que está sendo perguntado")] string pergunta)
        {
            // Os votos vivem nas reacoes, nao no processo: uma enquete que morre
            // ao reiniciar o bot nao serve para nada.
            var embed = new DiscordEmbedBuilder()
                .WithTitle("📊 Enquete")
                .WithDescription(Embeds.Trim(pergunta, 1500))
                .WithColor(DiscordColor.Blurple)
                .WithFooter($"Criada por {ctx.User.UsernameWithDiscriminator}");

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(embed));

            var message = await ctx.GetOriginalResponseAsync();
            await message.CreateReactionAsync(DiscordEmoji.FromUnicode("👍"));
            await message.CreateReactionAsync(DiscordEmoji.FromUnicode("👎"));
        }

        [SlashCommand("help", "Lista os comandos disponíveis")]
        public async Task HelpCommand(InteractionContext ctx)
        {
            var embed = new DiscordEmbedBuilder()
                .WithTitle("Comandos")
                .WithDescription("Os comandos de moderação só aparecem para quem tem a permissão correspondente no servidor.")
                .WithColor(DiscordColor.Blurple)
                .AddField(new DiscordEmbedField("Moderação",
                    "`/ban` `/kick` `/timeout` `/untimeout` `/purge` `/slowmode` `/lock` `/unlock`", false))
                .AddField(new DiscordEmbedField("Advertências",
                    "`/warn` `/warnings` `/delwarn`", false))
                .AddField(new DiscordEmbedField("Diversão",
                    "`/8ball` `/roll` `/coinflip` `/choose` `/avatar` `/say`", false))
                .AddField(new DiscordEmbedField("Utilidade",
                    "`/ping` `/userinfo` `/serverinfo` `/poll` `/help` `/logs`", false));

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(embed).AsEphemeral());
        }
    }
}
