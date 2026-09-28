using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading.Tasks;
using CommunityBot.Services;
using CommunityBot.Services.Roblox;
using DisCatSharp.ApplicationCommands;
using DisCatSharp.ApplicationCommands.Attributes;
using DisCatSharp.ApplicationCommands.Context;
using DisCatSharp.Entities;
using DisCatSharp.Enums;

namespace CommunityBot.commands
{
    /// <summary>
    /// Configuracao do bot, por servidor, feita por quem manda no servidor.
    ///
    /// Antes tudo isto vivia num arquivo na maquina do bot, o que exigia acesso
    /// SSH para apontar um canal - e, pior, as chaves eram globais do processo:
    /// com o bot em mais de um servidor, o log de moderacao de um podia cair no
    /// canal de outro.
    ///
    /// Manage Server e o portao, mesmo criterio do /ticket-painel. O dono sempre
    /// a tem e pode delegar sem entregar a conta.
    ///
    /// Cada subcomando mexe em UMA chave e tem UMA opcao opcional, onde omitir
    /// significa limpar. Um comando so com varias opcoes opcionais seria mais
    /// curto e ambiguo: nao daria para distinguir "nao mexa nisso" de "limpe
    /// isso".
    /// </summary>
    [SlashCommandGroup("config", "Configura o bot neste servidor", (long)Permissions.ManageGuild)]
    [ApplicationCommandRequireGuild]
    [ApplicationCommandRequireUserPermissions(Permissions.ManageGuild)]
    internal class Config : ApplicationCommandsModule
    {
        [SlashCommand("ver", "Mostra a configuração atual deste servidor")]
        public async Task ShowCommand(InteractionContext ctx)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var guild = ctx.Guild!;
            var settings = GuildSettingsStore.For(guild.Id);

            var embed = new DiscordEmbedBuilder()
                .WithTitle($"Configuração de {guild.Name}")
                .WithColor(DiscordColor.Blurple)
                .AddField(new DiscordEmbedField("Log de moderação",
                    Describe(guild, settings?.moderationLogChannelId, "`/config log-moderacao`"), false))
                .AddField(new DiscordEmbedField("Softban automático",
                    Describe(guild, settings?.autoSoftbanChannelId, "`/config auto-softban`"), false))
                .AddField(new DiscordEmbedField("Tickets — categoria",
                    Describe(guild, settings?.ticketCategoryId, "`/config tickets-categoria`"), true))
                .AddField(new DiscordEmbedField("Tickets — cargo da equipe",
                    settings?.ticketStaffRoleId is { } r && guild.GetRole(r) is { } role
                        ? role.Mention
                        : settings?.ticketStaffRoleId is not null
                            ? $"⚠️ cargo `{settings.ticketStaffRoleId}` não existe mais"
                            : "— *(`/config tickets-cargo`)*", true))
                .AddField(new DiscordEmbedField("Tickets — canal de log",
                    Describe(guild, settings?.ticketLogChannelId, "`/config tickets-log`"), true))
                .AddField(new DiscordEmbedField("Verificação — cargo",
                    DescribeRole(guild, settings?.verifiedRoleId, "`/config verificacao-cargo`"), true))
                .AddField(new DiscordEmbedField("Verificação — não verificado",
                    DescribeRole(guild, settings?.unverifiedRoleId, "`/config verificacao-nao-verificado`"), true))
                .AddField(new DiscordEmbedField("Verificação — apelido",
                    NicknameFormats.Describe(settings?.nicknameFormat), true))
                .AddField(new DiscordEmbedField("Verificação — idade mínima",
                    settings?.minAccountAgeDays is > 0 and var days ? $"{days} dia(s)" : "desligada", true))
                .AddField(new DiscordEmbedField("Binds de grupo",
                    settings?.groupBinds is { Count: > 0 } binds ? $"{binds.Count} (`/bind listar`)" : "— *(`/bind adicionar`)*", true));

            var verifyReady = (await VerificationFlow.ReadSettingsAsync()).IsConfigured;
            var pending = Pending(guild, settings, verifyReady);
            if (pending.Count > 0)
                embed.AddField(new DiscordEmbedField("Falta para funcionar",
                    string.Join("\n", pending.Select(p => $"• {p}")), false));

