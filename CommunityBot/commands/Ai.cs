using System;
using System.Threading.Tasks;
using CommunityBot.config;
using CommunityBot.Services;
using CommunityBot.Services.Ai;
using CommunityBot.Services.Economy;
using DisCatSharp.ApplicationCommands;
using DisCatSharp.ApplicationCommands.Attributes;
using DisCatSharp.ApplicationCommands.Context;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.Enums.Core;

namespace CommunityBot.commands
{
    /// <summary>
    /// Recursos que usam a IA local fora do chat por mencao: o menu de
    /// contexto "Translate" numa mensagem, e o /quiz. Os dois pedem ao modelo
    /// via LocalAi.AskAsync - nao tem a ver com a persona Sollarety do
    /// LocalAi.OnMessageCreated, so o servidor por tras e o mesmo.
    /// </summary>
    internal class Ai : ApplicationCommandsModule
    {
        private const int MinQuestions = 3;
        private const int MaxQuestions = 5;

        // ------------------------------------------------------------------
        // Menu de contexto: Translate
        // ------------------------------------------------------------------

        [ContextMenu(ApplicationCommandType.Message, "Translate",
            allowedContexts: new[] { InteractionContextType.Guild, InteractionContextType.BotDm, InteractionContextType.PrivateChannel },
            integrationTypes: new[] { ApplicationCommandIntegrationTypes.GuildInstall, ApplicationCommandIntegrationTypes.UserInstall })]
        [ContextMenuCooldown(3, 20, CooldownBucketType.User)]
        public async Task TranslateCommand(ContextMenuContext ctx)
        {
            var reader = new JSONReader();
            await reader.ReadJSON();
            var settings = reader.localAi;

            if (!LocalAi.IsAllowedGuild(settings, ctx.Guild?.Id))
            {
                await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                    new DiscordInteractionResponseBuilder().AddEmbed(Embeds.Info("Not available here",
                        "This only works in servers where the local AI is turned on.")).AsEphemeral());
                return;
            }

            // Sem o intent de Message Content para a maioria das mensagens, mas
            // o Discord manda o conteudo da mensagem-alvo de um menu de contexto
            // mesmo assim - e por isso que isto funciona sem pedir o intent.
            var text = (ctx.TargetMessage?.Content ?? string.Empty).Trim();
            if (text.Length == 0)
            {
                await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                    new DiscordInteractionResponseBuilder().AddEmbed(Embeds.Error("Nothing to translate",
                        "That message doesn't have any text — just an embed, image or attachment, maybe.")).AsEphemeral());
                return;
            }

