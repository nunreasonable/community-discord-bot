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
        [SlashCommand("logs", "Show the bot's latest logs", (long)Permissions.Administrator)]
        [ApplicationCommandRequireUserPermissions(Permissions.Administrator)]
        public async Task LogsCommand(
            InteractionContext ctx,
            [Option("amount", "How many lines to show (1–100, default 25)")] long quantidade = 25,
            [Choice("All", LevelAll)]
            [Choice("Errors only", LevelError)]
            [Choice("Warnings only", LevelWarn)]
            [Choice("Info only", LevelInfo)]
            [Option("level", "Filter by severity")] string nivel = LevelAll,
            [Option("filter", "Only show lines that contain this text")] string? filtro = null,
            [Option("private", "Only show the result to you")] bool privado = true)
        {
            if (!IsApplicationOwner(ctx))
            {
                await ctx.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                    new DiscordInteractionResponseBuilder()
                        .AddEmbed(Embeds.Error("Command denied",
                            "`/logs` shows the log for the **entire process** — what the bot did in " +
                            "every server it's in, including user IDs, exception messages and " +
                            "file paths on the host machine.\n\n" +
                            "That's why it's reserved for whoever runs **the bot**, not whoever runs " +
                            "a server."))
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
                    .WithTitle("Bot logs")
                    .WithDescription(BotLogBuffer.TotalSeen == 0
                        ? "No lines captured yet. The buffer starts empty every time the bot restarts."
                        : $"No lines match the filter.\n{scope}")
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

            // Mostra o nome exibido do nivel (ERROR/WARN/INFO), e nao o valor
            // interno da escolha ("erro"/"aviso"), que e em portugues.
            if (!string.Equals(nivel, LevelAll, StringComparison.OrdinalIgnoreCase))
            {
                var levelLabel = BotLogBuffer.ParseLevel(nivel) is { } parsed ? BotLogBuffer.LevelName(parsed) : nivel;
                parts.Add($"level: `{levelLabel}`");
            }
            if (contains is not null)
                parts.Add($"text: `{Embeds.Trim(contains, 40)}`");

            var buffer = $"Buffer: {counts.Info} info · {counts.Aviso} warn · {counts.Erro} error.";

            return parts.Count == 0
                ? $"Latest {shown} of up to {take} line(s). {buffer}"
                : $"Filter — {string.Join(" · ", parts)} · {shown} line(s). {buffer}";
        }

        /// <summary>
        /// Uma linha por entrada, em bloco de codigo para o alinhamento de
        /// "hora NIVEL texto" sobreviver. O teto por pagina e o mesmo do
        /// folgado dentro do limite de 4096 da descricao do embed.
        /// </summary>
        // 2800, e nao 3200: o `scope` e prefixado a TODA pagina e nunca era
        // contado, e a cerca de crases soma mais alguns. A descricao de um embed
        // para em 4096 e estourar derruba o /logs inteiro com "Command failed" -
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
                // erro da API pode ecoar o `reason` que alguem digitou.
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
                    .WithTitle("Bot logs")
                    // Zero-width space entre as crases: uma linha de log com ```
                    // dentro - que chega ate aqui por uma mensagem de erro da API
                    // ecoando o `reason` que alguem digitou - fechava a cerca no
                    // meio e embaralhava o resto da pagina.
                    .WithDescription($"{scope}\n```\n{chunks[i]}```")
                    .WithColor(DiscordColor.Blurple)
                    .WithFooter($"Page {i + 1}/{chunks.Count} — newest first")
                    .WithTimestamp(DateTimeOffset.UtcNow)));
            }

            return pages;
        }
    }
}
