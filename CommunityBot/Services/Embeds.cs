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

        public static DiscordEmbed Denied(string description) =>
            Error("Permissão negada", description);

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
            return text.Length <= max ? text : text[..(max - 1)] + "…";
        }
    }
}