            if (!await LocalAi.IsUpAsync(settings))
            {
                await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                    new DiscordInteractionResponseBuilder().AddEmbed(Embeds.Info("AI is off",
                        "The local AI isn't running right now. Try again later.")).AsEphemeral());
                return;
            }

            // Pode levar de 5 a 30s: o modelo so tem um slot, e pode estar
            // ocupado com outra coisa.
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var toPortuguese = LocalAi.IsEnglish(text);
            var system = toPortuguese
                ? "You are a translation engine. Translate the user's message into natural Brazilian Portuguese. " +
                  "Output ONLY the translation and nothing else: no quotes, no notes, no explanations. Keep the " +
                  "same tone, and keep mentions, markdown and links exactly as they are."
                : "You are a translation engine. Translate the user's message into natural English. " +
                  "Output ONLY the translation and nothing else: no quotes, no notes, no explanations. Keep the " +
                  "same tone, and keep mentions, markdown and links exactly as they are.";

            var translated = await LocalAi.AskAsync(settings, system, text, maxTokens: 500);

            if (string.IsNullOrWhiteSpace(translated))
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(Embeds.Error("Couldn't translate that",
                    "Something went wrong talking to the AI. Try again later.")));
                return;
            }

            // So a traducao, sem embed - e tudo que o pedido quer de volta.
            await ctx.EditResponseAsync(new DiscordWebhookBuilder().WithContent(Embeds.Trim(translated, 1900)));
        }

        // ------------------------------------------------------------------
        // /quiz
        // ------------------------------------------------------------------

        [SlashCommand("quiz", "Starts a themed trivia quiz for this channel", allowedContexts: new[] { InteractionContextType.Guild })]
        [ApplicationCommandRequireGuild]
        [SlashCommandCooldown(1, 10, CooldownBucketType.User)]
        public async Task QuizCommand(
            InteractionContext ctx,
            [Option("theme", "What the quiz should be about")] string tema,
            [Option("questions", "How many questions (3 to 5, default 3)")]
            [MinimumValue(MinQuestions)][MaximumValue(MaxQuestions)] long perguntas = MinQuestions)
        {
            var reader = new JSONReader();
            await reader.ReadJSON();
            var settings = reader.localAi;

            if (!LocalAi.IsAllowedGuild(settings, ctx.Guild?.Id))
            {
                await RefuseAsync(ctx, Embeds.Info("Not available here", "The local AI quiz is off in this server."));
                return;
            }

            if (!await LocalAi.IsUpAsync(settings))
            {
                await RefuseAsync(ctx, Embeds.Info("AI is off", "The local AI isn't running right now. Try again later."));
                return;
            }

            var reserve = AiQuiz.TryReserve(ctx.Channel.Id, out var game, out var cooldownUntil);
            if (reserve == QuizReserve.Active)
            {
                await RefuseAsync(ctx, Embeds.Info("Quiz already running",
                    "There's already a quiz going in this channel. Wait for it to finish."));
                return;
            }

            if (reserve == QuizReserve.Cooldown)
            {
                await RefuseAsync(ctx, Embeds.Info("Still cooling down",
                    $"This channel just had a quiz. Try again <t:{cooldownUntil.ToUnixTimeSeconds()}:R>."));
                return;
            }

            // Perguntas e 3 a 5 pelo MinimumValue/MaximumValue, mas a geracao e
            // cara o bastante para valer uma segunda trava aqui.
            var count = (int)Math.Clamp(perguntas, MinQuestions, MaxQuestions);

            // Publico, e nao efemero: o quiz e pra o canal inteiro jogar, e pode
            // levar de 5 a 30s para a IA montar as perguntas.
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource);

            var questions = await AiQuiz.GenerateAsync(settings, tema, count);
            if (questions is null)
            {
                AiQuiz.Cancel(game!);
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(Embeds.Error("Couldn't build that quiz",
                    "The AI didn't come back with a usable set of questions for that theme. Try again, maybe with a different theme.")));
                return;
            }

            game!.Questions = questions;
            game.Paid = GuildSettingsStore.For(ctx.Guild!.Id)?.economyEnabled == true;

            await ctx.EditResponseAsync(new DiscordWebhookBuilder()
                .AddEmbed(new DiscordEmbedBuilder()
                    .WithTitle("🧠 Quiz time!")
                    .WithDescription($"{ctx.User.Mention} started a quiz about **{Embeds.Safe(tema)}** — " +
                                      $"{questions.Count} question(s). " +
                                      (game.Paid
                                          ? $"First to answer each one right gets {EconomyService.Format(AiQuiz.Reward)}."
                                          : "First to answer each one right gets the point."))
                    .WithColor(DiscordColor.Blurple))
                .WithAllowedMentions(Array.Empty<IMention>()));

            // Daqui pra frente e o /quiz tocando sozinho: a interacao ja foi
            // respondida, e o resto sao mensagens novas no canal.
            _ = Task.Run(() => AiQuiz.RunAsync(ctx.Channel, game));
        }

        private static Task RefuseAsync(InteractionContext ctx, DiscordEmbed embed) =>
            ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AddEmbed(embed).AsEphemeral());
    }
}
