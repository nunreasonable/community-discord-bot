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
                    .AddField(new DiscordEmbedField("Pergunta", Embeds.SafeTrim(question, 900), false))
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
                    .WithDescription($"**{Embeds.SafeTrim(picked, 200)}**")
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

        [SlashCommand("say", "Faz o bot repetir um texto", (long)Permissions.ManageMessages)]
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
            [Option("texto", "O que o bot vai dizer")] string texto)
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
                    Embeds.Error("Sem permissão neste canal",
                        "Você não pode enviar mensagens aqui, então também não pode fazer isso através de mim.")));
                return;
            }

            try
            {
                await ctx.Channel.SendMessageAsync(builder);
            }
            catch (Exception ex)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Falha ao publicar", Embeds.Trim(ex.Message, 500))));
                return;
            }

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Enviado", "A mensagem foi publicada no canal.")));

            // Registrado como qualquer outra acao privilegiada. Era a unica que
            // nao deixava rastro em lugar NENHUM: a resposta e efemera, a mensagem
            // sai assinada pelo bot, e o Audit Log do Discord nao cobre envio de
            // mensagem. "Quem fez o bot dizer isso?" nao tinha resposta.
            await ModerationLog.RecordAsync(ctx.Client, ctx.Guild!.Id, "Mensagem via /say",
                null, ctx.User, null,
                $"{ctx.Channel.Mention}: {Embeds.SafeTrim(texto, 500)}",
                // Nao ha alvo: o rotulo padrao ("toda a conversa do canal") e do
                // /purge e sairia como "Usuario: toda a conversa do canal" para
                // uma mensagem publicada.
                noTargetLabel: "—");
        }
    }
}
