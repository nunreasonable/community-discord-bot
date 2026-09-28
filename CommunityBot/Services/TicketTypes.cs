using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading.Tasks;
using DisCatSharp.ApplicationCommands.Attributes;
using DisCatSharp.Entities;

namespace CommunityBot.Services
{
    /// <summary>Um tipo de ticket: o que vira botao no painel e prefixo de canal.</summary>
    internal sealed record TicketType(string Key, string Label, string Emoji, string ChannelPrefix);

    /// <summary>
    /// Os tipos disponiveis, num lugar so.
    ///
    /// O teto pratico e CINCO: o painel poe um botao por tipo, e uma action row
    /// do Discord comporta cinco botoes. Passando disso e preciso quebrar em duas
    /// linhas, ou trocar os botoes por um menu de selecao.
    /// </summary>
    internal static class TicketTypes
    {
        public static readonly IReadOnlyList<TicketType> All = new[]
        {
            new TicketType("duvida", "Question", "❓", "question"),
            new TicketType("denuncia", "Report", "🚨", "report"),
            new TicketType("parceria", "Partnership", "🤝", "partnership"),
            new TicketType("outro", "Other", "📩", "other")
        };

        /// <summary>
        /// Devolve o tipo, ou null se a chave nao existe mais.
        ///
        /// Nao lanca de proposito: um botao de painel publicado ha meses carrega a
        /// chave no custom id, e alguem pode ter removido aquele tipo desta lista
        /// nesse meio-tempo. Quem chama trata o null com uma mensagem, em vez de
        /// o clique virar "interacao falhou".
        /// </summary>
        public static TicketType? Find(string? key) =>
            All.FirstOrDefault(t => string.Equals(t.Key, key, StringComparison.OrdinalIgnoreCase));

        /// <summary>Rotulo legivel para um tipo que talvez nao exista mais.</summary>
        public static string Describe(string? key) => Find(key)?.Label ?? key ?? "(unknown)";
    }

    /// <summary>
    /// Gera as escolhas do /ticket a partir de <see cref="TicketTypes.All"/>.
    ///
    /// Existe porque [Choice] e atributo, e atributo so aceita constante de
    /// compilacao - nao ha como ele ler a lista. Com os tipos escritos a mao nos
    /// dois lugares, acrescentar um quinto tipo em All fazia o painel ganhar o
    /// botao novo e o /ticket NAO ganhar a opcao, sem erro nenhum: as duas listas
    /// divergiam em silencio. Assim a lista volta a ter um dono so.
    /// </summary>
    internal sealed class TicketTypeChoiceProvider : IChoiceProvider
    {
        public Task<IEnumerable<DiscordApplicationCommandOptionChoice>> Provider() =>
            Task.FromResult(TicketTypes.All.Select(t =>
                new DiscordApplicationCommandOptionChoice(t.Label, t.Key)));
    }
}