            if (settings?.updatedAtUtc is { } when)
                embed.WithFooter($"Última alteração por {settings.updatedById} em {when:yyyy-MM-dd HH:mm} UTC");

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(embed));
        }

        [SlashCommand("log-moderacao", "Canal que recebe um registro por ação de moderação")]
        public Task ModLogCommand(
            InteractionContext ctx,
            [ChannelTypes(ChannelType.Text, ChannelType.News)]
            [Option("canal", "Deixe vazio para desligar o registro")] DiscordChannel? canal = null) =>
            ApplyAsync(ctx, canal,
                needs: Permissions.SendMessages | Permissions.EmbedLinks,
                assign: (s, v) => s.moderationLogChannelId = v,
                onSet: c => Embeds.Ok("Log de moderação ligado",
                    $"As ações de moderação passam a ser registradas em {c.Mention}."),
                onClear: () => Embeds.Ok("Log de moderação desligado",
                    "Nada mais será registrado além do Audit Log do próprio Discord."));

        [SlashCommand("auto-softban", "Canal-armadilha: quem escrever nele leva softban automático")]
        public Task AutoSoftbanCommand(
            InteractionContext ctx,
            [Option("canal", "Deixe vazio para desarmar a armadilha")] DiscordChannel? canal = null) =>
            ApplyAsync(ctx, canal,
                needs: Permissions.AccessChannels,
                assign: (s, v) => s.autoSoftbanChannelId = v,
                // Este comando arma um recurso que bane sozinho. A resposta diz
                // exatamente o que vai acontecer, em vez de um "ok" de uma linha.
                onSet: c => Embeds.Ok("Armadilha armada",
                    $"**Qualquer mensagem em {c.Mention} passa a render um softban automático** — " +
                    "banimento seguido de desbanimento, que apaga os últimos 7 dias da pessoa e a deixa " +
                    "livre para voltar por convite.\n\n" +
                    "Ficam de fora bots e webhooks, mensagens de sistema, quem tem Ban Members, " +
                    "Gerenciar Servidor ou Administrador, o dono, e quem estiver acima do meu cargo. " +
                    "Publicações de fórum contam, porque são threads filhas do canal.\n\n" +
                    "Passando de 5 softbans em 60 segundos o vigia se desarma sozinho até o bot reiniciar."),
                onClear: () => Embeds.Ok("Armadilha desarmada",
                    "Ninguém mais será punido automaticamente por escrever em canal nenhum."));

        [SlashCommand("tickets-categoria", "Categoria onde os canais de ticket são criados")]
        public Task TicketCategoryCommand(
            InteractionContext ctx,
            [ChannelTypes(ChannelType.Category)]
            [Option("categoria", "Deixe vazio para desligar os tickets")] DiscordChannel? categoria = null) =>
            ApplyAsync(ctx, categoria,
                needs: Permissions.None,
                assign: (s, v) => s.ticketCategoryId = v,
                onSet: c => Embeds.Ok("Categoria dos tickets definida",
                    $"Os canais de ticket nascem em **{c.Name}**. Confira o resto com `/config ver`."),
                onClear: () => Embeds.Ok("Tickets desligados",
                    "Sem categoria não há onde criar os canais, então o `/ticket` passa a recusar."));

        [SlashCommand("tickets-cargo", "Cargo da equipe que enxerga todos os tickets")]
        public async Task TicketRoleCommand(
            InteractionContext ctx,
            [Option("cargo", "Deixe vazio para desligar os tickets")] DiscordRole? cargo = null)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            await GuildSettingsStore.Instance.SetAsync(ctx.Guild!.Id, ctx.User.Id,
                s => s.ticketStaffRoleId = cargo?.Id);

            if (cargo is null)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Ok("Tickets desligados",
                        "Sem cargo da equipe não há a quem dar acesso, então o `/ticket` passa a recusar.")));
                return;
            }

            var warning = cargo.IsMentionable
                ? string.Empty
                : "\n\n⚠️ Esse cargo **não é mencionável**, então a equipe não recebe notificação quando " +
                  "um ticket abre — a menção sai como texto inerte.";

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                Embeds.Ok("Cargo da equipe definido",
                    $"{cargo.Mention} passa a enxergar todos os tickets deste servidor.{warning}")));
        }

        [SlashCommand("tickets-log", "Canal que recebe o transcript quando um ticket fecha")]
        public Task TicketLogCommand(
            InteractionContext ctx,
            [ChannelTypes(ChannelType.Text, ChannelType.News)]
            [Option("canal", "Deixe vazio para parar de arquivar")] DiscordChannel? canal = null) =>
            ApplyAsync(ctx, canal,
                // AttachFiles porque o transcript e um anexo: sem ela o
                // fechamento falha e o canal do ticket fica de pe, e descobrir
                // isso no primeiro fechamento e tarde demais.
                needs: Permissions.SendMessages | Permissions.EmbedLinks | Permissions.AttachFiles,
                assign: (s, v) => s.ticketLogChannelId = v,
                onSet: c => Embeds.Ok("Log de tickets definido",
                    $"O transcript de cada ticket fechado vai para {c.Mention}, e só então o canal é apagado."),
                onClear: () => Embeds.Ok("Log de tickets desligado",
                    "Sem canal de log não há onde arquivar, então fechar um ticket passa a **trancar** o " +
                    "canal em vez de apagá-lo — a conversa não se perde sem cópia."));

        [SlashCommand("verificacao-cargo", "Cargo que quem verifica a conta Roblox recebe")]
        public Task VerifiedRoleCommand(
            InteractionContext ctx,
            [Option("cargo", "Deixe vazio para não dar cargo nenhum")] DiscordRole? cargo = null) =>
            ApplyRoleAsync(ctx, cargo,
                conflict: s => s.unverifiedRoleId,
                assign: (s, v) => s.verifiedRoleId = v,
                onSet: r => Embeds.Ok("Cargo de verificado definido",
                    $"Quem vincular a conta Roblox recebe {r.Mention} — na hora, pelo `/verify`, ou ao entrar, se já " +
                    "tiver verificado em outro servidor. Quem já estava verificado antes recebe no próximo `/update`."),
                onClear: () => Embeds.Ok("Cargo de verificado desligado",
                    "Verificar a conta deixa de dar cargo. Quem já o tem continua com ele."));

        [SlashCommand("verificacao-nao-verificado", "Cargo de quem ainda não vinculou a conta Roblox")]
        public Task UnverifiedRoleCommand(
            InteractionContext ctx,
            [Option("cargo", "Deixe vazio para desligar")] DiscordRole? cargo = null) =>
            ApplyRoleAsync(ctx, cargo,
                conflict: s => s.verifiedRoleId,
                assign: (s, v) => s.unverifiedRoleId = v,
                // O cargo so e dado em eventos (entrada, /update, /unverify): o
                // bot nao varre o servidor. A resposta diz isso para ninguem
                // esperar ver os membros antigos com o cargo de uma hora para a outra.
                onSet: r => Embeds.Ok("Cargo de não verificado definido",
                    $"Quem **entrar** sem conta vinculada recebe {r.Mention}, e perde o cargo ao verificar. " +
                    "Os membros que já estão no servidor não recebem sozinhos — só ao rodar `/update`."),
                onClear: () => Embeds.Ok("Cargo de não verificado desligado",
                    "Ninguém mais recebe cargo por não ter verificado. Quem já o tem continua com ele."));

        [SlashCommand("verificacao-apelido", "Como o bot escreve o apelido de quem verifica a conta Roblox")]
        public async Task NicknameCommand(
            InteractionContext ctx,
            [Choice("Não mexer no apelido", NicknameFormats.None)]
            [Choice("Nome de usuário do Roblox", NicknameFormats.Username)]
            [Choice("Nome de exibição do Roblox", NicknameFormats.Display)]
            [Choice("Exibição (@usuário), como no BloxLink", NicknameFormats.Smart)]
            [Option("formato", "O que vai no apelido")] string formato)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            var value = formato is NicknameFormats.Username or NicknameFormats.Display or NicknameFormats.Smart
                ? formato
                : null;

            await GuildSettingsStore.Instance.SetAsync(ctx.Guild!.Id, ctx.User.Id, s => s.nicknameFormat = value);

            var warning = value is not null && ctx.Guild.CurrentMember is { } bot &&
                          (bot.Permissions & (Permissions.ManageNicknames | Permissions.Administrator)) == 0
                ? "\n\n⚠️ Me falta **Gerenciar Apelidos**: sem ela, nenhum apelido vai mudar."
                : string.Empty;

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(value is null
                ? Embeds.Ok("Apelido livre", "O bot não mexe mais no apelido de quem verifica.")
                : Embeds.Ok("Formato de apelido definido",
                    $"Quem verificar passa a se chamar pelo **{NicknameFormats.Describe(value)}**. O dono do servidor e " +
                    $"quem está acima do meu cargo ficam de fora, por limite do Discord.{warning}")));
        }

        [SlashCommand("verificacao-idade-minima", "Recusa contas Roblox mais novas que isso, em dias")]
        public async Task MinimumAgeCommand(
            InteractionContext ctx,
            [Option("dias", "Idade mínima da conta Roblox, em dias. 0 desliga")] long dias)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            if (dias is < 0 or > 3650)
            {
                await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                    Embeds.Error("Valor inválido", "A idade mínima vai de 0 (desligada) a 3650 dias.")));
                return;
            }

            await GuildSettingsStore.Instance.SetAsync(ctx.Guild!.Id, ctx.User.Id, s => s.minAccountAgeDays = (int)dias);

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(dias == 0
                ? Embeds.Ok("Filtro anti-alt desligado", "Qualquer conta Roblox vinculada passa a valer neste servidor.")
                : Embeds.Ok("Filtro anti-alt ligado",
                    $"Contas Roblox com menos de **{dias} dia(s)** continuam podendo se vincular, mas neste servidor " +
                    "são tratadas como não verificadas: sem o cargo de verificado e sem os de grupo.")));
        }

        /// <summary>
        /// O caminho comum dos subcomandos de canal: confere que o canal e deste
        /// servidor, confere as permissoes que aquele recurso vai precisar, grava,
        /// e responde.
        ///
        /// A conferencia de servidor parece redundante - o seletor do Discord so
        /// oferece canais daqui -, mas e barata e a alternativa e confiar num
        /// dado que veio de fora. A de permissao e o ponto: sem ela o operador
        /// so descobre o problema quando o recurso falha calado, dias depois.
        /// </summary>
        private static async Task ApplyAsync(
            InteractionContext ctx,
            DiscordChannel? channel,
            Permissions needs,
            Action<GuildSettings, ulong?> assign,
            Func<DiscordChannel, DiscordEmbed> onSet,
            Func<DiscordEmbed> onClear)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            if (channel is not null)
            {
                if (channel.GuildId != ctx.Guild!.Id)
                {
                    await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                        Embeds.Error("Canal de outro servidor",
                            "Só dá para configurar canais deste servidor.")));
                    return;
                }

                if (needs != Permissions.None && ctx.Guild.CurrentMember is { } bot)
                {
                    var missing = needs & ~channel.PermissionsFor(bot);
                    if (missing != Permissions.None)
                    {
                        await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                            Embeds.Error("Faltam permissões nesse canal",
                                $"Não consigo usar {channel.Mention} porque me falta: **{missing}**.\n\n" +
                                "Ajuste as permissões do canal e rode o comando de novo.")));
                        return;
                    }
                }
            }

            await GuildSettingsStore.Instance.SetAsync(ctx.Guild!.Id, ctx.User.Id,
                s => assign(s, channel?.Id));

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                channel is null ? onClear() : onSet(channel)));
        }

        /// <summary>
        /// O caminho comum dos subcomandos de cargo da verificacao. O cargo passa
        /// pela mesma checagem do /bind: o bot vai distribui-lo sozinho, entao ele
        /// nao pode estar acima de quem configura nem acima do bot.
        /// </summary>
        private static async Task ApplyRoleAsync(
            InteractionContext ctx,
            DiscordRole? role,
            Func<GuildSettings, ulong?> conflict,
            Action<GuildSettings, ulong?> assign,
            Func<DiscordRole, DiscordEmbed> onSet,
            Func<DiscordEmbed> onClear)
        {
            await ctx.CreateResponseAsync(InteractionResponseType.DeferredChannelMessageWithSource,
                new DiscordInteractionResponseBuilder().AsEphemeral());

            if (role is not null)
            {
                if (VerificationService.CheckAssignableRole(ctx.Guild!, ctx.Member!, role) is { } refusal)
                {
                    await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(refusal));
                    return;
                }

                // Verificado e nao verificado no mesmo cargo: o bot daria e tiraria
                // o mesmo cargo a cada evento.
                if (GuildSettingsStore.For(ctx.Guild!.Id) is { } current && conflict(current) == role.Id)
                {
                    await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(Embeds.Error("Mesmo cargo",
                        $"{role.Mention} já é o outro cargo da verificação. Verificado e não verificado precisam ser diferentes.")));
                    return;
                }
            }

            await GuildSettingsStore.Instance.SetAsync(ctx.Guild!.Id, ctx.User.Id, s => assign(s, role?.Id));

            await ctx.EditResponseAsync(new DiscordWebhookBuilder().AddEmbed(
                role is null ? onClear() : onSet(role)));
        }

        private static string DescribeRole(DiscordGuild guild, ulong? roleId, string command)
        {
            if (roleId is not { } id)
                return $"— *({command})*";

            return guild.GetRole(id) is { } role
                ? role.Mention
                : $"⚠️ cargo `{id}` não existe mais";
        }

        private static string Describe(DiscordGuild guild, ulong? channelId, string command)
        {
            if (channelId is not { } id)
                return $"— *({command})*";

            return guild.GetChannel(id) is { } channel
                ? channel.Mention
                : $"⚠️ canal `{id}` não existe mais";
        }

        /// <summary>
        /// O que ainda falta para cada recurso funcionar. E a pergunta que quem
        /// configura de fato tem, e que uma lista de valores nao responde.
        /// </summary>
        private static List<string> Pending(DiscordGuild guild, GuildSettings? settings, bool verifyReady)
        {
            var pending = new List<string>();

            var verification = VerificationService.IsConfigured(settings);
            if (verification && !verifyReady)
                pending.Add("a verificação Roblox está configurada aqui, mas **quem administra o bot** ainda não a ligou: " +
                            "o `/verify` responde que está indisponível");

            var hasCategory = settings?.ticketCategoryId is > 0;
            var hasRole = settings?.ticketStaffRoleId is > 0;

            if (hasCategory ^ hasRole)
                pending.Add("os tickets precisam de **categoria e cargo** — só um dos dois não liga nada");

            if (hasCategory && hasRole && settings?.ticketLogChannelId is not > 0)
                pending.Add("sem canal de log de tickets, fechar um ticket **tranca** o canal em vez de apagar");

            if (settings?.autoSoftbanChannelId is > 0 && settings?.moderationLogChannelId is not > 0)
                pending.Add("com a armadilha armada e sem log de moderação, o aviso de disjuntor desarmado " +
                            "não chega a canal nenhum");

            if (guild.CurrentMember is { } bot)
            {
                if (hasCategory && (bot.Permissions & (Permissions.ManageChannels | Permissions.ManageRoles)) !=
                    (Permissions.ManageChannels | Permissions.ManageRoles))
                    pending.Add("me faltam **Gerenciar Canais** e/ou **Gerenciar Cargos**, e sem elas não crio " +
                                "o canal de um ticket");

                if (settings?.autoSoftbanChannelId is > 0 && (bot.Permissions & Permissions.BanMembers) == 0)
                    pending.Add("me falta **Banir Membros**, e sem ela todo softban automático vai falhar");

                var usesRoles = settings is not null &&
                                (settings.verifiedRoleId is > 0 || settings.unverifiedRoleId is > 0 || settings.groupBinds.Count > 0);
                if (usesRoles && (bot.Permissions & (Permissions.ManageRoles | Permissions.Administrator)) == 0)
                    pending.Add("me falta **Gerenciar Cargos**, e sem ela a verificação não dá nem tira cargo nenhum");

                if (settings?.nicknameFormat is not (null or NicknameFormats.None) &&
                    (bot.Permissions & (Permissions.ManageNicknames | Permissions.Administrator)) == 0)
                    pending.Add("me falta **Gerenciar Apelidos**, e sem ela o apelido de quem verifica não muda");
            }

            return pending;
        }
    }
}
