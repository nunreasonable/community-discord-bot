using System;
using System.Collections.Generic;
using System.Threading.Tasks;
using DisCatSharp.Entities;
using DisCatSharp.Enums;

namespace CommunityBot.Services.Fun
{
    /// <summary>Uma acao do /roleplay: de onde vem o GIF e como a frase sai.</summary>
    internal sealed record RoleplayAction(
        string Key,
        string Category,
        string Emoji,
        string Verb,
        DiscordColor Color,
        bool Friendly);

    /// <summary>
    /// O /roleplay no molde da Loritta: um GIF, uma frase e um botao "Return"
    /// que so o alvo pode usar, com contador de combo quando os dois ficam
    /// devolvendo.
    /// </summary>
    internal static class RoleplayActions
    {
        public const string Prefix = "rp:";

        // Nenhuma cor vermelha, nem nas acoes "agressivas": vermelho e reservado
        // para erro e recusa no bot inteiro (ver Embeds).
        private static readonly Dictionary<string, RoleplayAction> s_actions = new(StringComparer.Ordinal)
        {
            ["hug"] = new("hug", "hug", "🤗", "hugged", new DiscordColor(0xFF8DE6), true),
            ["kiss"] = new("kiss", "kiss", "😘", "kissed", new DiscordColor(0xFF6FA8), true),
            ["slap"] = new("slap", "slap", "🖐️", "slapped", new DiscordColor(0xF0A04B), false),
            ["pat"] = new("pat", "pat", "🤚", "patted", new DiscordColor(0xFFD166), true),
            ["highfive"] = new("highfive", "highfive", "✋", "high-fived", new DiscordColor(0x57C4E5), true),
            ["dance"] = new("dance", "dance", "💃", "danced with", new DiscordColor(0xB980F0), true),
            ["attack"] = new("attack", "punch", "👊", "attacked", new DiscordColor(0xE8894A), false),
            ["cuddle"] = new("cuddle", "cuddle", "🥰", "cuddled", new DiscordColor(0xF7A1C4), true),
            ["poke"] = new("poke", "poke", "👉", "poked", new DiscordColor(0x8BD3DD), true),
            ["bonk"] = new("bonk", "bonk", "🔨", "bonked", new DiscordColor(0xC9A66B), false),
        };

        public static RoleplayAction? Find(string key) => s_actions.TryGetValue(key, out var a) ? a : null;

        /// <summary>
        /// Quem faz o que em quem, depois dos casos especiais. Devolve null em
        /// <paramref name="actor"/> quando a acao foi recusada (beijo no bot).
        /// </summary>
        public static (DiscordUser? Actor, DiscordUser Target, string? Note) Resolve(RoleplayAction action,
            DiscordUser user, DiscordUser target, DiscordUser bot)
        {
            if (target.Id == user.Id)
            {
                // Abraco em si mesmo vira abraco do bot - como na Loritta.
                return action.Friendly
                    ? (bot, user, "Everyone needs one sometimes.")
                    : (user, user, "Are you okay?");
            }

            if (target.Id == bot.Id)
            {
                if (action.Key == "kiss")
                    return (null, target, null);

                // Bater no bot volta para quem bateu.
                return action.Friendly
                    ? (user, bot, "Thanks! 💖")
                    : (bot, user, "Nice try.");
            }

            return (user, target, null);
        }

        public static string Sentence(RoleplayAction action, DiscordUser actor, DiscordUser target, int combo)
        {
            var who = actor.Id == target.Id ? "themselves" : target.Mention;
            var comboTag = combo >= 3 ? $"**[COMBO x{combo}]** " : string.Empty;
            return $"{comboTag}{action.Emoji} {actor.Mention} {action.Verb} {who}!";
        }

        /// <summary>
        /// O cartao completo, com o botao "Return" quando faz sentido: o alvo
        /// precisa ser uma pessoa e outra que nao quem agiu.
        /// </summary>
        public static async Task<(DiscordEmbed Embed, DiscordComponent[] Buttons)> BuildAsync(RoleplayAction action,
            DiscordUser actor, DiscordUser target, int combo, string? note)
        {
            var gif = await NekosBest.GetAsync(action.Category);

            var description = Sentence(action, actor, target, combo);
            if (note is not null)
                description += $"\n*{note}*";

            var embed = new DiscordEmbedBuilder()
                .WithDescription(description)
                .WithColor(action.Color);

            if (gif is not null)
            {
                embed.WithImageUrl(gif.Url);
                embed.WithFooter(gif.AnimeName is null ? "via nekos.best" : $"Anime: {Embeds.Trim(gif.AnimeName, 100)} · via nekos.best");
            }
            else
            {
                embed.WithFooter("Couldn't fetch a GIF this time.");
            }

            var buttons = target.IsBot || target.Id == actor.Id
                ? Array.Empty<DiscordComponent>()
                : new DiscordComponent[] { ReturnButton(action, actor.Id, target.Id, combo, enabled: true) };

            return (embed.Build(), buttons);
        }

        /// <summary>
        /// "rp:acao:de:para:combo". Tudo o que o clique precisa vai no proprio
        /// botao: nada fica em memoria, entao o botao continua valendo depois de
        /// o bot reiniciar.
        /// </summary>
        public static DiscordButtonComponent ReturnButton(RoleplayAction action, ulong fromId, ulong toId, int combo,
            bool enabled) =>
            new(ButtonStyle.Primary, $"{Prefix}{action.Key}:{fromId}:{toId}:{combo}", "Return", !enabled,
                new DiscordComponentEmoji(action.Emoji));
    }
}
