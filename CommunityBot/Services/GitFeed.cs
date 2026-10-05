using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using CommunityBot.config;
using DisCatSharp;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.EventArgs;
using Newtonsoft.Json;

namespace CommunityBot.Services
{
    /// <summary>
    /// Feed de commits: olha um repositorio git desta maquina e posta num canal
    /// o resumo de cada commit novo, com o .patch em anexo.
    ///
    /// Por varredura, e nao por hook de post-commit: o hook dispara uma vez so,
    /// e um commit feito com o bot fora do ar (ou com a rede caindo) sumiria. Aqui
    /// o ultimo commit postado fica em disco, e a volta seguinte posta o que
    /// faltou.
    ///
    /// So LE o repositorio. Nada de fetch, checkout ou qualquer coisa que pegue
    /// lock: quem commita la nao pode tropecar no bot.
    /// </summary>
    internal static class GitFeed
    {
        private const int MaxFilesListed = 15;
        private const int MaxCommitsPerRound = 10;

        // Abaixo dos 10 MB de anexo do Discord, com folga para o resto da
        // requisicao.
        private const int MaxPatchBytes = 8 * 1024 * 1024;

        private static readonly DiscordColor s_color = new(0xB5651D);

        // O cabecalho do "fuller", mas SEM e-mail: o --format=fuller poe
        // "Author: nome <email>" e "Commit: nome <email>" no .patch, e o e-mail
        // do dono do repositorio nao tem o que fazer num canal do Discord. So
        // nome e data; %w(0,4,4) recua a mensagem como o git faz.
        private const string PatchHeaderFormat =
            "--format=commit %H%nAuthor:     %an%nAuthorDate: %ad%nCommit:     %cn%nCommitDate: %cd%n%n%w(0,4,4)%B";
        private static readonly TimeSpan s_gitTimeout = TimeSpan.FromSeconds(30);

        private static readonly IReadOnlyDictionary<string, string> s_gitEnv = new Dictionary<string, string>
        {
            // Sem isto, um `git status` qualquer poderia reescrever o index; os
            // comandos daqui nao escrevem, mas a garantia fica explicita.
            ["GIT_OPTIONAL_LOCKS"] = "0",
            ["GIT_TERMINAL_PROMPT"] = "0",
            // Erro do git em ingles e previsivel no log; saida em UTF-8.
            ["LC_ALL"] = "C.UTF-8"
        };

        private static readonly CancellationTokenSource s_stop = new();
        private static int s_started;
        private static string? s_lastReport;

        private sealed class State
        {
            public string? repoPath { get; set; }
            public string? branch { get; set; }
            public string? lastCommit { get; set; }
        }

        private static readonly string s_statePath = AppPaths.Data("git-feed.json");

        /// <summary>
        /// Liga a varredura na primeira vez que os servidores terminam de
        /// carregar - antes disso o canal nao esta no cache. Reconexao nao liga
        /// uma segunda.
        /// </summary>
        public static Task StartAsync(DiscordClient client, GuildDownloadCompletedEventArgs e)
        {
            if (Interlocked.Exchange(ref s_started, 1) == 0)
                _ = Task.Run(() => LoopAsync(client, s_stop.Token));

            return Task.CompletedTask;
        }

        public static void Stop()
        {
            try { s_stop.Cancel(); }
            catch (ObjectDisposedException) { }
        }

        private static async Task LoopAsync(DiscordClient client, CancellationToken ct)
        {
            while (!ct.IsCancellationRequested)
            {
                var settings = await ReadSettingsAsync();

                try
                {
                    await RoundAsync(client, settings, ct);
                }
                catch (OperationCanceledException) when (ct.IsCancellationRequested)
                {
                    return;
                }
                catch (Exception ex)
                {
                    Report($"[gitfeed] falha: {ex.Message}");
                }

                try
                {
                    // Desligado, olha o config de novo a cada minuto: ligar e so
                    // editar o arquivo, sem reiniciar.
                    await Task.Delay(settings.IsConfigured ? settings.PollInterval : TimeSpan.FromMinutes(1), ct);
                }
                catch (OperationCanceledException)
                {
                    return;
                }
            }
        }

