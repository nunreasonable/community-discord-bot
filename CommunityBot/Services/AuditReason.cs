using System.Linq;

namespace CommunityBot.Services
{
    /// <summary>
    /// Texto que vai para o Audit Log do Discord.
    ///
    /// Mora aqui, e nao dentro de um modulo de comando, porque o valor viaja num
    /// CABECALHO HTTP (X-Audit-Log-Reason) e a limpeza que isso exige nao pode
    /// existir em copias: uma quebra de linha no meio derruba a requisicao
    /// inteira, e o resultado e a acao falhar ANTES de acontecer por causa de um
    /// texto que alguem colou.
    ///
    /// Era um metodo privado de Moderation ate os tickets precisarem do mesmo
    /// cuidado ao criar e apagar canal.
    /// </summary>
    internal static class AuditReason
    {
        /// <summary>Limite do cabecalho, com folga para o prefixo de quem executou.</summary>
        private const int MaxLength = 400;

        /// <summary>
        /// Motivo carregando quem mandou fazer. Isso faz o registro nativo do
        /// Discord continuar util mesmo se o canal de log for apagado.
        /// </summary>
        public static string For(string actorTag, string? reason) =>
            Embeds.Trim($"{actorTag}: {Clean(reason)}", MaxLength);

        /// <summary>Motivo sem autor, para acao que o proprio bot decidiu.</summary>
        public static string Automatic(string reason) =>
            Embeds.Trim(Clean(reason), MaxLength);

        /// <summary>
        /// Troca todo caractere de controle por espaco. E o `\n` que importa, mas
        /// a regra vale para a classe toda: nada disso tem significado num
        /// cabecalho HTTP.
        /// </summary>
        private static string Clean(string? reason) =>
            new((reason ?? "sem motivo informado").Select(c => char.IsControl(c) ? ' ' : c).ToArray());
    }
}
