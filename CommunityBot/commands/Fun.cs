using System;
using System.Globalization;
using System.Linq;
using System.Security.Cryptography;
using System.Threading.Tasks;
using CommunityBot.Services;
using CommunityBot.Services.Fun;
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
    // /8ball, /roll, /coinflip, /choose, /avatar, /ship, /rate e /cancel
    // funcionam em DM e nao tocam em ctx.Guild. Todo comando daqui que PRECISA de servidor - ou que e
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

        [SlashCommand("8ball", "Ask the magic 8-ball a question",
            allowedContexts: new[] { InteractionContextType.Guild, InteractionContextType.BotDm, InteractionContextType.PrivateChannel },
            integrationTypes: new[] { ApplicationCommandIntegrationTypes.GuildInstall, ApplicationCommandIntegrationTypes.UserInstall })]
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

        [SlashCommand("roll", "Roll dice in NdM format",
            allowedContexts: new[] { InteractionContextType.Guild, InteractionContextType.BotDm, InteractionContextType.PrivateChannel },
            integrationTypes: new[] { ApplicationCommandIntegrationTypes.GuildInstall, ApplicationCommandIntegrationTypes.UserInstall })]
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

        [SlashCommand("coinflip", "Flip a coin",
            allowedContexts: new[] { InteractionContextType.Guild, InteractionContextType.BotDm, InteractionContextType.PrivateChannel },
            integrationTypes: new[] { ApplicationCommandIntegrationTypes.GuildInstall, ApplicationCommandIntegrationTypes.UserInstall })]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task CoinflipCommand(InteractionContext ctx)
        {
            var heads = RandomNumberGenerator.GetInt32(2) == 0;

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(new DiscordEmbedBuilder()
                    .WithTitle(heads ? "🪙 Heads" : "🪙 Tails")
                    .WithColor(DiscordColor.Gold)));
        }

        [SlashCommand("choose", "Pick one of the options you give",
            allowedContexts: new[] { InteractionContextType.Guild, InteractionContextType.BotDm, InteractionContextType.PrivateChannel },
            integrationTypes: new[] { ApplicationCommandIntegrationTypes.GuildInstall, ApplicationCommandIntegrationTypes.UserInstall })]
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

        [SlashCommand("avatar", "Show a user's avatar at full size",
            allowedContexts: new[] { InteractionContextType.Guild, InteractionContextType.BotDm, InteractionContextType.PrivateChannel },
            integrationTypes: new[] { ApplicationCommandIntegrationTypes.GuildInstall, ApplicationCommandIntegrationTypes.UserInstall })]
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

        // ------------------------------------------------------------------
        // /ship, /rate e /cancel - no molde da Loritta
        // ------------------------------------------------------------------

        private static readonly string[] s_shipPerfect =
        {
            "They're perfect for each other! 💞", "A match made in heaven. 💍"
        };

        private static readonly string[] s_shipHigh =
        {
            "They were made for each other! 💕", "Something's definitely there. 😍",
            "Wedding bells might be ringing soon. 🔔"
        };

        private static readonly string[] s_shipMid =
        {
            "If they stopped being so shy, maybe it could work. 🤔", "Could go either way. 🤷",
            "Worth a shot, maybe?"
        };

        private static readonly string[] s_shipLow =
        {
            "Nah, sadly it wouldn't work… 😢", "Better as friends. 🤝", "The stars say no. 🌧️"
        };

        private static readonly string[] s_shipFriendzone =
        {
            "I like you, but only as a friend. 😅", "Sorry, I'm married to my job. 🤖",
            "You're sweet, but I'm just a bot. 💾"
        };

        [SlashCommand("ship", "See how compatible two people are",
            allowedContexts: new[] { InteractionContextType.Guild, InteractionContextType.BotDm, InteractionContextType.PrivateChannel },
            integrationTypes: new[] { ApplicationCommandIntegrationTypes.GuildInstall, ApplicationCommandIntegrationTypes.UserInstall })]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task ShipCommand(
            InteractionContext ctx,
            [Option("user", "The first person")] DiscordUser usuario,
            [Option("other", "The second person (default: you)")] DiscordUser? outro = null)
        {
            var a = usuario;
            var b = outro ?? ctx.User;
            var botId = ctx.Client.CurrentUser.Id;

            // Soma dos ids: simetrica (A+B = B+A) e fixa, como na Loritta - o mesmo
            // casal sempre da o mesmo numero, e trocar a ordem nao muda nada.
            // unchecked por principio; dois snowflakes somados cabem folgado no ulong.
            var seed = unchecked(a.Id + b.Id);

            int percent;
            string line;
            if (a.Id == b.Id)
            {
                percent = 100;
                line = "Self-love is the best love. 💖";
            }
            else if (a.Id == botId || b.Id == botId)
            {
                percent = (int)(seed % 51);
                line = s_shipFriendzone[seed % (ulong)s_shipFriendzone.Length];
            }
            else
            {
                percent = (int)(seed % 101);
                var tier = percent switch
                {
                    100 => s_shipPerfect,
                    >= 67 => s_shipHigh,
                    >= 34 => s_shipMid,
                    >= 1 => s_shipLow,
                    _ => new[] { "No. Just no. 💔" }
                };
                line = tier[seed / 101 % (ulong)tier.Length];
            }

            // Metade de cada nome por elemento de texto, e nao por char: cortar
            // um emoji do nome ao meio deixaria um "?" no nome do casal.
            var nameA = new StringInfo(ShipDisplayName(a));
            var nameB = new StringInfo(ShipDisplayName(b));
            var shipName = nameA.SubstringByTextElements(0, (nameA.LengthInTextElements + 1) / 2) +
                           nameB.SubstringByTextElements(nameB.LengthInTextElements / 2);

            var filled = (int)Math.Round(percent / 10.0);
            var bar = new string('█', filled) + new string('░', 10 - filled);

            var color = percent >= 67 ? new DiscordColor(0xFF6FA8) : percent >= 34 ? DiscordColor.Gold : DiscordColor.Gray;

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(new DiscordEmbedBuilder()
                    .WithTitle("💘 Matchmaking")
                    .WithDescription($"{a.Mention} + {b.Mention} = ✨**{Embeds.SafeTrim(shipName, 64)}**✨\n\n" +
                                     $"`{bar}` **{percent}%**\n{line}")
                    .WithColor(color)));
        }

        private static string ShipDisplayName(DiscordUser user)
        {
            var name = string.IsNullOrWhiteSpace(user.GlobalName) ? user.Username : user.GlobalName;
            return name.Length == 0 ? "?" : name;
        }

        private static readonly string[] s_rateReasons =
        {
            "Please, just no. 🗑️",
            "Yikes. Hard pass. 😬",
            "I've seen better. Much better. 😐",
            "Not great, not terrible. Mostly not great. 🫤",
            "It has its moments. Few of them. 🙃",
            "Perfectly average. 🤷",
            "Pretty decent, actually. 🙂",
            "I like it! 👍",
            "Really good stuff. 😄",
            "Almost perfect! ✨",
            "Simply perfect. No notes. 🏆"
        };

        [SlashCommand("rate", "I'll rate anything from 0 to 10",
            allowedContexts: new[] { InteractionContextType.Guild, InteractionContextType.BotDm, InteractionContextType.PrivateChannel },
            integrationTypes: new[] { ApplicationCommandIntegrationTypes.GuildInstall, ApplicationCommandIntegrationTypes.UserInstall })]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task RateCommand(
            InteractionContext ctx,
            [Option("thing", "What should I rate?")][MaximumLength(200)] string coisa)
        {
            var key = coisa.Trim().ToLowerInvariant();
            var botId = ctx.Client.CurrentUser.Id;

            string score;
            string reason;
            if (key is "sollarety" or "you" or "yourself" || key == $"<@{botId}>" || key == $"<@!{botId}>")
            {
                score = "∞";
                reason = "Obviously. 😌";
            }
            else
            {
                // Muda uma vez por dia, como na Loritta: a mesma coisa tem a mesma
                // nota o dia inteiro, e insistir no comando nao sobe a nota.
                var today = DateTime.UtcNow;
                var value = (int)(StableHash.Of($"{key}|{today.Year}-{today.DayOfYear}") % 11);
                score = $"{value}/10";
                reason = s_rateReasons[value];
            }

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(new DiscordEmbedBuilder()
                    .WithDescription($"🤔 I rate **{Embeds.SafeTrim(coisa, 200)}** a **{score}**!\n*{reason}*")
                    .WithColor(DiscordColor.Blurple)));
        }

        private static readonly string[] s_cancelReasons =
        {
            "putting pineapple on pizza", "clapping when the plane landed", "replying \"k\" to a paragraph",
            "leaving someone on read for three business days", "using light mode at 3 AM",
            "saying \"it's giving\" unironically", "microwaving fish in the office", "spoiling the season finale",
            "not using their turn signal", "putting milk before cereal", "reheating coffee four times",
            "laughing at their own jokes before the punchline", "pronouncing GIF wrong (either way)",
            "having 4,000 unread emails", "skipping the tutorial and then asking for help",
            "calling a hot dog a sandwich", "saying \"no offense\" and then offending",
            "starting a sentence with \"well, actually\"", "never muting their mic",
            "breathing into the mic in voice chat", "pinging @everyone for a meme",
            "rating their own music 10/10", "eating the last slice without asking",
            "posting \"first\" in the comments", "spelling it \"definately\""
        };

        [SlashCommand("cancel", "Cancel someone on the internet (it's a joke)",
            allowedContexts: new[] { InteractionContextType.Guild, InteractionContextType.BotDm, InteractionContextType.PrivateChannel },
            integrationTypes: new[] { ApplicationCommandIntegrationTypes.GuildInstall, ApplicationCommandIntegrationTypes.UserInstall })]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public async Task CancelCommand(
            InteractionContext ctx,
            [Option("user", "Who gets cancelled")] DiscordUser usuario)
        {
            var reason = s_cancelReasons[RandomNumberGenerator.GetInt32(s_cancelReasons.Length)];

            await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(new DiscordEmbedBuilder()
                    .WithDescription($"📢 {usuario.Mention} was **cancelled** for {reason}.")
                    .WithFooter("It's a joke — nobody actually got cancelled.")
                    .WithColor(DiscordColor.Orange)));
        }

        [SlashCommand("say", "Make the bot say something", (long)Permissions.ManageMessages, allowedContexts: new[] { InteractionContextType.Guild })]
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
