using System;
using System.Collections.Generic;
using System.Linq;
using System.Text;
using System.Threading.Tasks;
using CommunityBot.config;
using CommunityBot.Services;
using DisCatSharp.ApplicationCommands;
using DisCatSharp.ApplicationCommands.Attributes;
using DisCatSharp.ApplicationCommands.Context;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.Interactivity.Entities;
using DisCatSharp.Interactivity.Extensions;

namespace CommunityBot.commands
{
    /// <summary>
    /// Mostra as ultimas linhas que o bot escreveu no console, lidas do
    /// <see cref="BotLogBuffer"/>. Evita precisar de acesso SSH a maquina para
    /// descobrir por que alguma coisa falhou.
    /// </summary>
    [ApplicationCommandRequireGuild]
    internal class BotLogs : ApplicationCommandsModule
    {
        private const string LevelAll = "todos";
        private const string LevelInfo = "info";
        private const string LevelWarn = "aviso";
        private const string LevelError = "erro";

        // Os logs carregam ids de usuario, mensagens de excecao e caminhos da
        // maquina que hospeda o bot.
        //
        // O RequireGuild na classe nao e redundante com o atributo abaixo: o
        // ApplicationCommandRequireUserPermissions tem IgnoreDms = true por
        // padrao e o defaultMemberPermissions nao vale em DM. Sem ele, com o
        // registro global qualquer um que pudesse mandar DM ao bot leria o
        // buffer inteiro.
        //
        // Os dois atributos juntos ainda param no "administrador DAQUELE
        // servidor", e esse portao deixou de bastar quando o registro virou
        // global: o buffer e do PROCESSO, com linhas de todos os servidores em
        // que o bot esta, e qualquer pessoa que criasse um servidor e
        // convidasse o bot seria administradora nele. Por isso a checagem de
        // dono da aplicacao no corpo do comando - ver IsApplicationOwner.
        [SlashCommand("logs", "Mostra os últimos logs do bot", (long)Permissions.Administrator)]
        [ApplicationCommandRequireUserPermissions(Permissions.Administrator)]
        public async Task LogsCommand(
            InteractionContext ctx,
            [Option("quantidade", "Quantas linhas mostrar (1 a 100, padrão 25)")] long quantidade = 25,
            [Choice("Todos", LevelAll)]
            [Choice("Apenas erros", LevelError)]
            [Choice("Apenas avisos", LevelWarn)]
            [Choice("Apenas informativos", LevelInfo)]
            [Option("nivel", "Filtrar por severidade")] string nivel = LevelAll,
            [Option("filtro", "Mostrar apenas linhas que contenham este texto")] string? filtro = null,
            [Option("privado", "Mostrar apenas para você")] bool privado = true)
        {
            if (!IsApplicationOwner(ctx))
            {
                await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                    new DiscordInteractionResponseBuilder()
                        .AddEmbed(Embeds.Error("Comando recusado",
                            "O `/logs` mostra o log do **processo inteiro** — o que o bot fez em " +
                            "todos os servidores em que está, com ids de usuário, mensagens de " +
                            "exceção e caminhos da máquina.\n\n" +
                            "Por isso ele é de quem administra **o bot**, não de quem administra " +
                            "um servidor."))
                        .AsEphemeral());
                return;
            }

            var deferBuilder = new DiscordInteractionResponseBuilder();
            if (privado)
                deferBuilder.AsEphemeral();

            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource, deferBuilder);

            var take = (int)Math.Clamp(quantidade, 1, 100);
            var level = BotLogBuffer.ParseLevel(nivel);
            var contains = string.IsNullOrWhiteSpace(filtro) ? null : filtro.Trim();

            var lines = BotLogBuffer.Snapshot(take, level, contains);
            var scope = BuildScopeText(nivel, contains, take, lines.Count);

