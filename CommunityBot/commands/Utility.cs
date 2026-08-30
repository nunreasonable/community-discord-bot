using System;
using System.Linq;
using System.Threading.Tasks;
using CommunityBot.Services;
using DisCatSharp;
using DisCatSharp.ApplicationCommands;
using DisCatSharp.ApplicationCommands.Attributes;
using DisCatSharp.ApplicationCommands.Context;
using DisCatSharp.Entities;
using DisCatSharp.Exceptions;
using DisCatSharp.Enums;
using DisCatSharp.Enums.Core;

namespace CommunityBot.commands
{
    // SEM ApplicationCommandRequireGuild no nivel da classe, de proposito: /ping
    // e /help funcionam em DM e nao tocam em ctx.Guild. Os daqui que PRECISAM de
    // servidor - /userinfo, /serverinfo, /poll - carregam o atributo no proprio
    // metodo, e e assim que tem de continuar: ver a explicacao em BotLogs.cs
    // sobre RequireUserPermissions ter IgnoreDms = true.
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
            catch (NotFoundException)
            {
                // SO o "usuario nao esta no servidor". O catch sem filtro que
                // estava aqui engolia tambem rate limit, 5xx e cancelamento, e
                // todos viravam a mesma afirmacao falsa no rodape ("nao esta
                // neste servidor") - sem deixar nada no BotLogBuffer para o
                // /logs diagnosticar.
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

                // ToUniversalTime em vez de SpecifyKind: o SpecifyKind RE-ROTULA o
                // valor sem converter, entao um DateTime que chegasse com Kind
                // Local seria tratado como se ja fosse UTC e o horario exibido
                // sairia deslocado pelo fuso da maquina. ToUniversalTime esta
                // correto para qualquer Kind.
                if (member.CommunicationDisabledUntil is { } mutedUntil
                    && mutedUntil.ToUniversalTime() > DateTime.UtcNow)
                {
                    var until = new DateTimeOffset(mutedUntil.ToUniversalTime(), TimeSpan.Zero);
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

            // MemberCount e `int?` e PremiumTier e `PremiumTier?`. O `?.` antes do
            // ToString e o que importa: `Nullable<T>.ToString()` devolve STRING
            // VAZIA quando nao ha valor, nunca null, entao o `?? "—"` de antes era
            // codigo morto - e o campo saia vazio. O Discord recusa campo de embed
            // com valor vazio, entao o /serverinfo inteiro morria em "Falha no
            // comando" sempre que um dos dois nao viesse preenchido.
            //
            // (O comentario antigo dizia que tirar os `??` daria CS8604. Isso vale
            // para as colecoes abaixo, que sao de tipo referencia; para os dois
            // nullables de valor, nao.)
            var members = guild.MemberCount?.ToString() ?? "—";
            var roleCount = (guild.Roles?.Count ?? 0).ToString();
            var channelCount = (guild.Channels?.Count ?? 0).ToString();
            var boost = guild.PremiumTier?.ToString() ?? "—";

            var embed = new DiscordEmbedBuilder()
                .WithTitle(guild.Name)
                .WithColor(DiscordColor.Blurple)
                .AddField(new DiscordEmbedField("ID", $"`{guild.Id}`", true))
                .AddField(new DiscordEmbedField("Criado em",
                    $"<t:{guild.CreationTimestamp.ToUnixTimeSeconds()}:D>", true))
                // OwnerId tambem e `ulong?`, como MemberCount e PremiumTier: com
                // ele nulo o campo saia como um "<@>" literal, que nao e mencao
                // nem nome.
                .AddField(new DiscordEmbedField("Dono",
                    guild.OwnerId is { } ownerId ? $"<@{ownerId}>" : "—", true))
                .AddField(new DiscordEmbedField("Membros", members, true))
                .AddField(new DiscordEmbedField("Cargos", roleCount, true))
                .AddField(new DiscordEmbedField("Canais", channelCount, true))
                .AddField(new DiscordEmbedField("Nível de boost", boost, true));

            if (!string.IsNullOrWhiteSpace(guild.IconUrl))
                embed.WithThumbnail(guild.IconUrl);

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(embed));
        }

        [SlashCommand("poll", "Cria uma enquete de sim/não com reações")]
        [ApplicationCommandRequireGuild]
        [ApplicationCommandRequireBotPermissions(Permissions.AddReactions | Permissions.ReadMessageHistory)]
        [SlashCommandCooldown(2, 30, CooldownBucketType.User)]
        public async Task PollCommand(
            InteractionContext ctx,
            [Option("pergunta", "O que está sendo perguntado")] string pergunta)
        {
            // Os votos vivem nas reacoes, nao no processo: uma enquete que morre
            // ao reiniciar o bot nao serve para nada.
            var embed = new DiscordEmbedBuilder()
                .WithTitle("📊 Enquete")
                .WithDescription(Embeds.SafeTrim(pergunta, 1500))
                .WithColor(DiscordColor.Blurple)
                .WithFooter($"Criada por {ctx.User.UsernameWithDiscriminator}");

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(embed));

            // Falha ao reagir NAO pode escapar daqui.
            //
            // A enquete ja foi publicada neste ponto. Se a excecao subisse, o
            // SlashCommandErrored chamaria EditResponseAsync e substituiria o
            // embed da enquete pelo embed de erro - a enquete simplesmente
            // desaparecia por causa de uma reacao que nao foi.
            try
            {
                var message = await ctx.GetOriginalResponseAsync();
                await message.CreateReactionAsync(DiscordEmoji.FromUnicode("👍"));
                await message.CreateReactionAsync(DiscordEmoji.FromUnicode("👎"));
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[poll] nao consegui reagir na enquete: {ex.Message}");
            }
        }

        [SlashCommand("help", "Lista os comandos disponíveis")]
        public async Task HelpCommand(InteractionContext ctx)
        {
            var embed = new DiscordEmbedBuilder()
                .WithTitle("Comandos")
                .WithDescription("Os comandos de moderação só aparecem para quem tem a permissão correspondente no servidor.")
                .WithColor(DiscordColor.Blurple)
                .AddField(new DiscordEmbedField("Moderação",
                    "`/ban` `/softban` `/kick` `/timeout` `/untimeout` `/purge` `/slowmode` `/lock` `/unlock`", false))
                .AddField(new DiscordEmbedField("Advertências",
                    "`/warn` `/warnings` `/delwarn`", false))
                .AddField(new DiscordEmbedField("Tickets",
                    "`/ticket` `/ticket-fechar` `/ticket-add` `/ticket-remove` `/ticket-painel`", false))
                .AddField(new DiscordEmbedField("Diversão",
                    "`/8ball` `/roll` `/coinflip` `/choose` `/avatar` `/say`", false))
                .AddField(new DiscordEmbedField("Configuração",
                    "`/config ver` `/config log-moderacao` `/config auto-softban` " +
                    "`/config tickets-categoria` `/config tickets-cargo` `/config tickets-log`", false))
                // Sem /logs: ele e de quem administra o BOT, nao de quem usa o
                // servidor, e anunciar um comando que so uma pessoa no mundo pode
                // rodar so rende tentativa recusada.
                .AddField(new DiscordEmbedField("Utilidade",
                    "`/ping` `/userinfo` `/serverinfo` `/poll` `/help`", false));

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(embed).AsEphemeral());
        }
    }
}
