using System;
using System.Linq;
using System.Security.Cryptography;
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
    /// <summary>
    /// Comandos de diversão.
    ///
    /// Todos têm cooldown: é o que impede um comando leve de virar ferramenta de
    /// flood num servidor grande. O cooldown é por usuário.
    /// </summary>
    internal class Fun : ApplicationCommandsModule
    {
        private static readonly string[] s_eightBall =
        {
            "Com certeza.", "É decidido que sim.", "Sem dúvida.", "Sim, definitivamente.",
            "Pode contar com isso.", "Pelo que vejo, sim.", "Provavelmente.", "Tudo indica que sim.",
            "Resposta nebulosa, tente de novo.", "Pergunte mais tarde.", "Melhor não te dizer agora.",
            "Não dá para prever agora.", "Concentre-se e pergunte de novo.",
            "Não conte com isso.", "Minha resposta é não.", "Minhas fontes dizem que não.",
            "As perspectivas não são boas.", "Muito duvidoso."
        };

        [SlashCommand("8ball", "Faz uma pergunta à bola oito")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task EightBallCommand(
            InteractionContext ctx,
            [Option("pergunta", "O que você quer saber")] string question)
        {
            var answer = s_eightBall[RandomNumberGenerator.GetInt32(s_eightBall.Length)];

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(new DiscordEmbedBuilder()
                    .WithTitle("🎱 Bola oito")
                    .AddField(new DiscordEmbedField("Pergunta", Embeds.Trim(question, 900), false))
                    .AddField(new DiscordEmbedField("Resposta", answer, false))
                    .WithColor(DiscordColor.Blurple)));
        }

        [SlashCommand("roll", "Rola dados no formato NdM")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task RollCommand(
            InteractionContext ctx,
            [Option("dados", "Ex.: 2d6, d20, 4d10")] string dados = "1d6")
        {
            if (!DiceRoller.TryRoll(dados, out var result, out var error))
            {
                await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                    new DiscordInteractionResponseBuilder().AddEmbed(Embeds.Error("Dados inválidos", error!)).AsEphemeral());
                return;
            }

            // Com muitos dados a lista estoura o embed; acima de 20 só o total
            // interessa mesmo.
            var detail = result.Rolls.Count <= 20
                ? string.Join(" + ", result.Rolls)
                : $"{result.Rolls.Count} dados";

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(new DiscordEmbedBuilder()
                    .WithTitle($"🎲 {result.Count}d{result.Sides}")
                    .WithDescription($"{detail}\n\n**Total: {result.Total}**")
                    .WithColor(DiscordColor.Blurple)));
        }

        [SlashCommand("coinflip", "Cara ou coroa")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task CoinflipCommand(InteractionContext ctx)
        {
            var heads = RandomNumberGenerator.GetInt32(2) == 0;

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(new DiscordEmbedBuilder()
                    .WithTitle(heads ? "🪙 Cara" : "🪙 Coroa")
                    .WithColor(DiscordColor.Gold)));
        }

        [SlashCommand("choose", "Escolhe uma das opções que você der")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task ChooseCommand(
            InteractionContext ctx,
            [Option("opcoes", "Separadas por vírgula ou ponto e vírgula")] string opcoes)
        {
            // Aceita os dois separadores: quem escreve uma lista costuma usar
            // virgula, mas a opcao em si pode conter virgula.
            var separators = opcoes.Contains(';') ? new[] { ';' } : new[] { ',' };

            var options = opcoes
                .Split(separators, StringSplitOptions.RemoveEmptyEntries)
                .Select(o => o.Trim())
                .Where(o => o.Length > 0)
                .ToList();

            if (options.Count < 2)
            {
                await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                    new DiscordInteractionResponseBuilder().AddEmbed(Embeds.Error("Poucas opções",
                        "Dê pelo menos duas opções, separadas por vírgula. Ex.: `pizza, sushi, hambúrguer`")).AsEphemeral());
                return;
            }

            var picked = options[RandomNumberGenerator.GetInt32(options.Count)];

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(new DiscordEmbedBuilder()
                    .WithTitle("🤔 Escolhi")
                    .WithDescription($"**{Embeds.Trim(picked, 200)}**")
                    .WithFooter($"de {options.Count} opções")
                    .WithColor(DiscordColor.Blurple)));
        }

        [SlashCommand("avatar", "Mostra o avatar de um usuário em tamanho grande")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task AvatarCommand(
            InteractionContext ctx,
            [Option("usuario", "De quem ver o avatar")] DiscordUser? usuario = null)
        {
            var target = usuario ?? ctx.User;
            var url = target.GetAvatarUrl(MediaFormat.Auto, 1024);

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(new DiscordEmbedBuilder()
                    .WithTitle($"Avatar de {target.UsernameWithDiscriminator}")
                    .WithImageUrl(url)
                    .WithUrl(url)
                    .WithColor(DiscordColor.Blurple)));
        }

        [SlashCommand("say", "Faz o bot repetir um texto")]
        // Exige ManageMessages: sem isso qualquer um faria o bot falar, e mensagem
        // vinda do bot tem aparencia de coisa oficial.
        [ApplicationCommandRequireUserPermissions(Permissions.ManageMessages)]
        [ApplicationCommandRequireGuild]
        public async Task SayCommand(
            InteractionContext ctx,
            [Option("texto", "O que o bot vai dizer")] string texto)
        {
            // Sem mencao de cargo nem de @everyone: o /say seria um jeito de
            // contornar quem pode mencionar todo mundo.
            var builder = new DiscordMessageBuilder()
                .WithContent(Embeds.Trim(texto, 1900))
                .WithAllowedMentions(Array.Empty<IMention>());

            await ctx.Channel.SendMessageAsync(builder);

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(
                    Embeds.Ok("Enviado", "A mensagem foi publicada no canal.")).AsEphemeral());
        }
    }
}
