using System;
using System.Linq;
using System.Threading.Tasks;
using CommunityBot.Services;
using CommunityBot.Services.Fun;
using DisCatSharp.ApplicationCommands;
using DisCatSharp.ApplicationCommands.Attributes;
using DisCatSharp.ApplicationCommands.Context;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.Enums.Core;

namespace CommunityBot.commands
{
    /// <summary>
    /// /text e /morse: transformacoes de texto no estilo do /text da Loritta.
    ///
    /// A resposta sai como mensagem comum, sem embed, porque o ponto e copiar o
    /// resultado. Duas protecoes valem para todas: nenhuma mencao e notificada
    /// (o texto e de quem digitou, mas a mensagem sai assinada pelo bot), e o
    /// resultado passa pelo Embeds.Safe - mensagem comum tambem renderiza link
    /// mascarado, e o /text clap preservaria um `[clique](url)` inteiro.
    /// </summary>
    internal static class TextReply
    {
        public static Task ReplyAsync(InteractionContext ctx, string result)
        {
            var text = string.IsNullOrWhiteSpace(result) ? "*(nothing left after the transformation)*" : Embeds.SafeTrim(result, 1990);

            return ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder()
                    .WithContent(text)
                    .WithAllowedMentions(Array.Empty<IMention>()));
        }
    }

    [SlashCommandGroup("text", "Transform some text")]
    internal class TextCommands : ApplicationCommandsModule
    {
        [SlashCommand("vaporwave", "Ｍａｋｅ ｉｔ ａｅｓｔｈｅｔｉｃ")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task Vaporwave(InteractionContext ctx,
            [Option("text", "The text to transform")][MaximumLength(900)] string texto) =>
            TextReply.ReplyAsync(ctx, TextTransforms.Vaporwave(texto));

        [SlashCommand("clap", "Put 👏 between 👏 every 👏 word")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task Clap(InteractionContext ctx,
            [Option("text", "The text to transform")][MaximumLength(900)] string texto,
            [Option("emoji", "What goes between the words (default 👏)")][MaximumLength(60)] string emoji = "👏") =>
            TextReply.ReplyAsync(ctx, TextTransforms.Clap(texto, string.IsNullOrWhiteSpace(emoji) ? "👏" : emoji.Trim()));

        [SlashCommand("mock", "wRiTe LiKe ThIs")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task Mock(InteractionContext ctx,
            [Option("text", "The text to transform")][MaximumLength(900)] string texto) =>
            TextReply.ReplyAsync(ctx, TextTransforms.Mock(texto));

        [SlashCommand("quality", "Q U A L I T Y")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task Quality(InteractionContext ctx,
            [Option("text", "The text to transform")][MaximumLength(900)] string texto) =>
            TextReply.ReplyAsync(ctx, TextTransforms.Quality(texto));

        [SlashCommand("upsidedown", "uʍop ǝpᴉsdn ʇᴉ uɹnꓕ")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task UpsideDown(InteractionContext ctx,
            [Option("text", "The text to transform")][MaximumLength(900)] string texto) =>
            TextReply.ReplyAsync(ctx, TextTransforms.UpsideDown(texto));

        [SlashCommand("reverse", "sdrawkcab ti etirW")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task Reverse(InteractionContext ctx,
            [Option("text", "The text to transform")][MaximumLength(900)] string texto) =>
            TextReply.ReplyAsync(ctx, TextTransforms.Reverse(texto));
    }

    [SlashCommandGroup("morse", "Morse code translator")]
    internal class MorseCommands : ApplicationCommandsModule
    {
        [SlashCommand("encode", "Turn text into Morse code")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task Encode(InteractionContext ctx,
            [Option("text", "The text to encode")][MaximumLength(300)] string texto)
        {
            var (code, unknown) = TextTransforms.MorseEncode(texto);
            var result = code.Length == 0 ? string.Empty : $"`{code}`";
            if (unknown.Count > 0)
                result += $"\n-# Skipped characters with no Morse code: {string.Join(" ", unknown.Take(20))}";

            return TextReply.ReplyAsync(ctx, result);
        }

        [SlashCommand("decode", "Turn Morse code back into text")]
        [SlashCommandCooldown(3, 10, CooldownBucketType.User)]
        public Task Decode(InteractionContext ctx,
            [Option("code", "Dots and dashes; letters split by spaces, words by /")][MaximumLength(1500)] string codigo)
        {
            var (text, unknown) = TextTransforms.MorseDecode(codigo);
            var result = text;
            if (unknown > 0)
                result += $"\n-# {unknown} code(s) weren't valid Morse and show up as �.";

            return TextReply.ReplyAsync(ctx, result);
        }
    }
}
