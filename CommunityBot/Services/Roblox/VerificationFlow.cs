using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Threading;
using System.Threading.Tasks;
using CommunityBot.config;
using DisCatSharp;
using DisCatSharp.Entities;
using DisCatSharp.Enums;
using DisCatSharp.Enums.Core;
using DisCatSharp.EventArgs;

namespace CommunityBot.Services.Roblox
{
    /// <summary>
    /// O caminho do /verify e dos botoes dele.
    ///
    /// Como funciona, de ponta a ponta:
    ///
    /// 1. O /verify (ou o botao do painel) responde, so para quem pediu, com um
    ///    botao-link para daeese.me/oauth/roblox/start. O link e o MESMO para
    ///    todo mundo e nao carrega identidade nenhuma.
    /// 2. No navegador, o Worker faz a pessoa entrar com o Discord e depois com o
    ///    Roblox. As duas identidades ficam provadas no mesmo navegador, e o
    ///    Worker guarda "Discord X e dono do Roblox Y" por meia hora.
    /// 3. Aqui, o bot pergunta ao Worker a cada poucos segundos se ja existe
    ///    resultado para aquele id do Discord. Quando existe, grava o vinculo,
    ///    aplica no servidor e edita a resposta.
    ///
    /// O passo pelo Discord no navegador e o que fecha o golpe do link repassado:
    /// se o link carregasse um codigo do /verify, bastaria mandar esse link para
    /// a vitima - ela autorizaria o Roblox DELA e a conta cairia no Discord de
    /// quem mandou. Sendo o Discord do navegador que decide, a vitima vincularia
    /// a propria conta a si mesma.
    ///
    /// Custom ids com prefixo "verify:". Este handler marca Handled so para eles,
    /// como o TicketComponents faz com "ticket:".
    /// </summary>
    internal static class VerificationFlow
    {
        public const string Prefix = "verify:";

        public const string StartId = Prefix + "start";
        private const string CheckId = Prefix + "check";
        public const string UnlinkId = Prefix + "unlink";

        private static readonly TimeSpan PollInterval = TimeSpan.FromSeconds(4);

        /// <summary>
        /// Abaixo dos 15 minutos de vida do token da interacao, que e o que
        /// permite editar a resposta quando o resultado chega.
        /// </summary>
        private static readonly TimeSpan PollWindow = TimeSpan.FromMinutes(10);

        /// <summary>Um polling por pessoa: um /verify novo substitui o anterior.</summary>
        private static readonly ConcurrentDictionary<ulong, CancellationTokenSource> s_polls = new();

        /// <summary>
        /// Uma conclusao por pessoa de cada vez. Sem isto o polling e o botao "I've
        /// authorized" podiam consumir o resultado em paralelo - um levava, o outro
        /// dizia "Nothing received yet" para quem acabou de verificar.
        /// </summary>
        private static readonly ConcurrentDictionary<ulong, SemaphoreSlim> s_gates = new();