        private static async Task RoundAsync(DiscordClient client, GitFeedSettings settings, CancellationToken ct)
        {
            if (!settings.IsConfigured)
            {
                Report("[gitfeed] desligado: faltam repoPath, guildId ou channelId na secao \"gitFeed\" do config.");
                return;
            }

            if (!Directory.Exists(settings.repoPath))
            {
                Report($"[gitfeed] aviso: o repositorio {settings.repoPath} nao existe.");
                return;
            }

            if (ResolveChannel(client, settings, out var problem) is not { } channel)
            {
                Report($"[gitfeed] aviso: {problem}");
                return;
            }

            var head = await HeadAsync(settings, ct);
            if (head is null)
            {
                Report($"[gitfeed] aviso: o branch \"{settings.branch}\" nao existe em {settings.repoPath}.");
                return;
            }

            var state = LoadState();
            var repoPath = Path.GetFullPath(settings.repoPath!);

            if (state?.lastCommit is null || state.repoPath != repoPath || state.branch != settings.branch)
            {
                // Primeira volta com este repositorio: posta so o commit mais
                // recente, como prova de que o canal esta certo, em vez de
                // despejar o historico inteiro.
                await PostAsync(channel, settings, head, ct);
                SaveState(new State { repoPath = repoPath, branch = settings.branch, lastCommit = head });
                ReportOnline(channel, settings);
                return;
            }

            ReportOnline(channel, settings);

            if (state.lastCommit == head)
                return;

            List<string> commits;
            var ancestor = await GitAsync(settings, ct, "merge-base", "--is-ancestor", state.lastCommit, head);
            if (ancestor.ExitCode == 0)
            {
                var list = await GitAsync(settings, ct, "rev-list", "--reverse", "--first-parent",
                    $"{state.lastCommit}..{head}");
                commits = Lines(list.StdOut);
            }
            else
            {
                // amend, rebase, reset: o ultimo postado saiu do historico. Nao
                // ha "o que veio depois" para mostrar; posta so a ponta nova.
                Console.WriteLine($"[gitfeed] aviso: o historico de {settings.branch} foi reescrito; postando so {Short(head)}.");
                commits = new List<string> { head };
            }

            if (commits.Count > MaxCommitsPerRound)
            {
                Console.WriteLine($"[gitfeed] aviso: {commits.Count} commits novos; postando so os {MaxCommitsPerRound} mais recentes.");
                commits = commits.TakeLast(MaxCommitsPerRound).ToList();
            }

            foreach (var sha in commits)
            {
                ct.ThrowIfCancellationRequested();
                await PostAsync(channel, settings, sha, ct);

                // Um por um: se o proximo falhar, a volta seguinte recomeca dele,
                // e nao do comeco da lista.
                state.lastCommit = sha;
                SaveState(state);
            }
        }

        // ------------------------------------------------------------------
        // Canal
        // ------------------------------------------------------------------

        private static DiscordChannel? ResolveChannel(DiscordClient client, GitFeedSettings settings, out string problem)
        {
            problem = string.Empty;

            if (!client.Guilds.TryGetValue(settings.guildId!.Value, out var guild))
            {
                problem = $"o bot nao esta no servidor {settings.guildId}.";
                return null;
            }

            // Pelo servidor, e nao por GetChannelAsync solto: um channelId de
            // outro servidor no config nao pode fazer o feed vazar para la.
            var channel = guild.GetChannel(settings.channelId!.Value);
            if (channel is null)
            {
                problem = $"o canal {settings.channelId} nao existe em \"{guild.Name}\" (ou o bot nao o enxerga).";
                return null;
            }

            const Permissions needed = Permissions.AccessChannels | Permissions.SendMessages |
                                       Permissions.EmbedLinks | Permissions.AttachFiles;
            var has = guild.CurrentMember is { } self ? channel.PermissionsFor(self) : Permissions.None;
            var missing = needed & ~has;
            if (missing != Permissions.None)
            {
                problem = $"faltam permissoes em #{channel.Name} (\"{guild.Name}\"): {missing}.";
                return null;
            }

            return channel;
        }

