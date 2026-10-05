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

        /// <summary>
        /// Texto de usuario indo para dentro de um EMBED.
        ///
        /// Descricao de embed renderiza link mascarado: `[clique aqui](https://…)`
        /// vira um link com rotulo arbitrario, coisa que mensagem comum nao faz.
        /// Sem isto, /poll, /8ball e /choose - todos abertos a qualquer membro -
        /// entregam a qualquer um um cartao ASSINADO PELO BOT com um link cujo
        /// destino nao aparece. E phishing com a cara da casa, e e exatamente o
        /// risco que faz o /say exigir ManageMessages: aquele portao fica
        /// meio inutil se o mesmo efeito sai por um comando aberto.
        ///
        /// Escapar o `[` basta: sem colchete de abertura nao se forma rotulo, e a
        /// URL crua que sobra mostra para onde vai. A barra invertida nao aparece
        /// no texto final.
        /// </summary>
        public static string Safe(string? value) =>
            (value ?? string.Empty)
                // A ORDEM E OBRIGATORIA. Escapar so o `[` era contornavel com uma
                // barra invertida: a entrada `\[texto](url)` virava `\\[`, o
                // Discord consumia `\\` como uma barra escapada e SOLTAVA o
                // colchete - o link mascarado voltava a valer, dentro de um embed
                // assinado pelo bot. Escapar a propria barra primeiro fecha isso;
                // inverter as duas linhas reduplicaria as barras recem-inseridas.
                .Replace("\\", "\\\\")
                .Replace("[", "\\[");

        /// <summary>
        /// Escapa e SO ENTAO corta, no limite pedido.
        ///
        /// A ordem importa e a inversa e uma armadilha: `Safe(Trim(x, 900))`
        /// corta em 900 e depois dobra cada `[` em `\[`, entao 900 colchetes
        /// viram 1800 caracteres - acima do teto de 1024 de um campo de embed.
        /// O Discord recusa a mensagem inteira, e no caminho do fechamento de
        /// ticket isso significava o transcript nao subir por causa de um texto
        /// que alguem colou. Compondo na ordem certa uma vez so, nenhum chamador
        /// precisa lembrar disso.
        /// </summary>
        public static string SafeTrim(string? value, int max) => Trim(Safe(value), max);

        /// <summary>
        /// Nome de pessoa ou servidor mostrado como texto puro: escapa TODA a
        /// marcacao, e nao so o link mascarado do Safe. Um username com "_" (que
        /// o Discord permite) viraria italico no meio de um ranking.
        /// </summary>
        public static string Plain(string? value, int max)
        {
            var text = Trim(value, max);
            var sb = new System.Text.StringBuilder(text.Length + 8);
            foreach (var c in text)
            {
                if (c is '\\' or '*' or '_' or '~' or '`' or '|' or '>' or '[' or ']' or '(' or ')' or '#' or '-' or '<' or '@')
                    sb.Append('\\');
                sb.Append(c);
            }
            return sb.ToString();
        }

        /// <summary>
        /// "1,250" - separador fixo. Com {x:N0} o numero sairia na cultura da
        /// maquina: a unit roda em en_GB hoje, mas um pt_BR viraria "1.250", que
        /// em ingles se le como um virgula vinte e cinco.
        /// </summary>
        public static string Number(long value) =>
            value.ToString("N0", System.Globalization.CultureInfo.InvariantCulture);

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