        /// <summary>O /verify e o botao do painel caem aqui.</summary>
        public static async Task BeginAsync(DiscordClient client, DiscordInteraction interaction, DiscordGuild guild,
            DiscordMember member)
        {
            if (RobloxLinkStore.For(member.Id) is { } existing)
            {
                await interaction.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                    new DiscordInteractionResponseBuilder().AsEphemeral());

                var report = await VerificationService.ApplyAsync(client, guild, member, existing,
                    AuditReason.Automatic("Roblox verification: /verify by an already linked member"));

                await interaction.EditOriginalResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    LinkedEmbed("You're already verified", existing, report,
                        "To switch Roblox accounts, run `/unverify` and then `/verify` again.")));
                return;
            }

            var settings = await ReadSettingsAsync();
            if (!settings.IsConfigured)
            {
                await interaction.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                    new DiscordInteractionResponseBuilder().AsEphemeral().AddEmbed(Embeds.Error("Verification unavailable",
                        "Whoever runs the bot hasn't turned on Roblox verification yet. Nothing is wrong on your end.")));
                return;
            }

            await interaction.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                new DiscordInteractionResponseBuilder()
                    .AsEphemeral()
                    .AddEmbed(PromptEmbed())
                    .AddComponents(
                        new DiscordLinkButtonComponent(settings.startUrl, "Open verification"),
                        new DiscordButtonComponent(ButtonStyle.Secondary, CheckId, "I've authorized")));

            StartPolling(client, interaction, guild.Id, member.Id);
        }

        public static Task OnComponent(DiscordClient client, ComponentInteractionCreateEventArgs e)
        {
            var id = e.Interaction.Data?.CustomId ?? e.Id;
            if (string.IsNullOrEmpty(id) || !id.StartsWith(Prefix, StringComparison.Ordinal))
                return Task.CompletedTask;

            // Sincrono, antes de qualquer await: o despachante so olha Handled
            // depois que este metodo retorna.
            e.Handled = true;

            var interaction = e.Interaction;
            _ = Task.Run(async () =>
            {
                try
                {
                    // O Unlink desfaz um vinculo GLOBAL: vale em DM e como app
                    // pessoal. So mexe em cargo e apelido quando o bot esta no
                    // servidor - num servidor sem ele, nao ha o que mexer.
                    if (id == UnlinkId)
                    {
                        var botGuild = CommandScope.BotGuild(client, e.Guild);
                        await UnlinkAsync(client, interaction, e.User, botGuild, botGuild is null ? null : e.Member);
                        return;
                    }

                    if (e.Guild is not { } guild || e.Member is not { } member)
                    {
                        await TryFailAsync(interaction, "This only works inside a server.");
                        return;
                    }

                    switch (id)
                    {
                        case StartId:
                            await BeginAsync(client, interaction, guild, member);
                            return;

                        case CheckId:
                            await CheckAsync(client, interaction, guild, member);
                            return;

                        default:
                            Console.WriteLine($"[verificacao] custom id desconhecido: '{id}'");
                            await TryFailAsync(interaction, "This button is from an old version. Run `/verify` again.");
                            return;
                    }
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"[verificacao] falha ao tratar '{id}' de {e.User.Id}: {ex}");
                    await TryFailAsync(interaction, "Something broke while processing this action. Let whoever runs the bot know.");
                }
            });

            return Task.CompletedTask;
        }

        public static DiscordEmbed LinkedEmbed(string title, RobloxLink link, ApplyReport? report, string? footer = null)
        {
            var embed = new DiscordEmbedBuilder()
                .WithTitle(title)
                .WithColor(DiscordColor.SpringGreen)
                .WithDescription($"Roblox account: {Describe(link)}");

            if (report is null || !report.Configured)
            {
                embed.AddField(new DiscordEmbedField("In this server",
                    "This server hasn't set up verification roles yet. The link already applies in every server " +
                    "that uses Sollarety.", false));
            }
            else
            {
                if (report.Refusal is not null)
                    embed.AddField(new DiscordEmbedField("Not approved in this server",
                        Embeds.Trim("The account is linked, but " + report.Refusal, 1024), false));

                embed.AddField(new DiscordEmbedField("In this server", report.Describe(), false));
            }

            if (footer is not null)
                embed.WithFooter(footer);

            return embed.Build();
        }

        /// <summary>"**Exibicao** (@usuario)", com link para o perfil.</summary>
        public static string Describe(RobloxLink link)
        {
            var name = Embeds.Safe(link.robloxName);
            var display = Embeds.Safe(string.IsNullOrWhiteSpace(link.displayName) ? link.robloxName : link.displayName);

            // O link mascarado aqui e seguro: a URL e montada pelo bot com o id
            // numerico, nao vem de ninguem.
            return $"**{display}** ([@{name}]({RobloxApi.ProfileUrl(link.robloxId)}))";
        }

        public static async Task<RobloxVerifySettings> ReadSettingsAsync()
        {
            try
            {
                var reader = new JSONReader();
                await reader.ReadJSON();
                return reader.robloxVerify;
            }
            catch (Exception ex)
            {
                // Config ilegivel no meio de uma edicao nao pode derrubar o
                // comando: vale como "verificacao desligada" ate a proxima leitura.
                Console.WriteLine($"[verificacao] aviso: nao consegui ler o config: {ex.Message}");
                return new RobloxVerifySettings();
            }
        }

        // ------------------------------------------------------------------
        // Conclusao
        // ------------------------------------------------------------------

        private enum Outcome { Pending, Linked, Failed }

        private readonly record struct Completion(Outcome Outcome, RobloxLink? Link, ApplyReport? Report, string? Error);

        /// <summary>
        /// Busca o resultado no Worker e, se houver, grava e aplica.
        /// <paramref name="wait"/> falso e o polling: se ja ha uma conclusao em
        /// andamento, ele so pula a rodada.
        /// </summary>
        private static async Task<Completion> TryCompleteAsync(DiscordClient client, ulong guildId, ulong userId,
            bool wait, CancellationToken ct)
        {
            var gate = s_gates.GetOrAdd(userId, _ => new SemaphoreSlim(1, 1));
            if (!await gate.WaitAsync(wait ? TimeSpan.FromSeconds(20) : TimeSpan.Zero, ct))
                return new Completion(Outcome.Pending, null, null, null);

            try
            {
                var take = await VerifyWorkerClient.TakeAsync(await ReadSettingsAsync(), userId, ct);

                if (take.Status == TakeStatus.Failed)
                    return new Completion(Outcome.Failed, null, null, take.Error);

                if (take.Status == TakeStatus.Pending)
                    return new Completion(Outcome.Pending, null, null, null);

                var account = take.Account!;

                // Do Worker vale so o id, que e o que o OAuth prova. Nome,
                // exibicao e data de criacao vem da API publica, que e a mesma
                // fonte do /update: assim os dois nunca discordam. Se ela falhar,
                // fica o que o OAuth disse.
                var user = await RobloxApi.GetUserAsync(account.RobloxId, fresh: true, ct);
                var link = await RobloxLinkStore.Instance.SetAsync(new RobloxLink
                {
                    discordId = userId,
                    robloxId = account.RobloxId,
                    robloxName = user.Value?.Name ?? account.Name,
                    displayName = user.Value?.DisplayName ?? account.DisplayName,
                    robloxCreatedUtc = user.Value?.Created ?? account.Created,
                    verifiedAtUtc = DateTimeOffset.UtcNow
                });

                Console.WriteLine($"[verificacao] {userId} vinculado a {link.robloxName} ({link.robloxId})");

                ApplyReport? report = null;
                if (client.Guilds.TryGetValue(guildId, out var guild) &&
                    await Hierarchy.TryGetMemberAsync(guild, userId) is { } member)
                {
                    report = await VerificationService.ApplyAsync(client, guild, member, link,
                        AuditReason.Automatic($"Roblox verification: linked to {link.robloxName} ({link.robloxId})"));
                }

                return new Completion(Outcome.Linked, link, report, null);
            }
            finally
            {
                gate.Release();
            }
        }

        private static void StartPolling(DiscordClient client, DiscordInteraction interaction, ulong guildId, ulong userId)
        {
            var cts = new CancellationTokenSource(PollWindow);

            // Um /verify novo substitui o anterior: a resposta que vale editar e a
            // mais recente, e dois pollings pela mesma pessoa so gastariam o Worker.
            s_polls.AddOrUpdate(userId, cts, (_, old) =>
            {
                old.Cancel();
                return cts;
            });

            _ = Task.Run(async () =>
            {
                var failureLogged = false;
                try
                {
                    while (true)
                    {
                        var result = await TryCompleteAsync(client, guildId, userId, wait: false, cts.Token);

                        if (result.Outcome == Outcome.Linked)
                        {
                            await EditQuietlyAsync(interaction,
                                LinkedEmbed("Account verified", result.Link!, result.Report));
                            return;
                        }

                        // Falha do Worker nao encerra o polling: pode ser uma
                        // instabilidade passageira. Vai para o log uma vez so.
                        if (result.Outcome == Outcome.Failed && !failureLogged)
                        {
                            Console.WriteLine($"[verificacao] aviso: polling de {userId}: {result.Error}");
                            failureLogged = true;
                        }

                        await Task.Delay(PollInterval, cts.Token);
                    }
                }
                catch (OperationCanceledException)
                {
                    // Tempo esgotado ou substituido por um /verify novo. O botao
                    // "I've authorized" continua funcionando sem polling nenhum.
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"[verificacao] falha no polling de {userId}: {ex}");
                }
                finally
                {
                    s_polls.TryRemove(new KeyValuePair<ulong, CancellationTokenSource>(userId, cts));
                    cts.Dispose();
                }
            });
        }

        private static void StopPolling(ulong userId)
        {
            if (s_polls.TryRemove(userId, out var cts))
            {
                try
                {
                    cts.Cancel();
                }
                catch (ObjectDisposedException)
                {
                    // O polling terminou sozinho entre o TryRemove e o Cancel.
                }
            }
        }

        /// <summary>
        /// "I've authorized". Funciona mesmo sem polling ativo - depois de o bot
        /// reiniciar, por exemplo - porque tudo o que precisa vem do clique.
        /// </summary>
        private static async Task CheckAsync(DiscordClient client, DiscordInteraction interaction, DiscordGuild guild,
            DiscordMember member)
        {
            // Atualiza a propria mensagem efemera onde o botao esta.
            await interaction.CreateResponseAsync(InteractionResponseType.DeferredMessageUpdate);

            var result = await TryCompleteAsync(client, guild.Id, member.Id, wait: true, CancellationToken.None);

            if (result.Outcome == Outcome.Linked)
            {
                StopPolling(member.Id);
                await interaction.EditOriginalResponseAsync(new DiscordWebhookBuilder()
                    .AddEmbed(LinkedEmbed("Account verified", result.Link!, result.Report)), ModifyMode.Replace);
                return;
            }

            if (result.Outcome == Outcome.Failed)
            {
                await FollowUpAsync(interaction, Embeds.Error("Couldn't check",
                    $"Something went wrong while fetching the result: {result.Error}. Try again in a few seconds."));
                return;
            }

            // Nada pendente, mas o polling pode ter concluido um instante antes.
            if (RobloxLinkStore.For(member.Id) is { } link)
            {
                StopPolling(member.Id);
                var report = await VerificationService.ApplyAsync(client, guild, member, link,
                    AuditReason.Automatic("Roblox verification: checked via button"));
                await interaction.EditOriginalResponseAsync(new DiscordWebhookBuilder()
                    .AddEmbed(LinkedEmbed("Account verified", link, report)), ModifyMode.Replace);
                return;
            }

            await FollowUpAsync(interaction, Embeds.Info("Nothing received yet",
                "Finish both logins on the page that opened — Discord first, then Roblox — and click here again.\n\n" +
                "If the page said it worked and still nothing arrives, check that it showed **this** Discord " +
                "account: your browser may be logged into a different one."));
        }

        private static async Task UnlinkAsync(DiscordClient client, DiscordInteraction interaction, DiscordUser user,
            DiscordGuild? guild, DiscordMember? member)
        {
            await interaction.CreateResponseAsync(InteractionResponseType.DeferredMessageUpdate);

            StopPolling(user.Id);
            var removed = await RobloxLinkStore.Instance.RemoveAsync(user.Id);
            if (removed is null)
            {
                await interaction.EditOriginalResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Info("Nothing to undo", "Your Discord account was no longer linked to any Roblox account.")),
                    ModifyMode.Replace);
                return;
            }

            Console.WriteLine($"[verificacao] {user.Id} desvinculou {removed.robloxName} ({removed.robloxId})");

            ApplyReport? report = null;
            if (guild is not null && member is not null)
            {
                var reason = AuditReason.Automatic("Roblox verification: /unverify");
                report = await VerificationService.ApplyAsync(client, guild, member, null, reason);
                await VerificationService.ClearNicknameIfOursAsync(client, guild, member, removed, reason, report);
            }

            var embed = new DiscordEmbedBuilder()
                .WithTitle("Account unlinked")
                .WithColor(DiscordColor.SpringGreen)
                .WithDescription($"{Describe(removed)} is no longer linked to your Discord account, in any server.\n\n" +
                                 (guild is null
                                     ? "In servers that use Sollarety, verification roles are removed on the next `/update`."
                                     : "In other servers, verification roles are removed on the next `/update`."));

            if (report is { Configured: true })
                embed.AddField(new DiscordEmbedField("In this server", report.Describe(), false));

            await interaction.EditOriginalResponseAsync(new DiscordWebhookBuilder().AddEmbed(embed), ModifyMode.Replace);
        }

        private static DiscordEmbed PromptEmbed() =>
            new DiscordEmbedBuilder()
                .WithTitle("Verify your Roblox account")
                .WithColor(DiscordColor.Blurple)
                .WithDescription(
                    "1. Click **Open verification**. Sign in with **this same Discord account**, then " +
                    "with your Roblox account.\n" +
                    "2. Come back here. I'll check automatically every few seconds for 10 minutes; if nothing changes, " +
                    "click **I've authorized**.\n\n" +
                    "The login is Roblox's official one: your password never goes through the bot. Only the Roblox " +
                    "account's id, name and creation date are stored, until you run `/unverify`.")
                .WithFooter("Roblox only lets accounts aged 13 or older authorize apps.")
                .Build();

        private static async Task EditQuietlyAsync(DiscordInteraction interaction, DiscordEmbed embed)
        {
            try
            {
                await interaction.EditOriginalResponseAsync(new DiscordWebhookBuilder().AddEmbed(embed), ModifyMode.Replace);
            }
            catch (Exception ex)
            {
                // A pessoa pode ter dispensado a mensagem efemera. O vinculo ja
                // foi gravado e aplicado; so o aviso visual se perdeu.
                Console.WriteLine($"[verificacao] nao consegui editar a resposta do /verify: {ex.Message}");
            }
        }

        private static async Task FollowUpAsync(DiscordInteraction interaction, DiscordEmbed embed)
        {
            try
            {
                await interaction.CreateFollowupMessageAsync(new DiscordFollowupMessageBuilder().AddEmbed(embed).AsEphemeral());
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[verificacao] nao consegui responder ao botao: {ex.Message}");
            }
        }

        private static async Task TryFailAsync(DiscordInteraction interaction, string message)
        {
            try
            {
                await interaction.CreateResponseAsync(InteractionResponseType.ChannelMessageWithSource,
                    new DiscordInteractionResponseBuilder().AddEmbed(Embeds.Error("Error", message)).AsEphemeral());
            }
            catch
            {
                // Ja tinha respondido (defer): o follow-up e o que resta.
                await FollowUpAsync(interaction, Embeds.Error("Error", message));
            }
        }
    }
}
