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
    // SEM ApplicationCommandRequireGuild no nivel da classe, de proposito:
    // /8ball, /roll, /coinflip, /choose e /avatar funcionam em DM e nao tocam em
    // ctx.Guild. Todo comando daqui que PRECISA de servidor - ou que e
    // privilegiado, como o /say - carrega o atributo no proprio metodo, e e assim
    // que tem de continuar: ver a explicacao em BotLogs.cs sobre
    // RequireUserPermissions ter IgnoreDms = true.
    internal class Fun : ApplicationCommandsModule
    {
        private static readonly string[] s_eightBall =
        {
            "It is certain.", "It is decidedly so.", "Without a doubt.", "Yes, definitely.",
            "You may rely on it.", "As I see it, yes.", "Most likely.", "Signs point to yes.",
            "Reply hazy, try again.", "Ask again later.", "Better not tell you now.",
            "Cannot predict now.", "Concentrate and ask again.",
            "Don't count on it.", "My reply is no.", "My sources say no.",
            "Outlook not so good.", "Very doubtful."
        };

        [SlashCommand("8ball", "Ask the magic 8-ball a question")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task EightBallCommand(
            InteractionContext ctx,
            [Option("question", "What you want to know")] string question)
        {
            var answer = s_eightBall[RandomNumberGenerator.GetInt32(s_eightBall.Length)];

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(new DiscordEmbedBuilder()
                    .WithTitle("🎱 Magic 8-ball")
                    .AddField(new DiscordEmbedField("Question", Embeds.SafeTrim(question, 900), false))
                    .AddField(new DiscordEmbedField("Answer", answer, false))
                    .WithColor(DiscordColor.Blurple)));
        }

        [SlashCommand("roll", "Roll dice in NdM format")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task RollCommand(
            InteractionContext ctx,
            [Option("dice", "E.g. 2d6, d20, 4d10")] string dados = "1d6")
        {
            if (!DiceRoller.TryRoll(dados, out var result, out var error))
            {
                await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                    new DiscordInteractionResponseBuilder().AddEmbed(Embeds.Error("Invalid dice", error!)).AsEphemeral());
                return;
            }

            // Com muitos dados a lista estoura o embed; acima de 20 só o total
            // interessa mesmo.
            var detail = result.Rolls.Count <= 20
                ? string.Join(" + ", result.Rolls)
                : $"{result.Rolls.Count} dice";

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(new DiscordEmbedBuilder()
                    .WithTitle($"🎲 {result.Count}d{result.Sides}")
                    .WithDescription($"{detail}\n\n**Total: {result.Total}**")
                    .WithColor(DiscordColor.Blurple)));
        }

        [SlashCommand("coinflip", "Flip a coin")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task CoinflipCommand(InteractionContext ctx)
        {
            var heads = RandomNumberGenerator.GetInt32(2) == 0;

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(new DiscordEmbedBuilder()
                    .WithTitle(heads ? "🪙 Heads" : "🪙 Tails")
                    .WithColor(DiscordColor.Gold)));
        }

        [SlashCommand("choose", "Pick one of the options you give")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task ChooseCommand(
            InteractionContext ctx,
            [Option("options", "Separated by commas or semicolons")] string opcoes)
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
                    new DiscordInteractionResponseBuilder().AddEmbed(Embeds.Error("Not enough options",
                        "Give at least two options, separated by commas. E.g. `pizza, sushi, burgers`")).AsEphemeral());
                return;
            }

            var picked = options[RandomNumberGenerator.GetInt32(options.Count)];

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(new DiscordEmbedBuilder()
                    .WithTitle("🤔 I pick")
                    .WithDescription($"**{Embeds.SafeTrim(picked, 200)}**")
                    .WithFooter($"out of {options.Count} options")
                    .WithColor(DiscordColor.Blurple)));
        }

        [SlashCommand("avatar", "Show a user's avatar at full size")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task AvatarCommand(
            InteractionContext ctx,
            [Option("user", "Whose avatar to show")] DiscordUser? usuario = null)
        {
            var target = usuario ?? ctx.User;
            var url = target.GetAvatarUrl(MediaFormat.Auto, 1024);

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(new DiscordEmbedBuilder()
                    .WithTitle($"{target.UsernameWithDiscriminator}'s avatar")
                    .WithImageUrl(url)
                    .WithUrl(url)
                    .WithColor(DiscordColor.Blurple)));
        }

        [SlashCommand("say", "Make the bot say something", (long)Permissions.ManageMessages)]
        // Exige ManageMessages: sem isso qualquer um faria o bot falar, e mensagem
        // vinda do bot tem aparencia de coisa oficial.
        [ApplicationCommandRequireUserPermissions(Permissions.ManageMessages)]
        // Declara a permissao que o comando de fato usa, em vez de descobrir a
        // falta dela por excecao no meio da execucao.
        [ApplicationCommandRequireBotPermissions(Permissions.SendMessages)]
        [ApplicationCommandRequireGuild]
        // A doc da classe diz que TODOS os comandos daqui tem cooldown; este era o
        // unico sem. E logo o que publica no canal.
        [SlashCommandCooldown(2, 15, CooldownBucketType.User)]
        public async Task SayCommand(
            InteractionContext ctx,
            [Option("text", "What the bot should say")] string texto)
        {
            // Sem mencao de cargo nem de @everyone: o /say seria um jeito de
            // contornar quem pode mencionar todo mundo.
            var builder = new DiscordMessageBuilder()
                .WithContent(Embeds.Trim(texto, 1900))
                .WithAllowedMentions(Array.Empty<IMention>());

            // Defer ANTES de publicar. Publicando primeiro, se o bot nao tivesse
            // permissao de escrever no canal a excecao acontecia com a interacao
            // ainda sem resposta: o Discord mostrava "This interaction failed" e
            // nada explicava o motivo.
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            /*
             * Quem usa precisa poder falar NESTE canal.
             *
             * O RequireUserPermissions confere ManageMessages - ja no escopo do
             * canal, e nao do servidor, ao contrario do que este comentario dizia.
             * Mas ManageMessages nao e SendMessages: sem esta checagem, um
             * moderador barrado de escrever num canal especifico - ou num canal
             * que outro moderador acabou de fechar com /lock - continuava
             * falando la pela boca do bot. O /say emprestaria a permissao do bot
             * para contornar uma restricao imposta a pessoa.
             */
            // Numa THREAD isto precisa olhar o canal PAI. PermissionsFor aplica so
            // os overwrites do proprio canal e nao sobe para o pai, e thread nao
            // tem overwrite proprio - entao dentro de uma thread o calculo caia
            // para a permissao de servidor e ignorava qualquer negacao do canal.
            // Um moderador barrado em #anuncios (ou um /lock recem-aplicado)
            // continuava publicando pela boca do bot a partir de uma thread de la.
            var inThread = ctx.Channel.Type is ChannelType.PublicThread or ChannelType.PrivateThread
                or ChannelType.NewsThread;
            var scope = inThread && ctx.Channel.Parent is { } parent ? parent : ctx.Channel;
            var needed = inThread ? Permissions.SendMessagesInThreads : Permissions.SendMessages;

            if ((ctx.Member!.PermissionsIn(scope) & needed) == 0)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("No permission in this channel",
                        "You can't send messages here, so you can't do it through me either.")));
                return;
            }

            try
            {
                await ctx.Channel.SendMessageAsync(builder);
            }
            catch (Exception ex)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Couldn't post the message", Embeds.Trim(ex.Message, 500))));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Sent", "The message was posted in the channel.")));

            // Registrado como qualquer outra acao privilegiada. Era a unica que
            // nao deixava rastro em lugar NENHUM: a resposta e efemera, a mensagem
            // sai assinada pelo bot, e o Audit Log do Discord nao cobre envio de
            // mensagem. "Quem fez o bot dizer isso?" nao tinha resposta.
            await ModerationLog.RecordAsync(ctx.Client, ctx.Guild!.Id, "Message via /say",
                null, ctx.User, null,
                $"{ctx.Channel.Mention}: {Embeds.SafeTrim(texto, 500)}",
                // Nao ha alvo: o rotulo padrao ("entire channel") e do
                // /purge e sairia como "User: entire channel" para
                // uma mensagem publicada.
                noTargetLabel: "—");
        }
    }
}