            if (lines.Count == 0)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(new DiscordEmbedBuilder()
                    .WithTitle("Logs do bot")
                    .WithDescription(BotLogBuffer.TotalSeen == 0
                        ? "Nenhuma linha capturada ainda. O buffer começa vazio a cada reinício do bot."
                        : $"Nenhuma linha corresponde ao filtro.\n{scope}")
                    .WithColor(DiscordColor.Orange)));
                return;
            }

            var pages = BuildPages(lines, scope);

            if (pages.Count == 1)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(pages[0]));
                return;
            }

            var paginated = pages.Select(p => new Page(string.Empty, p)).ToList();

            // Mesmo cuidado de qualquer paginacao publica: sem registrar o dono, quem nao rodou
            // o comando clica na seta e o Discord so diz "interacao falhou".
            var original = await ctx.GetOriginalResponseAsync();
            PaginationOwnership.Register(original.Id, ctx.User.Id);

            try
            {
                await ctx.Interaction.SendPaginatedResponseAsync(true, privado, ctx.User, paginated);
            }
            finally
            {
                PaginationOwnership.Unregister(original.Id);
            }
        }

        /// <summary>
        /// Quem administra o BOT, e nao quem administra um servidor qualquer.
        ///
        /// Nao da para usar o ApplicationCommandRequireTeamMember do DisCatSharp:
        /// aquele atributo pressupoe aplicacao de TIME, e esta pode ser de pessoa
        /// fisica - onde a aplicacao tem Owner e nao tem Team.
        ///
        /// Falha FECHADA. Se o CurrentApplication ainda nao estiver preenchido, a
        /// resposta e "nao", pela mesma razao da checagem de hierarquia: o modo de
        /// errar aqui e entregar o log da maquina a quem nao devia.
        /// </summary>
        private static bool IsApplicationOwner(InteractionContext ctx)
        {
            var app = ctx.Client.CurrentApplication;
            if (app is null)
                return false;

            if (app.Owner is { } owner && owner.Id == ctx.User.Id)
                return true;

            // Aplicacao de time: vale qualquer membro, com o papel que for. Quem
            // esta no time da aplicacao ja pode trocar o token no portal.
            return app.Team?.Members?.Any(m => m.User?.Id == ctx.User.Id) == true;
        }

        private static string BuildScopeText(string nivel, string? contains, int take, int shown)
        {
            var counts = BotLogBuffer.Counts();
            var parts = new List<string>();

            if (!string.Equals(nivel, LevelAll, StringComparison.OrdinalIgnoreCase))
                parts.Add($"nível: `{nivel}`");
            if (contains is not null)
                parts.Add($"texto: `{Embeds.Trim(contains, 40)}`");

            var buffer = $"Buffer: {counts.Info} info · {counts.Aviso} aviso · {counts.Erro} erro.";

            return parts.Count == 0
                ? $"Últimas {shown} de até {take} linha(s). {buffer}"
                : $"Filtro — {string.Join(" · ", parts)} · {shown} linha(s). {buffer}";
        }

        /// <summary>
        /// Uma linha por entrada, em bloco de codigo para o alinhamento de
        /// "hora NIVEL texto" sobreviver. O teto por pagina e o mesmo do
        /// folgado dentro do limite de 4096 da descricao do embed.
        /// </summary>
        // 2800, e nao 3200: o `scope` e prefixado a TODA pagina e nunca era
        // contado, e a cerca de crases soma mais alguns. A descricao de um embed
        // para em 4096 e estourar derruba o /logs inteiro com "Falha no comando" -
        // justamente quando alguem esta tentando ler o log para entender um
        // incidente.
        private static List<DiscordEmbedBuilder> BuildPages(List<LogLine> lines, string scope, int maxCharsPerPage = 2800)
        {
            var chunks = new List<string>();
            var sb = new StringBuilder();

            foreach (var line in lines)
            {
                var stamp = line.TimestampUtc.ToLocalTime().ToString("HH:mm:ss");
                // Escapa ANTES de medir. Escapando depois, cada ``` virava cinco
                // caracteres e uma pagina de 3200 crus podia render 5300 - acima
                // do teto do embed. E linha de log com ``` chega ate aqui: o
                // Program escreve a excecao INTEIRA no console, e uma mensagem de
                // erro da API pode ecoar o `motivo` que alguem digitou.
                var text = $"[{stamp}] {BotLogBuffer.LevelName(line.Level),-5} {line.Text}\n"
                    .Replace("```", "`\u200b`\u200b`");

                if (sb.Length + text.Length > maxCharsPerPage)
                {
                    chunks.Add(sb.ToString());
                    sb.Clear();
                }

                sb.Append(text);
            }

            if (sb.Length > 0)
                chunks.Add(sb.ToString());

            var pages = new List<DiscordEmbedBuilder>();
            for (var i = 0; i < chunks.Count; i++)
            {
                pages.Add((new DiscordEmbedBuilder()
                    .WithTitle("Logs do bot")
                    // Zero-width space entre as crases: uma linha de log com ```
                    // dentro - que chega ate aqui por uma mensagem de erro da API
                    // ecoando o `motivo` que alguem digitou - fechava a cerca no
                    // meio e embaralhava o resto da pagina.
                    .WithDescription($"{scope}\n```\n{chunks[i]}```")
                    .WithColor(DiscordColor.Blurple)
                    .WithFooter($"Página {i + 1}/{chunks.Count} — mais recente primeiro")
                    .WithTimestamp(DateTimeOffset.UtcNow)));
            }

            return pages;
        }
    }
}