        // ------------------------------------------------------------------
        // Git
        // ------------------------------------------------------------------

        private static Task<ProcessRunner.ProcessResult> GitAsync(GitFeedSettings settings, CancellationToken ct,
            params string[] args)
        {
            var all = new List<string>
            {
                "-C", settings.repoPath!,
                // Caminho com acento sai como texto, e nao como "\303\241".
                "-c", "core.quotePath=false",
                "-c", "color.ui=never"
            };
            all.AddRange(args);
            return ProcessRunner.RunAsync(settings.gitPath, all, s_gitTimeout, ct, s_gitEnv);
        }

        private static async Task<string?> HeadAsync(GitFeedSettings settings, CancellationToken ct)
        {
            var result = await GitAsync(settings, ct, "rev-parse", "--verify", "--quiet",
                $"refs/heads/{settings.branch}^{{commit}}");
            var sha = result.StdOut.Trim();
            return result.ExitCode == 0 && sha.Length >= 40 ? sha : null;
        }

        private sealed record FileChange(string Path, int? Added, int? Deleted)
        {
            public int Weight => (Added ?? 0) + (Deleted ?? 0);
        }

        private static async Task PostAsync(DiscordChannel channel, GitFeedSettings settings, string sha,
            CancellationToken ct)
        {
            // %x00 separa os campos: nenhum deles pode conter um NUL, e qualquer
            // outro separador poderia aparecer dentro da mensagem do commit.
            var meta = await GitAsync(settings, ct, "show", "-s", "--format=%h%x00%an%x00%aI%x00%s%x00%b", sha);
            var fields = meta.StdOut.Split('\0');
            if (meta.ExitCode != 0 || fields.Length < 5)
                throw new InvalidOperationException($"git show de {Short(sha)} saiu com {meta.ExitCode}: {meta.StdErr.Trim()}");

            var (shortSha, author, date, subject, body) = (fields[0], fields[1], fields[2], fields[3], fields[4].Trim());

            // --diff-merges=first-parent: merge mostra o que entrou no branch, e
            // o commit raiz mostra tudo como adicionado.
            var numstat = await GitAsync(settings, ct, "show", "--format=", "--numstat", "-M",
                "--diff-merges=first-parent", sha);
            var files = Lines(numstat.StdOut).Select(ParseNumstat).Where(f => f is not null).Select(f => f!).ToList();

            var patch = await GitAsync(settings, ct, "show", PatchHeaderFormat, "--stat", "--patch", "-M",
                "--diff-merges=first-parent", sha);
            var patchBytes = Encoding.UTF8.GetBytes(patch.StdOut);

            var embed = BuildEmbed(settings, shortSha, author, date, subject, body, files,
                attachPatch: patchBytes.Length <= MaxPatchBytes);

            var message = new DiscordMessageBuilder()
                .AddEmbed(embed)
                // A mensagem do commit e texto livre: um "@everyone" nela nao
                // pode notificar o servidor.
                .WithAllowedMentions(Array.Empty<IMention>());

            using var stream = new MemoryStream(patchBytes);
            if (patchBytes.Length <= MaxPatchBytes)
                message.AddFile($"{shortSha}.patch", stream);

            await channel.SendMessageAsync(message);
            Console.WriteLine($"[gitfeed] postado {shortSha} em #{channel.Name} ({files.Count} arquivo(s))");
        }

