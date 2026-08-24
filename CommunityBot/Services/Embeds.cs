using DisCatSharp.Entities;

namespace CommunityBot.Services
{
    /// <summary>
    /// Embeds padrao do bot.
    ///
    /// Segue a regra de cor do ccore: vermelho e RESERVADO para erro e recusa, e
    /// nada mais. Num bot que bane e expulsa gente, a cor mais forte da tela tem
    /// de querer dizer alguma coisa.
    /// </summary>
    internal static class Embeds
    {
        public static DiscordEmbed Error(string title, string description) =>
            new DiscordEmbedBuilder()
                .WithTitle(title)
                .WithDescription(description)
                .WithColor(DiscordColor.IndianRed)
                .Build();


        public static DiscordEmbed Ok(string title, string description) =>
            new DiscordEmbedBuilder()
                .WithTitle(title)
                .WithDescription(description)
                .WithColor(DiscordColor.SpringGreen)
                .Build();

        public static DiscordEmbed Info(string title, string description) =>
            new DiscordEmbedBuilder()
                .WithTitle(title)
                .WithDescription(description)
                .WithColor(DiscordColor.Blurple)
                .Build();

        /// <summary>Corta preservando o limite do Discord, com reticencias.</summary>
        public static string Trim(string? value, int max)
        {
            var text = value ?? string.Empty;
            if (text.Length <= max)
                return text;

            // Nao corta no meio de um par substituto (um emoji ou caractere fora
            // do BMP no ponto de corte): meio surrogate vira "<?>" no Discord.
            var cut = max - 1;
            if (cut > 0 && char.IsHighSurrogate(text[cut - 1]))
                cut--;

            return text[..cut] + "…";
        }
    }
}