        private static DiscordEmbed BuildEmbed(GitFeedSettings settings, string shortSha, string author, string date,
            string subject, string body, List<FileChange> files, bool attachPatch)
        {
            var added = files.Sum(f => f.Added ?? 0);
            var deleted = files.Sum(f => f.Deleted ?? 0);

            var text = new StringBuilder();
            if (body.Length > 0)
                text.AppendLine(Embeds.SafeTrim(body, 1500)).AppendLine();

            if (files.Count > 0)
            {
                // Os mais mexidos primeiro: e o que diz do que o commit trata.
                // Num bloco de codigo, onde "_" e "*" de nome de arquivo nao
                // viram italico.
                var listed = files.OrderByDescending(f => f.Weight).Take(MaxFilesListed).ToList();
                text.AppendLine("```");
                foreach (var f in listed)
                {
                    var counts = f.Added is null ? "binary" : $"+{f.Added} -{f.Deleted}";
                    text.AppendLine($"{counts,-13} {Embeds.Trim(f.Path.Replace("`", "'"), 90)}");
                }
                text.AppendLine("```");

                if (files.Count > listed.Count)
                    text.AppendLine($"…and {files.Count - listed.Count} more file(s).");
            }
            else
            {
                text.AppendLine("*No file changes.*");
            }

            if (!attachPatch)
                text.AppendLine("*The full patch is too large to attach.*");

            var embed = new DiscordEmbedBuilder()
                .WithAuthor(Embeds.Trim($"{settings.title} · {settings.branch}", 256))
                // Titulo de embed nao renderiza link mascarado; escapar ali so
                // deixaria a barra invertida a mostra.
                .WithTitle(Embeds.Trim(subject, 256))
                .WithDescription(Embeds.Trim(text.ToString(), 4000))
                .WithFooter(Embeds.Trim($"{shortSha} · {author} · {files.Count} file(s) · +{added} -{deleted}", 2048))
                .WithColor(s_color);

            if (DateTimeOffset.TryParse(date, out var when))
                embed.WithTimestamp(when);

            return embed.Build();
        }

        /// <summary>"12\t3\tcaminho", ou "-\t-\tcaminho" para binario.</summary>
        private static FileChange? ParseNumstat(string line)
        {
            var parts = line.Split('\t', 3);
            if (parts.Length != 3)
                return null;

            int? Parse(string s) => int.TryParse(s, out var n) ? n : null;
            return new FileChange(parts[2], Parse(parts[0]), Parse(parts[1]));
        }

        // ------------------------------------------------------------------
        // Estado e log
        // ------------------------------------------------------------------

        private static State? LoadState()
        {
            try
            {
                return File.Exists(s_statePath)
                    ? JsonConvert.DeserializeObject<State>(File.ReadAllText(s_statePath))
                    : null;
            }
            catch (Exception ex)
            {
                // Estado ilegivel vale como primeira volta: posta o commit mais
                // recente e segue dali. Pior caso, um commit repetido no canal.
                Console.WriteLine($"[gitfeed] aviso: {s_statePath} ilegivel ({ex.Message}); recomecando do commit atual.");
                return null;
            }
        }

        /// <summary>Grava por .tmp + rename: um crash no meio nao deixa o arquivo pela metade.</summary>
        private static void SaveState(State state)
        {
            var tmp = s_statePath + ".tmp";
            File.WriteAllText(tmp, JsonConvert.SerializeObject(state, Formatting.Indented));
            File.Move(tmp, s_statePath, overwrite: true);
        }

        private static void ReportOnline(DiscordChannel channel, GitFeedSettings settings) =>
            Report($"[gitfeed] ligado: {settings.repoPath} ({settings.branch}) -> #{channel.Name} " +
                   $"em \"{channel.Guild?.Name}\", a cada {settings.PollInterval.TotalSeconds:0}s");

        /// <summary>
        /// Uma linha por mudanca de estado. Sem isto, um canal sem permissao
        /// renderia a mesma linha de aviso a cada 30 segundos, para sempre.
        /// </summary>
        private static void Report(string line)
        {
            if (Interlocked.Exchange(ref s_lastReport, line) != line)
                Console.WriteLine(line);
        }

        private static async Task<GitFeedSettings> ReadSettingsAsync()
        {
            try
            {
                var reader = new JSONReader();
                await reader.ReadJSON();
                return reader.gitFeed;
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[gitfeed] aviso: nao consegui ler o config: {ex.Message}");
                return new GitFeedSettings();
            }
        }

        private static List<string> Lines(string text) =>
            text.Split('\n', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries).ToList();

        private static string Short(string sha) => sha.Length > 7 ? sha[..7] : sha;
    }
}
